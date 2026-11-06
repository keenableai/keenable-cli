use colored::Colorize;
use serde_json::{Value, json};

use crate::api::{ApiError, api_key_client, api_url, bare_client, handle_response};
use crate::config;
use crate::constants::SEARCH_MODES;
use crate::daemon::{self, DaemonRequest};
use crate::ui;

pub struct SearchFilters {
    pub site: Option<String>,
    pub exclude_domains: Vec<String>,
    pub acquired_after: Option<String>,
    pub acquired_before: Option<String>,
    pub published_after: Option<String>,
    pub published_before: Option<String>,
    pub query_time: Option<String>,
}

impl SearchFilters {
    pub(crate) fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        if let Some(v) = &self.site {
            map.insert("site".into(), json!(v));
        }
        if !self.exclude_domains.is_empty() {
            map.insert("exclude_domains".into(), json!(self.exclude_domains));
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
        if let Some(v) = &self.query_time {
            map.insert("query_time".into(), json!(v));
        }
        Value::Object(map)
    }
}

pub fn tidy_domain_list(raw: Vec<String>) -> Vec<String> {
    if raw.is_empty() {
        return raw;
    }
    let tidy: Vec<String> = raw
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect();
    if tidy.is_empty() {
        vec![String::new()]
    } else {
        tidy
    }
}

fn response_warning(data: &Value) -> Option<&str> {
    data["warning"]
        .as_str()
        .map(str::trim)
        .filter(|w| !w.is_empty())
}

#[derive(Debug, PartialEq)]
enum HumanBlock<'a> {
    Warning(&'a str),
    NoResults,
    Result(usize, &'a Value),
}

fn human_blocks(data: &Value) -> Vec<HumanBlock<'_>> {
    let mut blocks: Vec<HumanBlock<'_>> = response_warning(data)
        .map(HumanBlock::Warning)
        .into_iter()
        .collect();
    match data["results"].as_array() {
        Some(results) if !results.is_empty() => blocks.extend(
            results
                .iter()
                .enumerate()
                .map(|(i, r)| HumanBlock::Result(i, r)),
        ),
        _ => blocks.push(HumanBlock::NoResults),
    }
    blocks
}

fn print_human_result(i: usize, result: &Value) {
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
            // Every command is a read, so a daemon failure is always safe to
            // retry directly — regardless of whether it happened before or
            // after the request may have reached the API.
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
            let query = req.fetch_query().ok_or_else(|| missing("urls"))?;
            let resp = client
                .get(endpoint("/v1/fetch", authenticated))
                .query(&query)
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

fn login_hint(err: &ApiError, authenticated: bool) -> Option<String> {
    if err.is_auth_error() {
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
    }
}

fn error_yaml(err: &ApiError, hint: Option<String>) -> Value {
    let mut val = err.to_yaml_value();
    if let Some(hint) = hint {
        val["hint"] = Value::String(hint);
    }
    val
}

/// Display an ApiError according to output mode (human vs YAML).
fn handle_api_error(err: ApiError, human: bool, api_key_override: Option<&str>) -> ! {
    let authenticated = resolve_api_key(api_key_override).is_some();
    let login_hint = login_hint(&err, authenticated);

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
        print_yaml(&error_yaml(&err, login_hint));
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

fn build_search_body(
    query: &str,
    mode: Option<&str>,
    filters: &SearchFilters,
    snippet_max_length: Option<u64>,
    max_results: Option<u64>,
) -> Value {
    let mut body = json!({ "query": query });
    if let Some(m) = mode {
        body["mode"] = json!(m);
    }
    if let Some(n) = snippet_max_length {
        body["snippet_max_length"] = json!(n);
    }
    if let Some(n) = max_results {
        body["max_results"] = json!(n);
    }
    if let Value::Object(filter_map) = filters.to_json()
        && let Value::Object(ref mut body_map) = body
    {
        body_map.extend(filter_map);
    }
    body
}

pub async fn search(
    query: &str,
    mode: Option<&str>,
    filters: SearchFilters,
    snippet_max_length: Option<u64>,
    max_results: Option<u64>,
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

    let body = build_search_body(
        query,
        effective_mode.as_deref(),
        &filters,
        snippet_max_length,
        max_results,
    );

    let req = DaemonRequest {
        command: "search".to_string(),
        body: Some(body),
        ..Default::default()
    };

    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
    match execute(&req, api_key).await {
        Ok(mut data) => {
            fold_description_into_snippet(&mut data);
            if human {
                ui::header(&format!("keenable search \"{}\"", query));
                for block in human_blocks(&data) {
                    match block {
                        HumanBlock::Warning(warning) => {
                            ui::warning(warning);
                            eprintln!();
                        }
                        HumanBlock::NoResults => {
                            ui::info("No results found.");
                            eprintln!();
                        }
                        HumanBlock::Result(i, result) => print_human_result(i, result),
                    }
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => handle_api_error(e, human, api_key),
    }
}

pub async fn fetch(
    url: &str,
    live: bool,
    prompt: Option<String>,
    max_chars: Option<u64>,
    human: bool,
    api_key: Option<&str>,
) {
    let req = DaemonRequest {
        command: "fetch".to_string(),
        urls: Some(vec![url.to_string()]),
        live,
        prompt,
        max_chars,
        ..Default::default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn filters() -> SearchFilters {
        SearchFilters {
            site: None,
            exclude_domains: vec![],
            acquired_after: None,
            acquired_before: None,
            published_after: None,
            published_before: None,
            query_time: None,
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
            query_time: Some("2024-06-01T00:00:00Z".into()),
            ..filters()
        };
        let v = f.to_json();
        assert_eq!(v["site"], json!("docs.rs"));
        assert_eq!(v["published_after"], json!("2024-01-01"));
        assert_eq!(v["query_time"], json!("2024-06-01T00:00:00Z"));
        assert!(v.get("acquired_after").is_none());
        assert!(v.get("published_before").is_none());
    }

    #[test]
    fn to_json_omits_an_empty_exclude_list() {
        let v = filters().to_json();
        assert!(v.get("exclude_domains").is_none());
    }

    #[test]
    fn to_json_serializes_exclude_domains_as_an_array() {
        let f = SearchFilters {
            exclude_domains: vec!["blogs.un.org".into(), "spam.com".into()],
            ..filters()
        };
        let v = f.to_json();
        assert_eq!(v["exclude_domains"], json!(["blogs.un.org", "spam.com"]));
        assert!(v.get("site").is_none());
    }

    #[test]
    fn to_json_keeps_site_alongside_exclude_domains() {
        let f = SearchFilters {
            site: Some("docs.rs".into()),
            exclude_domains: vec!["old.docs.rs".into()],
            ..filters()
        };
        let v = f.to_json();
        assert_eq!(v["site"], json!("docs.rs"));
        assert_eq!(v["exclude_domains"], json!(["old.docs.rs"]));
    }

    #[test]
    fn to_json_does_not_validate_entries() {
        let f = SearchFilters {
            exclude_domains: vec!["https://example.com".into(), "*".into()],
            ..filters()
        };
        assert_eq!(
            f.to_json()["exclude_domains"],
            json!(["https://example.com", "*"])
        );
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

    #[test]
    fn response_warning_is_surfaced_only_when_set() {
        assert_eq!(
            response_warning(
                &json!({"warning": "Some results were removed by the domain filter."})
            ),
            Some("Some results were removed by the domain filter.")
        );
        assert_eq!(response_warning(&json!({"warning": "  "})), None);
        assert_eq!(response_warning(&json!({"warning": null})), None);
        assert_eq!(response_warning(&json!({"results": []})), None);
    }

    #[test]
    fn tidy_domain_list_leaves_absent_flags_absent() {
        assert!(tidy_domain_list(vec![]).is_empty());
        assert_eq!(tidy_domain_list(vec!["a.com".into()]), vec!["a.com"]);
    }

    #[test]
    fn tidy_domain_list_drops_blanks_beside_a_real_entry() {
        for raw in [
            vec!["", "a.com"],
            vec![" ", "a.com", ""],
            vec!["a.com", "  "],
        ] {
            let raw: Vec<String> = raw.into_iter().map(String::from).collect();
            assert_eq!(tidy_domain_list(raw.clone()), vec!["a.com"], "{raw:?}");
        }
    }

    #[test]
    fn build_search_body_merges_filters_with_request_fields() {
        let f = SearchFilters {
            exclude_domains: vec!["old.docs.rs".into()],
            published_after: Some("2024-01-01".into()),
            ..filters()
        };
        let body = build_search_body("rust async", Some("pro"), &f, Some(500), Some(20));
        assert_eq!(
            body,
            json!({
                "query": "rust async",
                "mode": "pro",
                "snippet_max_length": 500,
                "max_results": 20,
                "exclude_domains": ["old.docs.rs"],
                "published_after": "2024-01-01",
            })
        );
    }

    #[test]
    fn build_search_body_sends_only_the_query_by_default() {
        let body = build_search_body("q", None, &filters(), None, None);
        assert_eq!(body, json!({"query": "q"}));
    }

    #[test]
    fn fold_description_keeps_top_level_fields() {
        let mut data = json!({
            "query": "q",
            "mode": "realtime",
            "warning": "Some results were removed by the domain filter.",
            "results": [{"url": "https://a.com", "description": "d"}],
        });
        fold_description_into_snippet(&mut data);
        assert_eq!(
            data,
            json!({
                "query": "q",
                "mode": "realtime",
                "warning": "Some results were removed by the domain filter.",
                "results": [{"url": "https://a.com", "snippet": "d"}],
            })
        );
    }

    fn invalid_filter() -> ApiError {
        ApiError {
            status: 400,
            error: "invalid_filter".into(),
            message: Some(
                r#"Invalid exclude_domains value "docs.rs/a": paths are not supported"#.into(),
            ),
            retry_after: None,
        }
    }

    #[test]
    fn invalid_filter_is_neither_auth_nor_rate_limit() {
        let err = invalid_filter();
        assert!(!err.is_auth_error());
        assert!(!err.is_rate_limit());
        assert_eq!(login_hint(&err, true), None);
        assert_eq!(login_hint(&err, false), None);
    }

    #[test]
    fn invalid_filter_yaml_keeps_the_message_and_has_no_hint() {
        let err = invalid_filter();
        let v = error_yaml(&err, login_hint(&err, false));
        assert_eq!(
            v,
            json!({
                "error": "invalid_filter",
                "message": r#"Invalid exclude_domains value "docs.rs/a": paths are not supported"#,
            })
        );
    }

    #[test]
    fn auth_and_rate_limit_errors_get_a_login_hint() {
        let auth = ApiError {
            status: 401,
            error: "Authentication failed".into(),
            message: None,
            retry_after: None,
        };
        let v = error_yaml(&auth, login_hint(&auth, true));
        assert!(v["hint"].as_str().unwrap().contains("keenable login"));

        let limited = ApiError {
            status: 429,
            error: "Too many requests".into(),
            message: None,
            retry_after: Some(7),
        };
        assert!(
            login_hint(&limited, true)
                .unwrap()
                .contains("Retry after 7s")
        );
        assert!(
            login_hint(&limited, false)
                .unwrap()
                .contains("increase your limits")
        );
    }

    #[test]
    fn human_blocks_show_the_warning_before_empty_results() {
        let data = json!({"warning": "site: was dropped", "results": []});
        assert_eq!(
            human_blocks(&data),
            vec![
                HumanBlock::Warning("site: was dropped"),
                HumanBlock::NoResults
            ]
        );
    }

    #[test]
    fn human_blocks_show_the_warning_when_results_are_missing() {
        let data = json!({"warning": "site: was dropped"});
        assert_eq!(
            human_blocks(&data),
            vec![
                HumanBlock::Warning("site: was dropped"),
                HumanBlock::NoResults
            ]
        );
    }

    #[test]
    fn human_blocks_list_results_after_the_warning() {
        let data = json!({"warning": "w", "results": [{"url": "a"}, {"url": "b"}]});
        let blocks = human_blocks(&data);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0], HumanBlock::Warning("w"));
        assert_eq!(blocks[1], HumanBlock::Result(0, &data["results"][0]));
        assert_eq!(blocks[2], HumanBlock::Result(1, &data["results"][1]));
    }

    #[test]
    fn human_blocks_show_the_normalization_warning_verbatim() {
        let warning = "Normalized exclude_domains: \"https://UN.org/\" -> \"un.org\", \"WWW.spam.com\" -> \"spam.com\"\nSome results were removed by the domain filter.";
        let data = json!({"warning": warning, "results": [{"url": "a"}]});
        assert_eq!(
            human_blocks(&data),
            vec![
                HumanBlock::Warning(warning),
                HumanBlock::Result(0, &data["results"][0])
            ]
        );
    }

    #[test]
    fn human_blocks_without_a_warning() {
        assert_eq!(
            human_blocks(&json!({"results": []})),
            vec![HumanBlock::NoResults]
        );
        let data = json!({"results": [{"url": "a"}]});
        assert_eq!(
            human_blocks(&data),
            vec![HumanBlock::Result(0, &data["results"][0])]
        );
    }
}
