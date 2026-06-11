use colored::Colorize;
use serde_json::{json, Value};

use crate::api::{api_key_client, api_url, bare_client, handle_response, ApiError};
use crate::config;
use crate::constants::SEARCH_MODES;
use crate::daemon::{self, DaemonRequest};
use crate::ui;

pub struct SearchFilters {
    pub site: Option<String>,
    pub acquired_after: Option<String>,
    pub acquired_before: Option<String>,
    pub published_after: Option<String>,
    pub published_before: Option<String>,
}

impl SearchFilters {
    fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        if let Some(v) = &self.site { map.insert("site".into(), json!(v)); }
        if let Some(v) = &self.acquired_after { map.insert("acquired_after".into(), json!(v)); }
        if let Some(v) = &self.acquired_before { map.insert("acquired_before".into(), json!(v)); }
        if let Some(v) = &self.published_after { map.insert("published_after".into(), json!(v)); }
        if let Some(v) = &self.published_before { map.insert("published_before".into(), json!(v)); }
        Value::Object(map)
    }
}

/// Resolve the API key. Returns Some(key) for authenticated requests, None for public endpoints.
fn resolve_api_key(override_key: Option<&str>) -> Option<String> {
    if let Some(key) = override_key {
        return Some(key.to_string());
    }
    config::get_api_key()
}

fn endpoint(path: &str, authenticated: bool) -> String {
    if authenticated {
        api_url(path)
    } else {
        api_url(&format!("{}/public", path))
    }
}

fn print_yaml(data: &Value) {
    match serde_yaml::to_string(data) {
        Ok(yaml) => print!("{}", yaml),
        Err(_) => println!("{}", serde_json::to_string_pretty(data).unwrap()),
    }
}

/// Execute a request. When an API key override is provided, go direct (skip daemon).
/// Otherwise try daemon first, fall back to direct HTTP.
async fn execute(req: &DaemonRequest, api_key_override: Option<&str>) -> Result<Value, ApiError> {
    // If no override, try daemon first
    if api_key_override.is_none() {
        if daemon::ensure_daemon().is_ok() {
            match daemon::daemon_request(req).await {
                Ok(resp) if resp.ok => return Ok(resp.data.unwrap_or(Value::Null)),
                Ok(resp) => return Err(daemon_response_to_api_error(resp)),
                // If a non-idempotent request may already have reached the
                // API, surface the failure instead of re-sending.
                Err(daemon::DaemonError::AfterSend(e)) if !req.idempotent() => {
                    return Err(ApiError::request_failed(e));
                }
                Err(_) => {} // Fall through to direct
            }
        }
    }

    // Direct HTTP
    let api_key = resolve_api_key(api_key_override);
    let authenticated = api_key.is_some();
    let client = match &api_key {
        Some(key) => api_key_client(key),
        None => bare_client(),
    };

    let send_err = |e: reqwest::Error| ApiError::request_failed(e.to_string());
    let missing = |field: &str| ApiError {
        status: 0,
        error: format!("Missing {}", field),
        message: None,
        retry_after: None,
    };

    match req.command.as_str() {
        "search" => {
            let body = req.body.as_ref().ok_or_else(|| missing("body"))?;
            let resp = client
                .post(endpoint("/v1/search", authenticated))
                .json(body)
                .send()
                .await
                .map_err(send_err)?;
            handle_response(resp).await
        }
        "fetch" => {
            let urls = req.urls.as_ref().ok_or_else(|| missing("urls"))?;
            let resp = client
                .get(endpoint("/v1/fetch", authenticated))
                .query(&urls.iter().map(|u| ("url", u)).collect::<Vec<_>>())
                .send()
                .await
                .map_err(send_err)?;
            handle_response(resp).await
        }
        "feedback" => {
            let body = req.body.as_ref().ok_or_else(|| missing("body"))?;
            let resp = client
                .post(endpoint("/v1/feedback", authenticated))
                .json(body)
                .send()
                .await
                .map_err(send_err)?;
            handle_response(resp).await
        }
        _ => Err(ApiError {
            status: 0,
            error: format!("Unknown command: {}", req.command),
            message: None,
            retry_after: None,
        }),
    }
}

/// Reconstruct an ApiError from a failed DaemonResponse.
/// The daemon forwards the structured error value in `data`.
fn daemon_response_to_api_error(resp: daemon::DaemonResponse) -> ApiError {
    if let Some(data) = &resp.data {
        // Daemon forwards the structured error from handle_response
        let error = data["error"].as_str().unwrap_or("Request failed").to_string();
        let message = data["message"].as_str().map(|s| s.to_string());
        let retry_after = data["retry_after"].as_u64();
        let status = data["status"].as_u64().unwrap_or(0) as u16;
        return ApiError { status, error, message, retry_after };
    }
    // Fallback: use the plain error string
    ApiError {
        status: 0,
        error: resp.error.unwrap_or("Unknown error".to_string()),
        message: None,
        retry_after: None,
    }
}

/// Display an ApiError according to output mode (human vs YAML).
/// Login can't raise limits for an already-authenticated key, so the login
/// hint is shown for auth errors (re-auth fixes a bad key) and anonymous
/// rate limits, but not for authenticated rate limits.
fn handle_api_error(err: ApiError, human: bool, api_key_override: Option<&str>) -> ! {
    let authenticated = resolve_api_key(api_key_override).is_some();
    let login_hint = if err.is_auth_error() {
        Some("to authenticate.")
    } else if err.is_rate_limit() && !authenticated {
        Some("to authenticate and increase your limits.")
    } else {
        None
    };

    if human {
        ui::error(&err.display());
        if err.is_rate_limit() {
            if let Some(secs) = err.retry_after {
                ui::hint(&format!("Retry after {}s.", secs));
            }
        }
        if let Some(suffix) = login_hint {
            ui::hint(&format!("Run {} {}", "keenable login".cyan(), suffix));
        }
        eprintln!();
    } else {
        // YAML error output for agents (retry_after is already a field)
        let mut val = err.to_yaml_value();
        if let Some(suffix) = login_hint {
            val["hint"] = Value::String(format!("Run `keenable login` {}", suffix));
        }
        print_yaml(&val);
    }
    std::process::exit(1);
}

/// Resolve the effective search mode.
/// Priority: forced_search_mode config > --mode flag > default_search_mode config > none.
/// None lets the server default apply (including org-level overrides).
fn resolve_mode(flag: Option<&str>) -> Option<String> {
    let cfg = config::get_config();

    // forced_search_mode always wins
    if let Some(forced) = cfg["forced_search_mode"].as_str() {
        return Some(forced.to_string());
    }

    // --mode flag
    if let Some(m) = flag {
        return Some(m.to_string());
    }

    // default_search_mode as fallback
    cfg["default_search_mode"].as_str().map(|s| s.to_string())
}

pub async fn search(query: &str, mode: Option<&str>, filters: SearchFilters, human: bool, api_key: Option<&str>) {
    // Validate --mode flag if provided ("standard" is a legacy alias for "realtime")
    if let Some(m) = mode {
        if !SEARCH_MODES.contains(&m) && m != "standard" {
            ui::error(&format!("Invalid mode \"{}\". Must be \"{}\".", m, SEARCH_MODES.join("\" or \"")));
            eprintln!();
            std::process::exit(1);
        }
    }

    let effective_mode = resolve_mode(mode).map(|m| {
        // Graceful fallback: "standard" → "realtime"
        if m == "standard" { "realtime".to_string() } else { m }
    });

    let mut body = json!({ "query": query });
    if let Some(m) = &effective_mode {
        body["mode"] = json!(m);
    }
    // Merge filter fields into body
    if let Value::Object(filter_map) = filters.to_json() {
        if let Value::Object(ref mut body_map) = body {
            body_map.extend(filter_map);
        }
    }

    let req = DaemonRequest {
        command: "search".to_string(),
        query: Some(query.to_string()),
        urls: None,
        body: Some(body),
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header(&format!("keenable search \"{}\"", query));
                if let Some(results) = data["results"].as_array() {
                    if results.is_empty() {
                        ui::info("No results found.");
                        eprintln!();
                        return;
                    }
                    for (i, result) in results.iter().enumerate() {
                        let title = result["title"].as_str().unwrap_or("Untitled");
                        let url = result["url"].as_str().unwrap_or("");
                        let description = result["description"].as_str().unwrap_or("");
                        let desc_truncated: String = description.chars().take(200).collect();
                        let published = result["published_at"].as_str().unwrap_or("");
                        let acquired = result["acquired_at"].as_str().unwrap_or("");

                        let num = format!("{:>2}.", i + 1).dimmed();
                        eprintln!("   {} {}", num, title.bold());
                        eprintln!("      {}", url.cyan());
                        if !desc_truncated.is_empty() {
                            eprintln!("      {}", desc_truncated.dimmed());
                        }
                        if !published.is_empty() || !acquired.is_empty() {
                            let mut dates = Vec::new();
                            if !published.is_empty() { dates.push(format!("published: {}", published)); }
                            if !acquired.is_empty() { dates.push(format!("acquired: {}", acquired)); }
                            eprintln!("      {}", dates.join("  ").dimmed());
                        }
                        eprintln!();
                    }
                } else {
                    ui::info("No results found.");
                    eprintln!();
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => handle_api_error(e, human, api_key),
    }
}

pub async fn fetch(url: &str, human: bool, api_key: Option<&str>) {
    let req = DaemonRequest {
        command: "fetch".to_string(),
        query: None,
        urls: Some(vec![url.to_string()]),
        body: None,
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header("keenable fetch");
                let title = data["title"].as_str().unwrap_or("Untitled");
                let url = data["url"].as_str().unwrap_or("");
                let content = data["content"].as_str().unwrap_or("");
                eprintln!("   {} {}", title.bold(), url.cyan());
                eprintln!("   {}", "─".repeat(60).dimmed());
                for line in content.lines() {
                    eprintln!("   {}", line);
                }
                eprintln!();
                return;
            }
            print_yaml(&data);
        }
        Err(e) => handle_api_error(e, human, api_key),
    }
}

pub async fn feedback(query: &str, scores: &[String], human: bool, api_key: Option<&str>) {
    // The API requires a non-empty comment per entry, so reject comment-less
    // entries up front
    let mut relevance: Vec<Value> = Vec::new();
    for entry in scores {
        // URL may contain '=' (e.g. query params), so split from the right.
        // Note this means the comment itself cannot contain '=' — its first
        // '=' would be taken as the score separator.
        let parts: Vec<&str> = entry.rsplitn(3, '=').collect();
        // rsplitn reverses: [comment, score, url]
        if parts.len() < 3 || parts[0].is_empty() || parts[2].is_empty() {
            ui::error(&format!("Invalid format: {}. Expected url=score=comment (comment is required).", entry));
            eprintln!();
            std::process::exit(1);
        }
        let (comment, score_str, url) = (parts[0], parts[1], parts[2]);

        let score: u32 = match score_str.parse() {
            Ok(s) if s <= 5 => s,
            _ => {
                ui::error(&format!("Invalid score in '{}'. Must be 0-5. Expected url=score=comment — note the comment cannot contain '='.", entry));
                eprintln!();
                std::process::exit(1);
            }
        };
        relevance.push(json!({
            "url": url,
            "score": score,
            "comment": comment,
        }));
    }

    let body = json!({
        "query": query,
        "relevance": relevance,
    });

    let req = DaemonRequest {
        command: "feedback".to_string(),
        query: None,
        urls: None,
        body: Some(body),
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header("keenable feedback");
                ui::success("Feedback submitted");
                eprintln!();
                return;
            }
            print_yaml(&json!({"status": "ok", "message": "Feedback submitted", "data": data}));
        }
        Err(e) => handle_api_error(e, human, api_key),
    }
}
