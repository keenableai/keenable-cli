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
    /// What the search is for; the API logs it and does not change results.
    pub objective: Option<String>,
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
        if let Some(objective) = self
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
        {
            map.insert("objective".into(), json!(objective));
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

/// What the --api-key flag and the KEENABLE_API_KEY env var ask for.
#[derive(Debug, PartialEq)]
enum KeyOverride {
    /// Use this key; bypasses the daemon (whose cached auth state may differ).
    Key(String),
    /// Neither is set: the stored key, or the public endpoints.
    Unset,
    /// KEENABLE_API_KEY is set but blank: ignored, with a warning.
    BlankEnv,
    /// --api-key was passed a blank value: an error, never a silent fallback
    /// to another key or to anonymous requests.
    BlankFlag,
}

/// Effective key override: --api-key flag, then the KEENABLE_API_KEY env var.
fn key_override(flag: Option<&str>, env: Option<&str>) -> KeyOverride {
    let key = |s: &str| Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string);
    match (flag, env) {
        (Some(flag), _) => key(flag).map_or(KeyOverride::BlankFlag, KeyOverride::Key),
        (None, Some(env)) => key(env).map_or(KeyOverride::BlankEnv, KeyOverride::Key),
        (None, None) => KeyOverride::Unset,
    }
}

const BLANK_ENV_KEY_WARNING: &str = "KEENABLE_API_KEY is set but empty; ignoring it.";

/// The key override for this command. A blank --api-key exits 1 in the
/// command's own output format; a blank KEENABLE_API_KEY warns on stderr.
pub(crate) fn key_override_or_exit(flag: Option<&str>, human: bool) -> Option<String> {
    let env = std::env::var("KEENABLE_API_KEY").ok();
    match key_override(flag, env.as_deref()) {
        KeyOverride::Key(key) => Some(key),
        KeyOverride::Unset => None,
        KeyOverride::BlankEnv => {
            ui::warning(BLANK_ENV_KEY_WARNING);
            None
        }
        KeyOverride::BlankFlag => handle_api_error((blank_flag_key(), false), human),
    }
}

fn blank_flag_key() -> ApiError {
    ApiError {
        error: "Empty API key".into(),
        message: Some("--api-key was given an empty value.".into()),
        transport_hint: Some(
            "Pass the key itself, or leave --api-key out to use the stored key.".into(),
        ),
        ..Default::default()
    }
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

pub(crate) fn print_yaml(data: &Value) {
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
pub(crate) fn stored_key_not_used(saved_for: &str) -> ApiError {
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
        "fetch" => {
            let request = req
                .fetch_request(client, crate::api::api_base(), authenticated)
                .ok_or_else(|| missing("urls"))?;
            handle_response(request.send().await.map_err(send_err)?).await
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
pub(crate) fn handle_api_error((err, authenticated): (ApiError, bool), human: bool) -> ! {
    report_api_error(&err, authenticated, human);
    std::process::exit(err.exit_code());
}

fn report_api_error(err: &ApiError, authenticated: bool, human: bool) {
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

    let api_key = key_override_or_exit(api_key, human);
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

pub const MAX_FETCH_URLS: usize = 20;
const MAX_AGE_HOURS_LIMIT: f64 = 720.0;

/// `--max-age-hours`: a finite number of hours from 0 to 720, fractions allowed.
pub fn parse_max_age_hours(raw: &str) -> Result<f64, String> {
    let hours: f64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("`{raw}` is not a number of hours"))?;
    if !hours.is_finite() || !(0.0..=MAX_AGE_HOURS_LIMIT).contains(&hours) {
        return Err(format!("must be from 0 to {MAX_AGE_HOURS_LIMIT}"));
    }
    Ok(hours)
}

/// `--schema`: a JSON object given inline, or `@path` to read it from a file.
pub fn parse_schema(raw: &str) -> Result<Value, String> {
    let text = match raw.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read schema file `{path}`: {e}"))?,
        None => raw.to_string(),
    };
    let schema: Value = serde_json::from_str(&text).map_err(|e| format!("not valid JSON: {e}"))?;
    if !schema.is_object() {
        return Err("must be a JSON object (a JSON Schema whose root is an object)".into());
    }
    Ok(schema)
}

/// `keenable fetch` options. Unset ones are left to the server default.
#[derive(Default)]
pub struct FetchOptions {
    pub max_chars: Option<u64>,
    pub prompt: Option<String>,
    pub max_age_hours: Option<f64>,
    pub schema: Option<Value>,
    pub links: Option<u64>,
    pub images: Option<u64>,
    pub session_id: Option<String>,
}

fn hours_json(hours: f64) -> Value {
    if hours.fract() == 0.0 {
        json!(hours as u64)
    } else {
        json!(hours)
    }
}

impl FetchOptions {
    fn post_body(&self, urls: &[String]) -> Value {
        let mut content = serde_json::Map::new();
        if let Some(n) = self.max_chars {
            content.insert("max_chars".into(), json!(n));
        }
        if let Some(p) = &self.prompt {
            content.insert("prompt".into(), json!(p));
        }
        if let Some(s) = &self.schema {
            content.insert("schema".into(), s.clone());
        }
        if let Some(n) = self.links {
            content.insert("links".into(), json!(n));
        }
        if let Some(n) = self.images {
            content.insert("images".into(), json!(n));
        }
        let mut body = serde_json::Map::new();
        body.insert("urls".into(), json!(urls));
        if !content.is_empty() {
            body.insert("content".into(), Value::Object(content));
        }
        if let Some(h) = self.max_age_hours {
            body.insert("retrieval".into(), json!({"max_age_hours": hours_json(h)}));
        }
        Value::Object(body)
    }

    /// What this call asks for that the keyless GET route can't do.
    fn needs_key(&self, urls: &[String]) -> Vec<&'static str> {
        let mut needs = Vec::new();
        if urls.len() > 1 {
            needs.push("more than one URL");
        }
        if self.max_age_hours.is_some_and(|h| h != 0.0) {
            needs.push("--max-age-hours above 0");
        }
        if self.schema.is_some() {
            needs.push("--schema");
        }
        if self.links.is_some() {
            needs.push("--links");
        }
        if self.images.is_some() {
            needs.push("--images");
        }
        needs
    }

    fn keyless_request(self, url: &str) -> DaemonRequest {
        DaemonRequest {
            command: "fetch".to_string(),
            urls: Some(vec![url.to_string()]),
            live: self.max_age_hours == Some(0.0),
            prompt: self.prompt,
            max_chars: self.max_chars,
            session_id: self.session_id,
            ..Default::default()
        }
    }

    fn keyed_request(self, urls: &[String]) -> DaemonRequest {
        DaemonRequest {
            command: "fetch".to_string(),
            body: Some(self.post_body(urls)),
            session_id: self.session_id,
            ..Default::default()
        }
    }
}

fn key_required(needs: &[&str]) -> ApiError {
    ApiError {
        error: "API key required".into(),
        message: Some(format!("Not available without a key: {}.", needs.join(", "))),
        transport_hint: Some(
            "Log in with `keenable login`, or pass --api-key. Without a key, fetch takes one URL with --max-chars, --prompt and --max-age-hours 0."
                .into(),
        ),
        ..Default::default()
    }
}

/// The CLI's usage error code, as clap uses for invalid arguments.
const EXIT_INVALID_ARGUMENTS: i32 = 2;

fn normalize_page(page: &mut Value) {
    if let Some(obj) = page.as_object_mut() {
        obj.remove("description");
    }
    normalize_fetch_dates(page);
    mark_truncated(page);
}

fn is_page_error(entry: &Value) -> bool {
    entry.get("error").is_some_and(Value::is_object)
}

/// A failed page from a POST /v1/fetch result, as the error that URL
/// alone would have returned.
fn page_error(entry: &Value) -> ApiError {
    let body = &entry["error"];
    let text = |key: &str| body[key].as_str().map(str::to_string);
    ApiError {
        status: body["status"].as_u64().unwrap_or(0) as u16,
        error: text("error").unwrap_or_else(|| "Request failed".into()),
        message: text("message"),
        retry_after: body["retry_after_seconds"]
            .as_u64()
            .or_else(|| body["retryAfter"].as_u64()),
        ..Default::default()
    }
    .with_v1_fields(body)
}

/// The exit code most failed pages share; the first one seen on a tie.
fn dominant_exit_code(errors: &[ApiError]) -> i32 {
    let codes: Vec<i32> = errors.iter().map(ApiError::exit_code).collect();
    let count = |c: i32| codes.iter().filter(|&&x| x == c).count();
    codes
        .iter()
        .copied()
        .fold(None, |best: Option<i32>, c| match best {
            Some(b) if count(b) >= count(c) => Some(b),
            _ => Some(c),
        })
        .unwrap_or(crate::api::exit_code::ERROR)
}

/// Pretty-mode credits line for a batch: the lowest balance any page reported.
fn lowest_credits_line(pages: &[Value]) -> Option<String> {
    pages
        .iter()
        .filter(|p| p["usage"]["credits_remaining"].is_number())
        .min_by(|a, b| {
            let left = |v: &Value| v["usage"]["credits_remaining"].as_f64().unwrap_or(0.0);
            left(a).total_cmp(&left(b))
        })
        .and_then(credits_remaining_line)
}

fn print_page(page: &Value) {
    let title = ui::one_line(page["title"].as_str().unwrap_or("Untitled"));
    let url = ui::one_line(page["url"].as_str().unwrap_or(""));
    let content = ui::printable(page["content"].as_str().unwrap_or(""));
    eprintln!("   {} {}", title.bold(), url.cyan());
    if let Some(dates) = dates_line(page) {
        eprintln!("   {}", ui::one_line(&dates).dimmed());
    }
    for (key, label) in [("image", "image"), ("favicon", "favicon")] {
        if let Some(url) = page[key].as_str().filter(|u| !u.is_empty()) {
            eprintln!("   {}", format!("{label}: {}", ui::one_line(url)).dimmed());
        }
    }
    if page["stale"].as_bool() == Some(true) {
        ui::warning("Older stored copy: a fresh read failed.");
    }
    eprintln!("   {}", "─".repeat(60).dimmed());
    for line in content.lines() {
        eprintln!("   {}", line);
    }
    eprintln!();
    if let Some(structured) = page.get("structured").filter(|v| !v.is_null()) {
        eprintln!("   {}", "Structured".bold());
        let text = serde_json::to_string_pretty(structured).unwrap_or_default();
        for line in ui::printable(&text).lines() {
            eprintln!("   {}", line);
        }
        eprintln!();
    }
    if let Some(err) = page.get("answer_error").filter(|v| v.is_object()) {
        let message = err["message"].as_str().unwrap_or("no answer");
        let code = err["code"].as_str().unwrap_or("");
        ui::warning(&ui::one_line(&format!(
            "No structured answer: {message} ({code})"
        )));
        eprintln!();
    }
    for (key, label, text_key) in [("links", "Links", "text"), ("images", "Images", "alt")] {
        let Some(items) = page[key].as_array().filter(|a| !a.is_empty()) else {
            continue;
        };
        eprintln!("   {}", format!("{label} ({})", items.len()).bold());
        for item in items {
            let url = ui::one_line(item["url"].as_str().unwrap_or(""));
            match item[text_key]
                .as_str()
                .map(ui::one_line)
                .filter(|t| !t.is_empty())
            {
                Some(text) => eprintln!("   {}  {}", url.cyan(), text.dimmed()),
                None => eprintln!("   {}", url.cyan()),
            }
        }
        eprintln!();
    }
    if page["truncated"].as_bool() == Some(true) {
        ui::hint("Content was truncated; pass a larger --max-chars for more.");
        eprintln!();
    }
}

fn print_single_page(page: &Value, human: bool) {
    if !human {
        print_yaml(page);
        return;
    }
    ui::header("keenable fetch");
    print_page(page);
    if let Some(line) = credits_remaining_line(page) {
        ui::hint(&line);
        eprintln!();
    }
}

pub async fn fetch(urls: &[String], options: FetchOptions, human: bool, api_key: Option<&str>) {
    let api_key = key_override_or_exit(api_key, human);
    let keyed = api_key.is_some() || !matches!(config::stored_key(), config::StoredKey::None);
    let api_key = api_key.as_deref();

    if !keyed {
        let needs = options.needs_key(urls);
        if !needs.is_empty() {
            report_api_error(&key_required(&needs), false, human);
            std::process::exit(EXIT_INVALID_ARGUMENTS);
        }
        let Some(url) = urls.first() else {
            return;
        };
        match execute(&options.keyless_request(url), api_key).await {
            Ok(mut page) => {
                normalize_page(&mut page);
                print_single_page(&page, human);
            }
            Err(e) => handle_api_error(*e, human),
        }
        return;
    }

    let mut data = match execute(&options.keyed_request(urls), api_key).await {
        Ok(data) => data,
        Err(e) => handle_api_error(*e, human),
    };
    let Some(results) = data.get_mut("results").and_then(Value::as_array_mut) else {
        let err = ApiError {
            error: "Failed to parse response".into(),
            message: Some("The response has no `results` list.".into()),
            ..Default::default()
        };
        handle_api_error((err, true), human);
    };
    for entry in results.iter_mut().filter(|e| !is_page_error(e)) {
        normalize_page(entry);
    }
    let results = data["results"].as_array().map_or(&[][..], Vec::as_slice);

    if let [only] = results {
        if is_page_error(only) {
            handle_api_error((page_error(only), true), human);
        }
        print_single_page(only, human);
        return;
    }

    let errors: Vec<ApiError> = results
        .iter()
        .filter(|e| is_page_error(e))
        .map(page_error)
        .collect();
    let all_failed = !results.is_empty() && errors.len() == results.len();

    if human {
        ui::header(&format!("keenable fetch ({} URLs)", results.len()));
        for (i, entry) in results.iter().enumerate() {
            eprintln!("   {}", format!("[{}/{}]", i + 1, results.len()).dimmed());
            if is_page_error(entry) {
                let err = page_error(entry);
                let url = ui::one_line(entry["url"].as_str().unwrap_or(""));
                ui::error(&format!("{url}: {}", err.display()));
                if let Some(hint) = err.hint(true) {
                    ui::hint(&ui::printable(&hint));
                }
                eprintln!();
            } else {
                print_page(entry);
            }
        }
        if let Some(line) = lowest_credits_line(results) {
            ui::hint(&line);
            eprintln!();
        }
        if all_failed {
            ui::error(&format!("All {} URLs failed.", results.len()));
            eprintln!();
        }
    } else {
        print_yaml(&data);
        if all_failed {
            ui::error_line(&format!("all {} URLs failed", errors.len()));
        }
    }
    if all_failed {
        std::process::exit(dominant_exit_code(&errors));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn search_options_send_a_trimmed_objective_only_when_it_has_text() {
        let with = SearchOptions {
            objective: Some("  Find the 2024 CBP count for NAICS 812112.  ".into()),
            ..Default::default()
        }
        .to_json();
        assert_eq!(
            with,
            json!({"objective": "Find the 2024 CBP count for NAICS 812112."})
        );

        for blank in [None, Some(String::new()), Some("   ".to_string())] {
            let without = SearchOptions {
                objective: blank,
                ..Default::default()
            }
            .to_json();
            assert!(without.as_object().unwrap().is_empty());
        }
    }

    #[test]
    fn search_options_include_session() {
        let v = SearchOptions {
            snippet_max_length: Some(500),
            max_results: Some(5),
            session_id: Some("sess-1".into()),
            ..Default::default()
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
            objective: None,
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

    #[test]
    fn key_override_takes_the_flag_then_the_env() {
        use KeyOverride::{Key, Unset};
        assert_eq!(key_override(None, None), Unset);
        assert_eq!(key_override(Some(" keen_f\n"), None), Key("keen_f".into()));
        assert_eq!(key_override(None, Some(" keen_e ")), Key("keen_e".into()));
        assert_eq!(
            key_override(Some("keen_f"), Some("keen_e")),
            Key("keen_f".into())
        );
        assert_eq!(key_override(Some("keen_f"), Some("")), Key("keen_f".into()));
    }

    #[test]
    fn a_blank_flag_is_an_error_and_never_falls_back() {
        use KeyOverride::{BlankEnv, BlankFlag};
        for blank in ["", " ", "\t\n"] {
            assert_eq!(key_override(Some(blank), None), BlankFlag, "{blank:?}");
            // Not even to a good key in the environment.
            assert_eq!(key_override(Some(blank), Some("keen_e")), BlankFlag);
            assert_eq!(key_override(None, Some(blank)), BlankEnv, "{blank:?}");
        }
    }

    #[test]
    fn a_blank_flag_error_is_a_local_error_with_a_hint() {
        let err = blank_flag_key();
        assert_eq!(err.exit_code(), 1);
        assert_eq!(
            err.display(),
            "Empty API key: --api-key was given an empty value."
        );
        assert!(err.hint(false).unwrap().contains("leave --api-key out"));
    }

    fn urls(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("https://example.com/{i}")).collect()
    }

    #[test]
    fn max_age_hours_takes_0_to_720_with_fractions() {
        assert_eq!(parse_max_age_hours("0"), Ok(0.0));
        assert_eq!(parse_max_age_hours("1.5"), Ok(1.5));
        assert_eq!(parse_max_age_hours("720"), Ok(720.0));
        for bad in ["-1", "720.5", "NaN", "inf", "soon", ""] {
            assert!(parse_max_age_hours(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn schema_is_a_json_object_inline_or_from_a_file() {
        let inline = parse_schema(r#"{"type": "object"}"#).unwrap();
        assert_eq!(inline, json!({"type": "object"}));

        let dir = std::env::temp_dir().join(format!("keenable-schema-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("schema.json");
        std::fs::write(&file, r#"{"type": "object", "properties": {}}"#).unwrap();
        let from_file = parse_schema(&format!("@{}", file.display())).unwrap();
        assert_eq!(from_file["properties"], json!({}));
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(parse_schema("[1]").unwrap_err().contains("JSON object"));
        assert!(
            parse_schema("{nope")
                .unwrap_err()
                .contains("not valid JSON")
        );
        assert!(
            parse_schema("@/no/such/schema.json")
                .unwrap_err()
                .contains("cannot read")
        );
    }

    #[test]
    fn post_body_sends_only_the_urls_by_default() {
        let body = FetchOptions::default().post_body(&urls(2));
        assert_eq!(
            body,
            json!({"urls": ["https://example.com/0", "https://example.com/1"]})
        );
    }

    #[test]
    fn post_body_maps_every_flag() {
        let options = FetchOptions {
            max_chars: Some(500),
            prompt: Some("List the plans".into()),
            max_age_hours: Some(24.0),
            schema: Some(json!({"type": "object"})),
            links: Some(10),
            images: Some(0),
            session_id: Some("sess-1".into()),
        };
        assert_eq!(
            options.post_body(&urls(1)),
            json!({
                "urls": ["https://example.com/0"],
                "content": {
                    "max_chars": 500,
                    "prompt": "List the plans",
                    "schema": {"type": "object"},
                    "links": 10,
                    "images": 0,
                },
                "retrieval": {"max_age_hours": 24},
            })
        );
        let half = FetchOptions {
            max_age_hours: Some(0.5),
            ..Default::default()
        };
        assert_eq!(half.post_body(&urls(1))["retrieval"]["max_age_hours"], 0.5);
    }

    #[test]
    fn keyed_fetch_posts_the_body_and_keeps_the_session() {
        let req = FetchOptions {
            session_id: Some("sess-1".into()),
            max_age_hours: Some(0.0),
            ..Default::default()
        }
        .keyed_request(&urls(1));
        assert_eq!(req.command, "fetch");
        assert_eq!(req.session_id.as_deref(), Some("sess-1"));
        assert_eq!(req.body.unwrap()["retrieval"]["max_age_hours"], 0);
        assert!(!req.live && req.urls.is_none());
    }

    #[test]
    fn keyless_fetch_needs_one_url_and_get_only_flags() {
        let plain = FetchOptions {
            max_chars: Some(10),
            prompt: Some("p".into()),
            max_age_hours: Some(0.0),
            session_id: Some("s".into()),
            ..Default::default()
        };
        assert!(plain.needs_key(&urls(1)).is_empty());
        assert_eq!(plain.needs_key(&urls(2)), ["more than one URL"]);

        let post_only = FetchOptions {
            max_age_hours: Some(1.0),
            schema: Some(json!({"type": "object"})),
            links: Some(1),
            images: Some(0),
            ..Default::default()
        };
        assert_eq!(
            post_only.needs_key(&urls(1)),
            ["--max-age-hours above 0", "--schema", "--links", "--images"]
        );
    }

    #[test]
    fn keyless_fetch_sends_max_age_0_as_live() {
        let req = FetchOptions {
            max_age_hours: Some(0.0),
            max_chars: Some(10),
            prompt: Some("p".into()),
            ..Default::default()
        }
        .keyless_request("https://example.com");
        assert!(req.live);
        assert!(req.body.is_none());
        let q = req.fetch_query().unwrap();
        assert!(q.contains(&("live", "true".to_string())), "{q:?}");
        assert!(q.contains(&("max_chars", "10".to_string())), "{q:?}");
        assert!(
            !FetchOptions::default()
                .keyless_request("https://example.com")
                .live
        );
    }

    #[test]
    fn key_required_names_what_needs_a_key() {
        let err = key_required(&["more than one URL", "--schema"]);
        assert_eq!(
            err.display(),
            "API key required: Not available without a key: more than one URL, --schema."
        );
        assert!(err.hint(false).unwrap().contains("--api-key"));
    }

    #[test]
    fn a_failed_page_becomes_that_urls_error() {
        let entry = json!({"url": "https://dead.example", "error": {
            "status": 404, "error": "Not found", "message": "gone",
            "type": "target_error", "code": "page_not_found",
        }});
        assert!(is_page_error(&entry));
        let err = page_error(&entry);
        assert_eq!(err.status, 404);
        assert_eq!(err.display(), "Not found: gone");
        assert_eq!(err.code.as_deref(), Some("page_not_found"));
        assert_eq!(err.exit_code(), crate::api::exit_code::TARGET);

        let limited = page_error(&json!({"url": "u", "error": {
            "status": 429, "error": "Too many requests", "retry_after_seconds": 3,
        }}));
        assert_eq!(limited.retry_after, Some(3));
        assert_eq!(limited.exit_code(), crate::api::exit_code::RATE_LIMITED);

        assert!(!is_page_error(&json!({"url": "u", "content": "c"})));
    }

    #[test]
    fn the_most_common_page_error_picks_the_exit_code() {
        let err = |status: u16| ApiError {
            status,
            ..Default::default()
        };
        use crate::api::exit_code::{ERROR, RATE_LIMITED, TARGET};
        assert_eq!(dominant_exit_code(&[err(404), err(429), err(404)]), TARGET);
        assert_eq!(dominant_exit_code(&[err(429), err(404)]), RATE_LIMITED);
        assert_eq!(
            dominant_exit_code(&[err(404), err(429), err(429)]),
            RATE_LIMITED
        );
        assert_eq!(dominant_exit_code(&[]), ERROR);
    }

    #[test]
    fn batch_credits_show_the_lowest_balance() {
        let pages = [
            json!({"usage": {"credits_remaining": 9}}),
            json!({"url": "u", "error": {"status": 404}}),
            json!({"usage": {"credits_remaining": 7}}),
        ];
        assert_eq!(
            lowest_credits_line(&pages).as_deref(),
            Some("Credits left: 7 (https://keenable.ai/console)")
        );
        assert_eq!(lowest_credits_line(&[json!({"url": "u"})]), None);
    }

    #[test]
    fn normalize_page_cleans_a_fetched_page() {
        let mut page = json!({
            "url": "u",
            "description": "d",
            "published_at": 1792695600,
            "content": format!("x{NOTE}"),
        });
        normalize_page(&mut page);
        assert!(page.get("description").is_none());
        assert_eq!(page["published_at"], "2026-10-22T19:00:00Z");
        assert_eq!(page["truncated"], true);
    }
}
