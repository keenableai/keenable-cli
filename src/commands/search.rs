use colored::Colorize;
use serde_json::{json, Value};

use crate::api::{api_key_client, api_url, bare_client, handle_response, ApiError};
use crate::config;
use crate::constants::{DEFAULT_SEARCH_MODE, SEARCH_MODES};
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

    let send_err = |e: reqwest::Error| ApiError {
        status: 0,
        error: "Request failed".into(),
        message: Some(e.to_string()),
        retry_after: None,
    };
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
fn handle_api_error(err: ApiError, human: bool) -> ! {
    if human {
        ui::error(&err.display());
        if err.is_auth_error() {
            ui::hint(&format!("Run {} to authenticate.", "keenable login".cyan()));
        } else if err.is_rate_limit() {
            if let Some(secs) = err.retry_after {
                ui::hint(&format!("Retry after {}s.", secs));
            }
            ui::hint(&format!(
                "Run {} to authenticate and increase your limits.",
                "keenable login".cyan()
            ));
        }
        eprintln!();
    } else {
        // YAML error output for agents
        let mut val = err.to_yaml_value();
        if err.is_rate_limit() || err.is_auth_error() {
            val["hint"] =
                Value::String("Run `keenable login` to authenticate and increase your limits.".into());
        }
        print_yaml(&val);
    }
    std::process::exit(1);
}

/// Resolve the effective search mode.
/// Priority: forced_search_mode config > --mode flag > default_search_mode config > "pro".
/// Always sends an explicit mode — the server default differs from the documented "pro".
fn resolve_mode(flag: Option<&str>) -> String {
    let cfg = config::get_config();

    // forced_search_mode always wins
    if let Some(forced) = cfg["forced_search_mode"].as_str() {
        return forced.to_string();
    }

    // --mode flag
    if let Some(m) = flag {
        return m.to_string();
    }

    // default_search_mode, then the documented default
    cfg["default_search_mode"]
        .as_str()
        .unwrap_or(DEFAULT_SEARCH_MODE)
        .to_string()
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

    let mut effective_mode = resolve_mode(mode);
    // Graceful fallback: "standard" → "realtime"
    if effective_mode == "standard" {
        effective_mode = "realtime".to_string();
    }

    let mut body = json!({ "query": query, "mode": effective_mode });
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
        Err(e) => handle_api_error(e, human),
    }
}

pub async fn fetch(urls: &[String], human: bool, api_key: Option<&str>) {
    // The API accepts a single URL per request, so fetch each URL separately.
    let mut pages: Vec<Result<Value, ApiError>> = Vec::new();
    for url in urls {
        let req = DaemonRequest {
            command: "fetch".to_string(),
            query: None,
            urls: Some(vec![url.clone()]),
            body: None,
        };
        pages.push(execute(&req, api_key).await);
    }

    // Single URL keeps the original output shape (object / single page).
    if pages.len() == 1 {
        let data = pages
            .into_iter()
            .next()
            .unwrap()
            .unwrap_or_else(|e| handle_api_error(e, human));
        if human {
            ui::header("keenable fetch");
            print_page(&data);
        } else {
            print_yaml(&data);
        }
        return;
    }

    let any_failed = pages.iter().any(|p| p.is_err());
    if human {
        ui::header("keenable fetch");
        for (url, page) in urls.iter().zip(&pages) {
            match page {
                Ok(data) => print_page(data),
                Err(e) => {
                    ui::error(&format!("{}: {}", url, e.display()));
                    eprintln!();
                }
            }
        }
    } else {
        let results: Vec<Value> = urls
            .iter()
            .zip(pages)
            .map(|(url, page)| match page {
                Ok(data) => data,
                Err(e) => {
                    let mut val = e.to_yaml_value();
                    val["url"] = json!(url);
                    val
                }
            })
            .collect();
        print_yaml(&json!({ "results": results }));
    }
    if any_failed {
        std::process::exit(1);
    }
}

fn print_page(data: &Value) {
    let title = data["title"].as_str().unwrap_or("Untitled");
    let url = data["url"].as_str().unwrap_or("");
    let content = data["content"].as_str().unwrap_or("");
    eprintln!("   {} {}", title.bold(), url.cyan());
    eprintln!("   {}", "─".repeat(60).dimmed());
    for line in content.lines() {
        eprintln!("   {}", line);
    }
    eprintln!();
}

pub async fn feedback(query: &str, scores: &[String], human: bool, api_key: Option<&str>) {
    // Parse url=score=comment entries
    let mut relevance: Vec<Value> = Vec::new();
    for entry in scores {
        // Split as url=score or url=score=comment
        // URL may contain '=' (e.g. query params), so split from the right
        let parts: Vec<&str> = entry.rsplitn(3, '=').collect();
        if parts.len() < 2 {
            ui::error(&format!("Invalid format: {}. Expected url=score or url=score=comment.", entry));
            eprintln!();
            std::process::exit(1);
        }

        let (score_str, url, comment) = if parts.len() == 3 {
            // url=score=comment (rsplitn reverses: [comment, score, url])
            (parts[1], parts[2], parts[0])
        } else {
            // url=score (rsplitn reverses: [score, url])
            (parts[0], parts[1], "")
        };

        let score: u32 = match score_str.parse() {
            Ok(s) if s <= 5 => s,
            _ => {
                ui::error(&format!("Invalid score in '{}'. Must be 0-5.", entry));
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
        Err(e) => handle_api_error(e, human),
    }
}
