use colored::Colorize;
use serde_json::{Value, json};

use crate::api::{
    ApiError, CAUSE_SEPARATOR, api_key_client, api_url, bare_client, handle_response,
};
use crate::config;
use crate::constants::SEARCH_MODES;
use crate::daemon::{self, DaemonRequest};
use crate::net;
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

/// Trims each entry. Blank entries are kept so the server rejects them.
pub fn trim_domain_entries(raw: Vec<String>) -> Vec<String> {
    raw.iter().map(|v| v.trim().to_string()).collect()
}

fn response_warning(data: &Value) -> Option<&str> {
    data["warning"]
        .as_str()
        .map(str::trim)
        .filter(|w| !w.is_empty())
}

/// Non-filter search parameters; unset ones are left to the server default.
#[derive(Default)]
pub struct SearchOptions {
    pub snippet_max_length: Option<u64>,
    pub max_results: Option<u64>,
    /// Groups related calls (search and fetch) under one session.
    pub session_id: Option<String>,
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
        Ok(yaml) => ui::stdout(&yaml),
        Err(_) => ui::stdout(&format!(
            "{}\n",
            serde_json::to_string_pretty(data).unwrap_or_default()
        )),
    }
}

/// A failed daemon round-trip that a direct request should retry: the
/// daemon's network state (OS trust store, system proxy) may be stale, so a
/// transport failure gets one fresh attempt. Timeouts are not retried — that
/// would double an already long wait. A response without structured data
/// comes from an older daemon or a malformed request; retry those too.
fn retry_directly(resp: &daemon::DaemonResponse) -> bool {
    match &resp.data {
        None => true,
        Some(d) => d["transport"] == true && d["timeout"] != true,
    }
}

/// A failed request, with whether it was sent with an API key. Captured at
/// send time so the hint matches the request even if a concurrent login or
/// logout changes the stored key before the error is shown. Boxed: it is
/// only built on the error path, and keeps `Result` small as ApiError grows.
type Failure = Box<(ApiError, bool)>;

/// Execute a request. When an API key override is provided, or with
/// --verbose (so the network details shown are this process's own), go
/// direct (skip daemon). Otherwise try daemon first, fall back to direct HTTP.
async fn execute(req: &DaemonRequest, api_key_override: Option<&str>) -> Result<Value, Failure> {
    if api_key_override.is_none() {
        // Without a key override the stored key (and the daemon) live under
        // the home directory; report a missing one in the command's own
        // output format. With a key override nothing there is needed.
        if config::home_dir().is_none() {
            let err = ApiError {
                error: config::NO_HOME.into(),
                transport_hint: Some(config::NO_HOME_HINT.into()),
                ..Default::default()
            };
            return Err(Box::new((err, false)));
        }
        if let config::StoredKey::OtherBase { saved_for } = config::stored_key() {
            return Err(Box::new((stored_key_not_used(&saved_for), false)));
        }
    }
    let use_daemon = api_key_override.is_none() && !net::verbose();
    // A bad SSL_CERT_FILE must surface as a clear error, not as a daemon
    // that fails to start. Its hint doesn't depend on the auth state.
    net::check_cert_file().map_err(|e| Box::new((ApiError::from(e), false)))?;
    if use_daemon && daemon::ensure_daemon().is_ok() {
        match daemon::daemon_request(req).await {
            Ok(resp) if resp.ok => return Ok(resp.data.unwrap_or(Value::Null)),
            // Every command is a read, so a daemon failure is always safe to
            // retry directly — regardless of whether it happened before or
            // after the request may have reached the API. Stop the daemon so
            // the next command gets one with fresh network state.
            Ok(resp) if retry_directly(&resp) => daemon::kill_daemon(),
            Ok(resp) => return Err(daemon_response_to_api_error(resp)),
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
    let client = client.map_err(|e| Box::new((ApiError::from(e), authenticated)))?;
    send_direct(&client, req, authenticated)
        .await
        .map_err(|e| Box::new((*e, authenticated)))
}

/// The stored key belongs to another API base than the one this process
/// talks to. Stop rather than send it there, or quietly go anonymous.
fn stored_key_not_used(saved_for: &str) -> ApiError {
    let base = crate::api::api_base();
    ApiError {
        error: "Stored API key not used".into(),
        message: Some(format!(
            "The stored API key was saved for {saved_for} and is not sent to {base}."
        )),
        transport_hint: Some(format!(
            "Pass a key for {base} with KEENABLE_API_KEY or --api-key, or run `keenable login` with {} set to save one for it. Unset {} to use {saved_for}.",
            crate::constants::API_URL_ENV,
            crate::constants::API_URL_ENV
        )),
        ..Default::default()
    }
}

async fn send_direct(
    client: &reqwest::Client,
    req: &DaemonRequest,
    authenticated: bool,
) -> Result<Value, Box<ApiError>> {
    let send_err = |e: reqwest::Error| Box::new(ApiError::from_transport(&e));
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
        "fetch_post" => {
            let body = req.body.as_ref().ok_or_else(|| missing("body"))?;
            let resp = client
                .post(endpoint("/v1/fetch", authenticated))
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
        let text = |key: &str| data[key].as_str().map(str::to_string);
        let cause = text("cause");
        let mut chain: Vec<String> = Vec::new();
        if let Some(c) = &cause {
            chain.push(message.clone().unwrap_or_else(|| error.clone()));
            chain.extend(c.split(CAUSE_SEPARATOR).map(str::to_string));
        } else {
            chain.extend(message.iter().cloned());
        }
        return ApiError {
            status,
            error,
            message,
            retry_after,
            rate_limit_reset,
            request_id,
            // The daemon adds `hint` only for transport and setup failures.
            transport_hint: text("hint"),
            chain,
            transport: data["transport"] == true,
            timeout: data["timeout"] == true,
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

/// Pretty-mode line with the organization's credits left after this
/// request (`usage.credits_remaining`), when the API sent it. Keyless
/// requests carry no usage, so nothing is printed for them.
fn credits_remaining_line(data: &Value) -> Option<String> {
    let left = data["usage"]["credits_remaining"].as_f64()?;
    let shown = if left.fract() == 0.0 {
        format!("{}", left as i64)
    } else {
        format!("{left:.2}")
    };
    Some(format!(
        "Credits left: {shown} ({})",
        crate::api::CONSOLE_URL
    ))
}

/// Display an ApiError according to output mode (human vs YAML).
/// The next step comes from `ApiError::hint`, worded for the auth state the
/// request was sent with (a structured `hint` field in YAML mode). The API's
/// request id is shown too, so users can quote it to support.
fn handle_api_error((err, authenticated): (ApiError, bool), human: bool) -> ! {
    let hint = err.hint(authenticated);

    if human {
        ui::error(&err.display());
        err.print_cause();
        if let Some(hint) = &hint {
            let hint = ui::printable(hint);
            ui::hint(&hint.replace("`keenable login`", &format!("{}", "keenable login".cyan())));
        }
        if let Some(url) = &err.docs_url {
            ui::hint(&format!("Docs: {}", ui::one_line(url)));
        }
        if let Some(id) = &err.request_id {
            ui::hint(&format!(
                "Request ID: {} (quote it when contacting support)",
                ui::one_line(id)
            ));
        }
        eprintln!();
    } else {
        // YAML error output for agents (retry_after, request_id, cause and
        // the v1 type/code/fix/docs_url are already fields; `hint` keeps the
        // CLI's wording and repeats `fix` only where the CLI has no hint)
        if net::verbose() && err.chain.len() > 1 {
            ui::info("Error chain:");
            for (i, msg) in err.chain.iter().enumerate() {
                ui::sub_info(&format!("{}. {}", i + 1, ui::one_line(msg)));
            }
        }
        let mut val = err.to_yaml_value();
        if let Some(hint) = hint {
            val["hint"] = Value::String(hint);
        }
        print_yaml(&val);
        // One line on stderr too, so a script that reads only stderr (or
        // only checks it) sees the failure; the exit code says which kind.
        ui::error_line(&err.display());
    }
    std::process::exit(err.exit_code());
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
    options: &SearchOptions,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("query".into(), json!(query));
    if let Some(m) = mode {
        body.insert("mode".into(), json!(m));
    }
    for extra in [options.to_json(), filters.to_json()] {
        if let Value::Object(map) = extra {
            body.extend(map);
        }
    }
    Value::Object(body)
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

    let body = build_search_body(query, effective_mode.as_deref(), &filters, &options);

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
                if let Some(warning) = response_warning(&data) {
                    ui::warning(&ui::printable(warning));
                    eprintln!();
                }
                let results = data["results"].as_array().filter(|r| !r.is_empty());
                if let Some(results) = results {
                    for (i, result) in results.iter().enumerate() {
                        // Titles, URLs and snippets come from web pages:
                        // never let them drive the terminal.
                        let title = ui::one_line(result["title"].as_str().unwrap_or("Untitled"));
                        let url = ui::one_line(result["url"].as_str().unwrap_or(""));
                        let snippet = result["snippet"].as_str().unwrap_or("");
                        let snippet_flat = ui::one_line(snippet);
                        let snippet_truncated: String = snippet_flat.chars().take(200).collect();

                        let num = format!("{:>2}.", i + 1).dimmed();
                        eprintln!("   {} {}", num, title.bold());
                        eprintln!("      {}", url.cyan());
                        if !snippet_truncated.is_empty() {
                            eprintln!("      {}", snippet_truncated.dimmed());
                        }
                        if let Some(dates) = dates_line(result) {
                            eprintln!("      {}", ui::one_line(&dates).dimmed());
                        }
                        eprintln!();
                    }
                } else {
                    ui::info("No results found.");
                    eprintln!();
                }
                if let Some(line) = credits_remaining_line(&data) {
                    ui::hint(&line);
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
                let title = ui::one_line(data["title"].as_str().unwrap_or("Untitled"));
                let url = ui::one_line(data["url"].as_str().unwrap_or(""));
                let content = ui::printable(data["content"].as_str().unwrap_or(""));
                eprintln!("   {} {}", title.bold(), url.cyan());
                if let Some(dates) = dates_line(&data) {
                    eprintln!("   {}", ui::one_line(&dates).dimmed());
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
                if let Some(line) = credits_remaining_line(&data) {
                    ui::hint(&line);
                    eprintln!();
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => handle_api_error(*e, human),
    }
}

/// Options for `keenable fetch` sent to `POST /v1/fetch`.
#[derive(Debug, Default)]
pub struct FetchManyArgs {
    pub urls: Vec<String>,
    pub max_chars: Option<u64>,
    pub offset: Option<u64>,
    pub no_text: bool,
    pub summary: Option<String>,
    pub output_schema: Option<Value>,
    pub highlights: bool,
    pub highlights_query: Option<String>,
    pub links: Option<u64>,
    pub images: Option<u64>,
    pub max_age: Option<String>,
    pub timeout: Option<String>,
    pub session_id: Option<String>,
}

/// The JSON body `POST /v1/fetch` takes. Unset options are left out; the
/// API judges the values.
pub fn build_fetch_body(args: &FetchManyArgs) -> Value {
    let mut content = serde_json::Map::new();
    if !args.no_text {
        let mut limits = serde_json::Map::new();
        if let Some(n) = args.max_chars {
            limits.insert("max_chars".into(), json!(n));
        }
        if let Some(n) = args.offset {
            limits.insert("offset".into(), json!(n));
        }
        let text = if limits.is_empty() {
            json!(true)
        } else {
            Value::Object(limits)
        };
        content.insert("text".into(), text);
    }
    let mut summary = serde_json::Map::new();
    if let Some(instructions) = args.summary.as_deref().filter(|s| !s.is_empty()) {
        summary.insert("instructions".into(), json!(instructions));
    }
    if let Some(schema) = &args.output_schema {
        summary.insert("output_schema".into(), schema.clone());
    }
    if !summary.is_empty() {
        content.insert("summary".into(), Value::Object(summary));
    }
    if let Some(query) = args.highlights_query.as_deref().filter(|q| !q.is_empty()) {
        content.insert("highlights".into(), json!({ "query": query }));
    } else if args.highlights {
        content.insert("highlights".into(), json!(true));
    }
    if let Some(n) = args.links {
        content.insert("links".into(), json!(n));
    }
    if let Some(n) = args.images {
        content.insert("images".into(), json!(n));
    }
    let mut body = serde_json::Map::new();
    body.insert("urls".into(), json!(args.urls));
    body.insert("content".into(), Value::Object(content));
    for (key, value) in [
        ("max_age", &args.max_age),
        ("timeout", &args.timeout),
        ("session_id", &args.session_id),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            body.insert(key.into(), json!(v));
        }
    }
    Value::Object(body)
}

/// Read a JSON Schema file for `--schema-file`.
pub fn read_schema_file(path: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let schema: Value =
        serde_json::from_str(&text).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    if !schema.is_object() {
        return Err(format!("{path} must hold a JSON object (a JSON Schema)"));
    }
    Ok(schema)
}

/// True when every URL failed: the call succeeded but returned no page.
fn every_url_failed(data: &Value) -> bool {
    let empty = |key: &str| data[key].as_array().is_none_or(|a| a.is_empty());
    empty("results") && !empty("errors")
}

/// `keenable fetch` on `POST /v1/fetch`: several URLs and the v1 options.
pub async fn fetch_many(args: FetchManyArgs, human: bool, api_key: Option<&str>) {
    let req = DaemonRequest {
        command: "fetch_post".to_string(),
        body: Some(build_fetch_body(&args)),
        ..Default::default()
    };
    let api_key = key_override(api_key);
    let api_key = api_key.as_deref();
    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header("keenable fetch");
                for result in data["results"].as_array().into_iter().flatten() {
                    let title = ui::one_line(result["title"].as_str().unwrap_or("Untitled"));
                    let url = ui::one_line(result["url"].as_str().unwrap_or(""));
                    eprintln!("   {} {}", title.bold(), url.cyan());
                    eprintln!("   {}", "─".repeat(60).dimmed());
                    let body = match &result["summary"] {
                        Value::String(s) => s.clone(),
                        Value::Object(_) => {
                            serde_json::to_string_pretty(&result["summary"]).unwrap_or_default()
                        }
                        _ => result["text"].as_str().unwrap_or("").to_string(),
                    };
                    for line in ui::printable(&body).lines() {
                        eprintln!("   {}", line);
                    }
                    if let Some(next) = result["next_offset"].as_u64() {
                        ui::hint(&format!("More text: pass --offset {next} to continue."));
                    }
                    eprintln!();
                }
                for error in data["errors"].as_array().into_iter().flatten() {
                    let url = ui::one_line(error["url"].as_str().unwrap_or(""));
                    let message = ui::one_line(error["message"].as_str().unwrap_or(""));
                    ui::warning(&format!("{url}: {message}"));
                }
            } else {
                print_yaml(&data);
            }
            if every_url_failed(&data) {
                std::process::exit(6);
            }
        }
        Err(e) => handle_api_error(*e, human),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_body_defaults_to_text() {
        let args = FetchManyArgs {
            urls: vec!["https://a.example/".into()],
            ..Default::default()
        };
        assert_eq!(
            build_fetch_body(&args),
            json!({"urls": ["https://a.example/"], "content": {"text": true}})
        );
    }

    #[test]
    fn fetch_body_carries_every_option() {
        let args = FetchManyArgs {
            urls: vec!["https://a.example/".into(), "https://b.example/".into()],
            max_chars: Some(1000),
            offset: Some(10),
            summary: Some("Card fees".into()),
            output_schema: Some(json!({"type": "object"})),
            highlights_query: Some("pricing".into()),
            links: Some(5),
            images: Some(2),
            max_age: Some("1d".into()),
            timeout: Some("15s".into()),
            session_id: Some("run-1".into()),
            ..Default::default()
        };
        assert_eq!(
            build_fetch_body(&args),
            json!({
                "urls": ["https://a.example/", "https://b.example/"],
                "content": {
                    "text": {"max_chars": 1000, "offset": 10},
                    "summary": {"instructions": "Card fees", "output_schema": {"type": "object"}},
                    "highlights": {"query": "pricing"},
                    "links": 5,
                    "images": 2
                },
                "max_age": "1d",
                "timeout": "15s",
                "session_id": "run-1"
            })
        );
    }

    #[test]
    fn fetch_body_without_text_and_with_default_highlights() {
        let args = FetchManyArgs {
            urls: vec!["https://a.example/".into()],
            no_text: true,
            highlights: true,
            summary: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            build_fetch_body(&args),
            json!({"urls": ["https://a.example/"], "content": {"highlights": true}})
        );
    }

    #[test]
    fn every_url_failed_needs_errors_and_no_results() {
        assert!(every_url_failed(
            &json!({"results": [], "errors": [{"url": "u"}]})
        ));
        assert!(!every_url_failed(&json!({"results": [{"url": "u"}]})));
        assert!(!every_url_failed(&json!({"results": []})));
    }

    #[test]
    fn read_schema_file_rejects_non_objects() {
        let dir = std::env::temp_dir().join(format!("keenable-schema-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.json");
        let bad = dir.join("bad.json");
        std::fs::write(&good, r#"{"type":"object"}"#).unwrap();
        std::fs::write(&bad, "[1]").unwrap();
        assert_eq!(
            read_schema_file(good.to_str().unwrap()).unwrap(),
            json!({"type": "object"})
        );
        assert!(read_schema_file(bad.to_str().unwrap()).is_err());
        assert!(read_schema_file(dir.join("missing.json").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn credits_remaining_line_shows_the_balance_only_when_sent() {
        let line = |v: Value| credits_remaining_line(&v);
        assert_eq!(
            line(json!({"usage": {"credits_remaining": 99120}})).as_deref(),
            Some("Credits left: 99120 (https://keenable.ai/console)")
        );
        assert_eq!(
            line(json!({"usage": {"credits_remaining": 12.345}})).as_deref(),
            Some("Credits left: 12.35 (https://keenable.ai/console)")
        );
        assert_eq!(
            line(json!({"usage": {"credits_remaining": -3}})).as_deref(),
            Some("Credits left: -3 (https://keenable.ai/console)")
        );
        assert_eq!(line(json!({"usage": {"credits": 1}})), None);
        assert_eq!(line(json!({"results": []})), None);
        // An empty search still carries the balance, and pretty mode prints
        // it after "No results found." (no early return).
        assert_eq!(
            line(json!({"results": [], "usage": {"credits_remaining": 5}})).as_deref(),
            Some("Credits left: 5 (https://keenable.ai/console)")
        );
    }

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
    fn search_options_include_session() {
        let v = SearchOptions {
            snippet_max_length: Some(500),
            max_results: Some(5),
            session_id: Some("sess-1".into()),
        }
        .to_json();
        assert_eq!(
            v,
            json!({
                "snippet_max_length": 500,
                "max_results": 5,
                "session_id": "sess-1",
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

    fn daemon_err(data: Option<Value>) -> daemon::DaemonResponse {
        daemon::DaemonResponse {
            ok: false,
            data,
            error: Some("x".into()),
        }
    }

    #[test]
    fn daemon_transport_failures_retry_directly_except_timeouts() {
        assert!(retry_directly(&daemon_err(Some(
            json!({"error": "Request failed", "status": 0, "transport": true, "timeout": false})
        ))));
        assert!(!retry_directly(&daemon_err(Some(
            json!({"error": "Request failed", "status": 0, "transport": true, "timeout": true})
        ))));
        // API errors are answers, not network trouble.
        assert!(!retry_directly(&daemon_err(Some(
            json!({"error": "Too many requests", "status": 429})
        ))));
        // Older daemons sent transport errors as a bare string.
        assert!(retry_directly(&daemon_err(None)));
    }

    #[test]
    fn daemon_errors_keep_cause_and_hint() {
        let (err, _) = *daemon_response_to_api_error(daemon_err(Some(json!({
            "error": "Request failed",
            "message": "error sending request",
            "cause": "client error (Connect) → invalid peer certificate: UnknownIssuer",
            "hint": "set SSL_CERT_FILE",
            "status": 0,
            "transport": true,
            "timeout": false,
        }))));
        assert_eq!(err.transport_hint.as_deref(), Some("set SSL_CERT_FILE"));
        assert_eq!(
            err.chain,
            vec![
                "error sending request",
                "client error (Connect)",
                "invalid peer certificate: UnknownIssuer"
            ]
        );
        assert!(err.transport && !err.timeout);
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
    /// round trip; the hint stays the CLI's own (`keenable login` on a 401).
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
        assert_eq!(err.fix.as_deref(), Some("Copy the full key and retry."));
        assert_eq!(
            err.hint(authed).as_deref(),
            Some("Run `keenable login` to authenticate.")
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
    fn trim_domain_entries_leaves_absent_flags_absent() {
        assert!(trim_domain_entries(vec![]).is_empty());
        assert_eq!(trim_domain_entries(vec!["a.com".into()]), vec!["a.com"]);
    }

    #[test]
    fn trim_domain_entries_sends_blanks_beside_a_real_entry() {
        for (raw, want) in [
            (vec!["", "a.com"], vec!["", "a.com"]),
            (vec![" ", "a.com", ""], vec!["", "a.com", ""]),
            (vec!["a.com", "  "], vec!["a.com", ""]),
        ] {
            let raw: Vec<String> = raw.into_iter().map(String::from).collect();
            assert_eq!(trim_domain_entries(raw.clone()), want, "{raw:?}");
        }
    }

    #[test]
    fn build_search_body_merges_filters_with_request_fields() {
        let f = SearchFilters {
            exclude_domains: vec!["old.docs.rs".into()],
            published_after: Some("2024-01-01".into()),
            ..filters()
        };
        let options = SearchOptions {
            snippet_max_length: Some(500),
            max_results: Some(20),
            session_id: None,
        };
        let body = build_search_body("rust async", Some("pro"), &f, &options);
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
        let body = build_search_body("q", None, &filters(), &SearchOptions::default());
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
}
