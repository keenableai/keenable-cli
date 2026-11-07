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
    pub query_time: Option<String>,
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
        if let Some(v) = &self.query_time {
            map.insert("query_time".into(), json!(v));
        }
        Value::Object(map)
    }
}

/// Non-filter search parameters; unset ones are left to the server default.
#[derive(Default)]
pub struct SearchOptions {
    pub snippet_max_length: Option<u64>,
    pub max_results: Option<u64>,
    /// Groups related calls (search and fetch) under one session.
    pub session_id: Option<String>,
    /// Experimental structured-data extraction; pro mode only.
    pub enable_wizards: bool,
}

impl SearchOptions {
    fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        if let Some(n) = self.snippet_max_length {
            map.insert("snippet_max_length".into(), json!(n));
        }
        if let Some(n) = self.max_results {
            map.insert("max_results".into(), json!(n));
        }
        if let Some(id) = &self.session_id {
            map.insert("session_id".into(), json!(id));
        }
        // Sent only when set: the API rejects it outside pro mode, so an
        // explicit `false` would turn a valid realtime search into a 400.
        if self.enable_wizards {
            map.insert("enable_wizards".into(), json!(true));
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

/// Values below this (1973-03-03 in seconds) are not treated as epochs:
/// short digit runs like `2026` or `20261022` are more likely a year or a
/// compact date than a 1970 timestamp, so they are left as sent.
const EPOCH_MIN: f64 = 1e8;

/// Epoch values at or above this are milliseconds: 1e11 seconds is the year
/// 5138, while 1e11 milliseconds is 1973. Same thresholds as the SDKs.
const EPOCH_MS_THRESHOLD: f64 = 1e11;

/// Seconds since the Unix epoch as RFC 3339 UTC (`2026-10-22T19:00:00Z`).
/// None outside years 1970..=9999, which no real page date falls in.
fn epoch_to_rfc3339(secs: i64) -> Option<String> {
    // 253402300800 = 10000-01-01T00:00:00Z
    if !(0..253_402_300_800).contains(&secs) {
        return None;
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month,
        day,
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    ))
}

/// Fetch returns dates as a Unix epoch number while search returns ISO
/// strings. Normalize a numeric (or all-digit string) epoch to RFC 3339 UTC,
/// treating values >= 1e11 as milliseconds. None leaves the value as-is:
/// ISO strings, values below 1e8, and anything unparseable pass through
/// untouched.
fn epoch_value_to_rfc3339(value: &Value) -> Option<String> {
    let raw = match value {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            t.parse::<f64>().ok()?
        }
        _ => return None,
    };
    if !raw.is_finite() || raw < EPOCH_MIN {
        return None;
    }
    let secs = if raw >= EPOCH_MS_THRESHOLD {
        raw / 1_000.0
    } else {
        raw
    };
    epoch_to_rfc3339(secs.floor() as i64)
}

/// Rewrite fetch's epoch `published_at` / `acquired_at` as RFC 3339 so both
/// commands emit the same type for the same field.
fn normalize_fetch_dates(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    for key in ["published_at", "acquired_at"] {
        if let Some(iso) = obj.get(key).and_then(epoch_value_to_rfc3339) {
            obj.insert(key.into(), Value::String(iso));
        }
    }
}

/// The note the fetch API appends to content cut at `max_chars`:
/// `..._This content has been truncated to stay below N characters_...`
const TRUNCATION_NOTE_PREFIX: &str = "..._This content has been truncated to stay below ";
const TRUNCATION_NOTE_SUFFIX: &str = " characters_...";

/// Whether fetched content ends with the API's exact truncation sentence
/// (N is one or more digits), optionally followed by whitespace.
/// Best-effort: it reads false if the API's wording changes.
fn is_truncated(content: &str) -> bool {
    let Some(rest) = content.trim_end().strip_suffix(TRUNCATION_NOTE_SUFFIX) else {
        return false;
    };
    let digits = rest.len() - rest.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    digits > 0 && rest[..rest.len() - digits].ends_with(TRUNCATION_NOTE_PREFIX)
}

/// Flag truncated fetch content with `truncated: true`, placed just before
/// `content` so agents see it before the (long) text. Content stays verbatim.
fn mark_truncated(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    if !obj
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(is_truncated)
    {
        return;
    }
    let mut out = serde_json::Map::with_capacity(obj.len() + 1);
    for (k, v) in std::mem::take(obj) {
        if k == "content" {
            out.insert("truncated".into(), Value::Bool(true));
        }
        if k != "truncated" {
            out.insert(k, v);
        }
    }
    *obj = out;
}

/// "published: …  acquired: …" for pretty output, or None when neither is set.
/// Bare digit strings (an epoch too small to convert) are not dates to show.
fn dates_line(item: &Value) -> Option<String> {
    let mut dates = Vec::new();
    for (key, label) in [("published_at", "published"), ("acquired_at", "acquired")] {
        if let Some(v) = item[key]
            .as_str()
            .filter(|v| !v.trim().bytes().all(|b| b.is_ascii_digit()))
        {
            dates.push(format!("{}: {}", label, v));
        }
    }
    (!dates.is_empty()).then(|| dates.join("  "))
}

fn print_yaml(data: &Value) {
    match serde_yaml::to_string(data) {
        Ok(yaml) => print!("{}", yaml),
        Err(_) => println!("{}", serde_json::to_string_pretty(data).unwrap()),
    }
}

/// A failed request, with whether it was sent with an API key. Captured at
/// send time so the hint matches the request even if a concurrent login or
/// logout changes the stored key before the error is shown. Boxed: it is
/// only built on the error path, and keeps `Result` small as ApiError grows.
type Failure = Box<(ApiError, bool)>;

/// Execute a request. When an API key override is provided, go direct (skip daemon).
/// Otherwise try daemon first, fall back to direct HTTP.
async fn execute(req: &DaemonRequest, api_key_override: Option<&str>) -> Result<Value, Failure> {
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
    send_direct(&client, req, authenticated)
        .await
        .map_err(|e| Box::new((*e, authenticated)))
}

async fn send_direct(
    client: &reqwest::Client,
    req: &DaemonRequest,
    authenticated: bool,
) -> Result<Value, Box<ApiError>> {
    let send_err = |e: reqwest::Error| Box::new(ApiError::request_failed(e.to_string()));
    let missing = |field: &str| {
        Box::new(ApiError {
            error: format!("Missing {}", field),
            ..Default::default()
        })
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
        _ => Err(Box::new(ApiError {
            error: format!("Unknown command: {}", req.command),
            ..Default::default()
        })),
    }
}

/// Reconstruct an ApiError from a failed DaemonResponse, with the auth state
/// the daemon sent it with. The daemon forwards the structured error value
/// (and `authenticated`) in `data`; a daemon from an older version omits
/// `authenticated`, so fall back to the stored key it would have loaded.
fn daemon_response_to_api_error(resp: daemon::DaemonResponse) -> Failure {
    let authenticated = resp
        .data
        .as_ref()
        .and_then(|d| d["authenticated"].as_bool())
        .unwrap_or_else(|| config::get_api_key().is_some());
    Box::new((daemon_error(resp), authenticated))
}

fn daemon_error(resp: daemon::DaemonResponse) -> ApiError {
    if let Some(data) = &resp.data {
        // Daemon forwards the structured error from handle_response
        let error = data["error"]
            .as_str()
            .unwrap_or("Request failed")
            .to_string();
        let message = data["message"].as_str().map(|s| s.to_string());
        let retry_after = data["retry_after"].as_u64();
        let status = data["status"].as_u64().unwrap_or(0) as u16;
        let request_id = data["request_id"].as_str().map(|s| s.to_string());
        let rate_limit_reset = data["rate_limit_reset"].as_str().map(|s| s.to_string());
        return ApiError {
            status,
            error,
            message,
            retry_after,
            rate_limit_reset,
            request_id,
            ..Default::default()
        }
        .with_v1_fields(data);
    }
    // Fallback: use the plain error string
    ApiError {
        error: resp.error.unwrap_or("Unknown error".to_string()),
        ..Default::default()
    }
}

/// Display an ApiError according to output mode (human vs YAML).
/// The next step comes from `ApiError::hint`, worded for the auth state the
/// request was sent with (a structured `hint` field in YAML mode). The API's
/// request id is shown too, so users can quote it to support.
fn handle_api_error((err, authenticated): (ApiError, bool), human: bool) -> ! {
    let hint = err.hint(authenticated);

    if human {
        ui::error(&err.display());
        if let Some(hint) = &hint {
            ui::hint(&hint.replace("`keenable login`", &format!("{}", "keenable login".cyan())));
        }
        if let Some(url) = &err.docs_url {
            ui::hint(&format!("Docs: {}", url));
        }
        if let Some(id) = &err.request_id {
            ui::hint(&format!(
                "Request ID: {} (quote it when contacting support)",
                id
            ));
        }
        eprintln!();
    } else {
        // YAML error output for agents (retry_after, request_id and the v1
        // type/code/fix/docs_url are already fields; `hint` repeats `fix`
        // when the server sent one, so agents reading `hint` get it too)
        let mut val = err.to_yaml_value();
        if let Some(hint) = hint {
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
    options: SearchOptions,
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
    // Merge option and filter fields into body. The body goes to the API
    // verbatim on both the daemon and the direct path.
    if let Value::Object(ref mut body_map) = body {
        for extra in [options.to_json(), filters.to_json()] {
            if let Value::Object(map) = extra {
                body_map.extend(map);
            }
        }
    }

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

                        let num = format!("{:>2}.", i + 1).dimmed();
                        eprintln!("   {} {}", num, title.bold());
                        eprintln!("      {}", url.cyan());
                        if !snippet_truncated.is_empty() {
                            eprintln!("      {}", snippet_truncated.dimmed());
                        }
                        if let Some(dates) = dates_line(result) {
                            eprintln!("      {}", dates.dimmed());
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
        Err(e) => handle_api_error(*e, human),
    }
}

pub async fn fetch(
    url: &str,
    live: bool,
    prompt: Option<String>,
    max_chars: Option<u64>,
    session_id: Option<String>,
    human: bool,
    api_key: Option<&str>,
) {
    let req = DaemonRequest {
        command: "fetch".to_string(),
        urls: Some(vec![url.to_string()]),
        live,
        prompt,
        max_chars,
        session_id,
        ..Default::default()
    };

    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
    match execute(&req, api_key).await {
        Ok(mut data) => {
            if let Some(obj) = data.as_object_mut() {
                obj.remove("description");
            }
            normalize_fetch_dates(&mut data);
            mark_truncated(&mut data);
            if human {
                ui::header("keenable fetch");
                let title = data["title"].as_str().unwrap_or("Untitled");
                let url = data["url"].as_str().unwrap_or("");
                let content = data["content"].as_str().unwrap_or("");
                eprintln!("   {} {}", title.bold(), url.cyan());
                if let Some(dates) = dates_line(&data) {
                    eprintln!("   {}", dates.dimmed());
                }
                eprintln!("   {}", "─".repeat(60).dimmed());
                for line in content.lines() {
                    eprintln!("   {}", line);
                }
                eprintln!();
                if data["truncated"].as_bool() == Some(true) {
                    ui::hint("Content was truncated; pass a larger --max-chars for more.");
                    eprintln!();
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => handle_api_error(*e, human),
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
    fn search_options_omit_unset_fields() {
        assert!(
            SearchOptions::default()
                .to_json()
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn search_options_include_session_and_wizards() {
        let v = SearchOptions {
            snippet_max_length: Some(500),
            max_results: Some(5),
            session_id: Some("sess-1".into()),
            enable_wizards: true,
        }
        .to_json();
        assert_eq!(
            v,
            json!({
                "snippet_max_length": 500,
                "max_results": 5,
                "session_id": "sess-1",
                "enable_wizards": true,
            })
        );
    }

    #[test]
    fn fetch_query_carries_session_id() {
        let req = DaemonRequest {
            command: "fetch".into(),
            urls: Some(vec!["https://example.com".into()]),
            session_id: Some("sess-1".into()),
            ..Default::default()
        };
        let q = req.fetch_query().unwrap();
        assert!(q.contains(&("session_id", "sess-1".to_string())), "{q:?}");
        let bare = DaemonRequest {
            urls: Some(vec!["https://example.com".into()]),
            ..Default::default()
        };
        assert!(
            !bare
                .fetch_query()
                .unwrap()
                .iter()
                .any(|(k, _)| *k == "session_id")
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

    fn daemon_403(data: Value) -> daemon::DaemonResponse {
        daemon::DaemonResponse {
            ok: false,
            data: Some(data),
            error: None,
        }
    }

    #[test]
    fn daemon_error_hint_follows_the_auth_state_it_was_sent_with() {
        let forbidden = json!({"status": 403, "error": "Forbidden", "message": "no"});
        let mut keyed = forbidden.clone();
        keyed["authenticated"] = json!(true);
        let (err, authed) = *daemon_response_to_api_error(daemon_403(keyed));
        assert!(authed);
        assert_eq!(err.status, 403);
        assert!(err.hint(authed).unwrap().contains("revoked"));

        let mut anon = forbidden;
        anon["authenticated"] = json!(false);
        let (err, authed) = *daemon_response_to_api_error(daemon_403(anon));
        assert!(!authed);
        assert!(!err.hint(authed).unwrap().contains("revoked"));
    }

    /// The daemon forwards `to_yaml_value`, so the v1 fields survive the
    /// round trip and the server fix stays the hint.
    #[test]
    fn daemon_error_keeps_the_v1_fields() {
        let sent = ApiError {
            status: 401,
            error: "Authentication failed".into(),
            message: Some("Invalid API key.".into()),
            request_id: Some("req-1".into()),
            ..Default::default()
        }
        .with_v1_fields(&json!({
            "type": "authentication_error",
            "code": "invalid_api_key",
            "fix": "Copy the full key and retry.",
            "docs_url": "https://docs.keenable.ai/errors#invalid-api-key",
        }));
        let mut data = sent.to_yaml_value();
        data["status"] = json!(401);
        data["authenticated"] = json!(true);
        let (err, authed) = *daemon_response_to_api_error(daemon_403(data));
        assert_eq!(err.to_yaml_value(), sent.to_yaml_value());
        assert_eq!(
            err.hint(authed).as_deref(),
            Some("Copy the full key and retry.")
        );
    }

    #[test]
    fn epoch_seconds_become_rfc3339() {
        assert_eq!(
            epoch_value_to_rfc3339(&json!(1792695600)).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
        // The smallest value still read as an epoch.
        assert_eq!(
            epoch_value_to_rfc3339(&json!(100_000_000)).as_deref(),
            Some("1973-03-03T09:46:40Z")
        );
        // Leap day, and the last second of a year.
        assert_eq!(
            epoch_value_to_rfc3339(&json!(951782400)).as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(
            epoch_value_to_rfc3339(&json!(1735689599)).as_deref(),
            Some("2024-12-31T23:59:59Z")
        );
    }

    #[test]
    fn epoch_milliseconds_are_detected() {
        assert_eq!(
            epoch_value_to_rfc3339(&json!(1792695600000_u64)).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
        assert_eq!(
            epoch_value_to_rfc3339(&json!(1792695600999_u64)).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
        // Milliseconds before 2001 (below the old 1e12 threshold).
        assert_eq!(
            epoch_value_to_rfc3339(&json!(946684800000_u64)).as_deref(),
            Some("2000-01-01T00:00:00Z")
        );
        assert_eq!(
            epoch_value_to_rfc3339(&json!("946684800000")).as_deref(),
            Some("2000-01-01T00:00:00Z")
        );
    }

    #[test]
    fn values_below_1e8_are_not_epochs() {
        for v in [
            json!("20261022"),
            json!("2026"),
            json!(20261022),
            json!(2026),
            json!(0),
            json!("0"),
            json!(99_999_999),
        ] {
            assert_eq!(epoch_value_to_rfc3339(&v), None, "{v}");
        }
        // Left exactly as sent, string or number, and hidden from -p.
        let mut data = json!({"published_at": "20261022", "acquired_at": 2026});
        normalize_fetch_dates(&mut data);
        assert_eq!(data["published_at"], json!("20261022"));
        assert_eq!(data["acquired_at"], json!(2026));
        assert_eq!(dates_line(&data), None);
    }

    #[test]
    fn numeric_string_and_float_epochs_convert() {
        assert_eq!(
            epoch_value_to_rfc3339(&json!("1792695600")).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
        assert_eq!(
            epoch_value_to_rfc3339(&json!(" 1792695600 ")).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
        assert_eq!(
            epoch_value_to_rfc3339(&json!(1792695600.75)).as_deref(),
            Some("2026-10-22T19:00:00Z")
        );
    }

    #[test]
    fn iso_strings_and_junk_pass_through() {
        for v in [
            json!("2026-10-22T19:00:00Z"),
            json!("2026-10-22"),
            json!(""),
            json!("yesterday"),
            json!("-5"),
            json!(-5),
            json!(true),
            json!(null),
            json!({"s": 1}),
            json!(1e20),
        ] {
            assert_eq!(epoch_value_to_rfc3339(&v), None, "{v}");
        }
    }

    #[test]
    fn normalize_fetch_dates_rewrites_only_epoch_fields() {
        let mut data = json!({
            "url": "https://example.com",
            "published_at": 1792695600,
            "acquired_at": "2026-10-23T08:00:00Z",
            "content": "1792695600",
        });
        normalize_fetch_dates(&mut data);
        assert_eq!(data["published_at"], "2026-10-22T19:00:00Z");
        assert_eq!(data["acquired_at"], "2026-10-23T08:00:00Z");
        assert_eq!(data["content"], "1792695600");
        // Absent fields stay absent rather than appearing as null.
        let mut bare = json!({"url": "https://example.com"});
        normalize_fetch_dates(&mut bare);
        assert!(bare.get("published_at").is_none());
    }

    #[test]
    fn dates_line_lists_whichever_dates_exist() {
        assert_eq!(
            dates_line(&json!({"published_at": "2026-10-22T19:00:00Z"})).as_deref(),
            Some("published: 2026-10-22T19:00:00Z")
        );
        assert_eq!(
            dates_line(&json!({"published_at": "a", "acquired_at": "b"})).as_deref(),
            Some("published: a  acquired: b")
        );
        assert_eq!(dates_line(&json!({"published_at": ""})), None);
        assert_eq!(dates_line(&json!({})), None);
    }

    const NOTE: &str = "\n\n..._This content has been truncated to stay below 50 characters_...";

    #[test]
    fn truncation_note_is_detected() {
        assert!(is_truncated(&format!("Example Domain{NOTE}")));
        // Trailing whitespace after the note doesn't hide it.
        assert!(is_truncated(&format!("Example Domain{NOTE}\n")));
        // Underscores in the page text before the note are fine.
        assert!(is_truncated(&format!("snake_case_name{NOTE}")));
        assert!(is_truncated(NOTE.trim()));
    }

    #[test]
    fn content_without_the_note_is_not_truncated() {
        assert!(!is_truncated(""));
        assert!(!is_truncated("Example Domain"));
        assert!(!is_truncated("ends with _..."));
        // The note quoted mid-page, not at the end.
        assert!(!is_truncated(&format!("{NOTE}\n\nmore text")));
        assert!(!is_truncated(
            "..._This content has been truncated_with_more_underscores_..."
        ));
        // Same opening words, but not the API's sentence.
        assert!(!is_truncated(
            "Notes\n\n..._This content has been truncated by the author_..."
        ));
        assert!(!is_truncated(
            "..._This content has been truncated to stay below many characters_..."
        ));
        assert!(!is_truncated(
            "..._This content has been truncated to stay below  characters_..."
        ));
    }

    #[test]
    fn mark_truncated_adds_flag_before_content_and_keeps_content() {
        let content = format!("Example Domain{NOTE}");
        let mut data = json!({"url": "u", "title": "t", "content": content, "author": "a"});
        mark_truncated(&mut data);
        assert_eq!(data["truncated"], json!(true));
        assert_eq!(data["content"], json!(content));
        let keys: Vec<_> = data.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["url", "title", "truncated", "content", "author"]);
    }

    #[test]
    fn mark_truncated_leaves_full_content_alone() {
        let mut data = json!({"url": "u", "content": "Example Domain"});
        mark_truncated(&mut data);
        assert!(data.get("truncated").is_none());
        let mut no_content = json!({"url": "u"});
        mark_truncated(&mut no_content);
        assert!(no_content.get("truncated").is_none());
    }

    #[test]
    fn config_mode_ignores_invalid_or_missing() {
        assert_eq!(config_mode(&json!({"m": "bogus"}), "m"), None);
        assert_eq!(config_mode(&json!({}), "m"), None);
    }
}
