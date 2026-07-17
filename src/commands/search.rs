use colored::Colorize;
use serde_json::{Value, json};

use crate::api::{ApiError, api_key_client, api_url, bare_client, handle_response};
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
        if let Some(v) = &self.site {
            map.insert("site".into(), json!(v));
        }
        if let Some(v) = &self.acquired_after {
            map.insert("acquired_after".into(), json!(v));
        }
        if let Some(v) = &self.acquired_before {
            map.insert("acquired_before".into(), json!(v));
        }
        if let Some(v) = &self.published_after {
            map.insert("published_after".into(), json!(v));
        }
        if let Some(v) = &self.published_before {
            map.insert("published_before".into(), json!(v));
        }
        Value::Object(map)
    }
}

/// Resolve the API key. Returns Some(key) for authenticated requests, None for public endpoints.
/// Trimmed — stray whitespace breaks HTTP header building.
fn resolve_api_key(override_key: Option<&str>) -> Option<String> {
    if let Some(key) = override_key {
        return Some(key.trim().to_string());
    }
    config::get_api_key()
}

/// Effective key override: --api-key flag, then the KEENABLE_API_KEY env var.
/// Either bypasses the daemon (whose cached auth state may differ).
fn key_override(flag: Option<&str>) -> Option<String> {
    flag.map(str::to_string)
        .or_else(|| std::env::var("KEENABLE_API_KEY").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn endpoint(path: &str, authenticated: bool) -> String {
    if authenticated {
        api_url(path)
    } else {
        api_url(&format!("{}/public", path))
    }
}

/// Older servers still return `description`; fold it into `snippet` when no
/// snippet exists and drop the field — the CLI never outputs `description`.
fn fold_description_into_snippet(data: &mut Value) {
    let Some(results) = data["results"].as_array_mut() else {
        return;
    };
    for result in results {
        let Some(obj) = result.as_object_mut() else {
            continue;
        };
        let Some(desc) = obj.remove("description") else {
            continue;
        };
        let has_snippet = obj
            .get("snippet")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty());
        if !has_snippet && desc.as_str().is_some_and(|d| !d.is_empty()) {
            obj.insert("snippet".into(), desc);
        }
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
    if api_key_override.is_none() && daemon::ensure_daemon().is_ok() {
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
        let error = data["error"]
            .as_str()
            .unwrap_or("Request failed")
            .to_string();
        let message = data["message"].as_str().map(|s| s.to_string());
        let retry_after = data["retry_after"].as_u64();
        let status = data["status"].as_u64().unwrap_or(0) as u16;
        return ApiError {
            status,
            error,
            message,
            retry_after,
        };
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
/// Auth and rate-limit errors always carry a `keenable login` hint (a
/// structured `hint` field in YAML mode), but the wording is auth-aware:
/// login can't raise limits for a key that's already in use, so an
/// authenticated 429 suggests retrying or switching accounts instead of
/// falsely promising higher limits.
fn handle_api_error(err: ApiError, human: bool, api_key_override: Option<&str>) -> ! {
    let authenticated = resolve_api_key(api_key_override).is_some();
    let login_hint = if err.is_auth_error() {
        Some("Run `keenable login` to authenticate.".to_string())
    } else if err.is_rate_limit() {
        if authenticated {
            let retry = err
                .retry_after
                .map(|s| format!("Retry after {}s", s))
                .unwrap_or_else(|| "Retry later".to_string());
            Some(format!(
                "Rate limit reached for your API key. {}, or run `keenable login` to switch accounts.",
                retry
            ))
        } else {
            Some("Run `keenable login` to authenticate and increase your limits.".to_string())
        }
    } else {
        None
    };

    if human {
        ui::error(&err.display());
        if err.is_rate_limit()
            && !authenticated
            && let Some(secs) = err.retry_after
        {
            ui::hint(&format!("Retry after {}s.", secs));
        }
        if let Some(hint) = &login_hint {
            ui::hint(&hint.replace("`keenable login`", &format!("{}", "keenable login".cyan())));
        }
        eprintln!();
    } else {
        // YAML error output for agents (retry_after is already a field)
        let mut val = err.to_yaml_value();
        if let Some(hint) = login_hint {
            val["hint"] = Value::String(hint);
        }
        print_yaml(&val);
    }
    std::process::exit(1);
}

/// Read a mode from config, ignoring (with a warning) hand-edited invalid
/// values — they would otherwise pass straight through to the server.
fn config_mode(cfg: &Value, key: &str) -> Option<String> {
    let m = cfg[key].as_str()?;
    if SEARCH_MODES.contains(&m) || m == "standard" {
        Some(m.to_string())
    } else {
        ui::warning(&format!(
            "Ignoring invalid {} \"{}\" in config (allowed: {})",
            key,
            m,
            SEARCH_MODES.join(", ")
        ));
        None
    }
}

/// Resolve the effective search mode.
/// Priority: forced_search_mode config > --mode flag > default_search_mode config > none.
/// None lets the server default apply (including org-level overrides).
fn resolve_mode(flag: Option<&str>) -> Option<String> {
    let cfg = config::get_config();

    // forced_search_mode always wins
    if let Some(forced) = config_mode(&cfg, "forced_search_mode") {
        return Some(forced);
    }

    // --mode flag
    if let Some(m) = flag {
        return Some(m.to_string());
    }

    // default_search_mode as fallback
    config_mode(&cfg, "default_search_mode")
}

pub async fn search(
    query: &str,
    mode: Option<&str>,
    filters: SearchFilters,
    human: bool,
    api_key: Option<&str>,
) {
    // Validate --mode flag if provided ("standard" is a legacy alias for "realtime")
    if let Some(m) = mode
        && !SEARCH_MODES.contains(&m)
        && m != "standard"
    {
        ui::error(&format!(
            "Invalid mode \"{}\". Must be \"{}\".",
            m,
            SEARCH_MODES.join("\" or \"")
        ));
        eprintln!();
        std::process::exit(1);
    }

    let effective_mode = resolve_mode(mode).map(|m| {
        // Graceful fallback: "standard" → "realtime"
        if m == "standard" {
            "realtime".to_string()
        } else {
            m
        }
    });

    let mut body = json!({ "query": query });
    if let Some(m) = &effective_mode {
        body["mode"] = json!(m);
    }
    // Merge filter fields into body
    if let Value::Object(filter_map) = filters.to_json()
        && let Value::Object(ref mut body_map) = body
    {
        body_map.extend(filter_map);
    }

    let req = DaemonRequest {
        command: "search".to_string(),
        urls: None,
        body: Some(body),
    };

    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
    match execute(&req, api_key).await {
        Ok(mut data) => {
            fold_description_into_snippet(&mut data);
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
                        let snippet = result["snippet"].as_str().unwrap_or("");
                        let snippet_flat = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                        let snippet_truncated: String = snippet_flat.chars().take(200).collect();
                        let published = result["published_at"].as_str().unwrap_or("");
                        let acquired = result["acquired_at"].as_str().unwrap_or("");

                        let num = format!("{:>2}.", i + 1).dimmed();
                        eprintln!("   {} {}", num, title.bold());
                        eprintln!("      {}", url.cyan());
                        if !snippet_truncated.is_empty() {
                            eprintln!("      {}", snippet_truncated.dimmed());
                        }
                        if !published.is_empty() || !acquired.is_empty() {
                            let mut dates = Vec::new();
                            if !published.is_empty() {
                                dates.push(format!("published: {}", published));
                            }
                            if !acquired.is_empty() {
                                dates.push(format!("acquired: {}", acquired));
                            }
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
        urls: Some(vec![url.to_string()]),
        body: None,
    };

    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
    match execute(&req, api_key).await {
        Ok(mut data) => {
            if let Some(obj) = data.as_object_mut() {
                obj.remove("description");
            }
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
            ui::error(&format!(
                "Invalid format: {}. Expected url=score=comment (comment is required).",
                entry
            ));
            eprintln!();
            std::process::exit(1);
        }
        let (comment, score_str, url) = (parts[0], parts[1], parts[2]);

        let score: u32 = match score_str.parse() {
            Ok(s) if s <= 5 => s,
            _ => {
                ui::error(&format!(
                    "Invalid score in '{}'. Must be 0-5. Expected url=score=comment — note the comment cannot contain '='.",
                    entry
                ));
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
        urls: None,
        body: Some(body),
    };

    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn filters() -> SearchFilters {
        SearchFilters {
            site: None,
            acquired_after: None,
            acquired_before: None,
            published_after: None,
            published_before: None,
        }
    }

    #[test]
    fn to_json_omits_unset_filters() {
        assert!(filters().to_json().as_object().unwrap().is_empty());
    }

    #[test]
    fn to_json_includes_only_set_filters() {
        let f = SearchFilters {
            site: Some("docs.rs".into()),
            published_after: Some("2024-01-01".into()),
            ..filters()
        };
        let v = f.to_json();
        assert_eq!(v["site"], json!("docs.rs"));
        assert_eq!(v["published_after"], json!("2024-01-01"));
        assert!(v.get("acquired_after").is_none());
        assert!(v.get("published_before").is_none());
    }

    #[test]
    fn config_mode_accepts_valid_and_legacy_standard() {
        assert_eq!(
            config_mode(&json!({"m": "pro"}), "m").as_deref(),
            Some("pro")
        );
        assert_eq!(
            config_mode(&json!({"m": "realtime"}), "m").as_deref(),
            Some("realtime")
        );
        // "standard" is the legacy alias and is accepted here (mapped later).
        assert_eq!(
            config_mode(&json!({"m": "standard"}), "m").as_deref(),
            Some("standard")
        );
    }

    #[test]
    fn config_mode_ignores_invalid_or_missing() {
        assert_eq!(config_mode(&json!({"m": "bogus"}), "m"), None);
        assert_eq!(config_mode(&json!({}), "m"), None);
    }
}
