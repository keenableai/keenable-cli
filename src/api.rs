use reqwest::Client;
use serde_json::Value;

use std::sync::OnceLock;

use crate::constants::{API_URL_ENV, DEFAULT_API_BASE_URL};
use crate::net;
use crate::ui;

const USER_AGENT: &str = concat!("keenable-cli/", env!("CARGO_PKG_VERSION"));

/// Default `X-Keenable-Title` value. The backend requires this header on
/// token-less (public) endpoints and records it as `app_title` for
/// observability. Sending it always keeps the unauthenticated flow working and
/// makes first-party CLI traffic attributable in dashboards.
const DEFAULT_APP_TITLE: &str = "keenable-cli";

/// Resolve the app title from an optional env value, falling back to the
/// default. Override via `KEENABLE_APP_TITLE` to separate first-party
/// automation (the e2e suite sets `keenable-cli-e2e`) from real CLI users.
/// Pure so it can be unit-tested without touching the process environment.
fn resolve_app_title(env_value: Option<String>) -> String {
    env_value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_APP_TITLE.to_string())
}

fn app_title() -> String {
    resolve_app_title(std::env::var("KEENABLE_APP_TITLE").ok())
}

/// Headers common to every keenable API client. Carries `X-Keenable-Title`,
/// which is mandatory on public endpoints. A non-parseable override is dropped
/// rather than panicking — the default is always a valid header value.
fn base_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = app_title().parse() {
        headers.insert("X-Keenable-Title", value);
    }
    headers
}

/// Where users add credits or check what their plan includes.
pub const CONSOLE_URL: &str = "https://keenable.ai/console";

/// Structured API error matching the backend's flat error body: the legacy
/// `{error, message, retryAfter}` plus the v1 fields (`type`, `code`, `fix`,
/// `docs_url`), which older servers do not send.
/// Transport failures (no HTTP status) also carry the error's cause chain
/// and a fix suggestion.
#[derive(Default)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
    /// v1 `type`: the closed category (`authentication_error`,
    /// `rate_limit_error`, `target_error`, ...).
    pub error_type: Option<String>,
    /// v1 `code`: stable snake_case id, e.g. `invalid_api_key`.
    pub code: Option<String>,
    pub message: Option<String>,
    /// v1 `fix`: the server's next step, worded for raw HTTP callers. Shown
    /// as `hint` only where the CLI has no hint of its own.
    pub fix: Option<String>,
    /// v1 `docs_url`: the docs entry for `code`.
    pub docs_url: Option<String>,
    pub retry_after: Option<u64>,
    /// `X-RateLimit-Reset` on a 429: when the window reopens (ISO 8601). The
    /// API sends this rather than `Retry-After`, so it is the only retry time
    /// most 429s carry.
    pub rate_limit_reset: Option<String>,
    /// The API's `X-Request-Id` for the failed call; users quote it to support.
    pub request_id: Option<String>,
    /// Transport or local-setup fix, e.g. set SSL_CERT_FILE or HTTPS_PROXY.
    /// `hint()` returns it for errors with no HTTP status.
    pub transport_hint: Option<String>,
    /// Every message in the chain, outermost first (shown with --verbose).
    pub chain: Vec<String>,
    /// The request failed before any HTTP response arrived. Read only by the
    /// Unix daemon (and tests) when forwarding errors.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub transport: bool,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub timeout: bool,
}

pub const CAUSE_SEPARATOR: &str = " → ";

/// Exit codes for a failed `search` / `fetch`, so scripts can tell failures
/// apart without parsing output. 2 is clap's usage error; 9 is a Keenable-side
/// server error worth retrying later; anything without a more specific code
/// (local setup errors) exits 1.
pub mod exit_code {
    pub const ERROR: i32 = 1;
    pub const AUTH: i32 = 3;
    pub const RATE_LIMITED: i32 = 4;
    pub const OUT_OF_CREDITS: i32 = 5;
    pub const TARGET: i32 = 6;
    pub const NETWORK: i32 = 7;
    pub const INVALID_REQUEST: i32 = 8;
    pub const SERVER: i32 = 9;
}

impl ApiError {
    /// The process exit code for this error (see `exit_code`). The v1 `type`
    /// decides when the server sent one: it is more precise than the status
    /// (a `target_error` 403 is the fetched site's refusal, not the key's;
    /// a 404 `invalid_request_error` is a wrong path, not a missing page).
    /// Older servers send no type, so the status decides for them.
    pub fn exit_code(&self) -> i32 {
        match self.error_type.as_deref() {
            Some("target_error") => return exit_code::TARGET,
            Some("rate_limit_error") => return exit_code::RATE_LIMITED,
            // Only exhausted credits: payment_required and
            // payment_settlement_failed (x402) aren't fixed by adding credits.
            Some("billing_error") if self.code.as_deref() == Some("insufficient_credits") => {
                return exit_code::OUT_OF_CREDITS;
            }
            Some("billing_error") => return exit_code::ERROR,
            Some("authentication_error") => return exit_code::AUTH,
            Some("permission_error") if self.code.as_deref() == Some("private_url_not_allowed") => {
                return exit_code::INVALID_REQUEST;
            }
            Some("permission_error") => return exit_code::AUTH,
            Some("invalid_request_error") => return exit_code::INVALID_REQUEST,
            Some("api_error") => return exit_code::SERVER,
            _ => {}
        }
        if self.is_auth_error() {
            return exit_code::AUTH;
        }
        match self.status {
            0 if self.transport => exit_code::NETWORK,
            403 => exit_code::AUTH,
            402 => exit_code::OUT_OF_CREDITS,
            404 | 410 => exit_code::TARGET,
            429 => exit_code::RATE_LIMITED,
            400 | 422 => exit_code::INVALID_REQUEST,
            500..=599 => exit_code::SERVER,
            _ => exit_code::ERROR,
        }
    }

    /// Human-readable summary: "error: message" or just "error".
    pub fn display(&self) -> String {
        match &self.message {
            Some(msg) => format!("{}: {}", self.error, msg),
            None => self.error.clone(),
        }
    }

    /// Read the v1 fields (`type`, `code`, `fix`, `docs_url`) from an API
    /// error body, or from a daemon-forwarded `to_yaml_value`, which uses the
    /// same names. Missing or blank fields (older servers) stay None.
    pub fn with_v1_fields(mut self, body: &Value) -> Self {
        let field = |key: &str| {
            body[key]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        self.error_type = field("type");
        self.code = field("code");
        self.fix = field("fix");
        self.docs_url = field("docs_url");
        self
    }

    /// Structured YAML output for agent consumption. The v1 fields are only
    /// present when the server sent them.
    pub fn to_yaml_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("error".into(), Value::String(self.error.clone()));
        if let Some(t) = &self.error_type {
            map.insert("type".into(), Value::String(t.clone()));
        }
        if let Some(code) = &self.code {
            map.insert("code".into(), Value::String(code.clone()));
        }
        if let Some(msg) = &self.message {
            map.insert("message".into(), Value::String(msg.clone()));
        }
        if let Some(fix) = &self.fix {
            map.insert("fix".into(), Value::String(fix.clone()));
        }
        if let Some(ra) = self.retry_after {
            map.insert("retry_after".into(), Value::Number(ra.into()));
        }
        if let Some(reset) = &self.rate_limit_reset {
            map.insert("rate_limit_reset".into(), Value::String(reset.clone()));
        }
        if let Some(url) = &self.docs_url {
            map.insert("docs_url".into(), Value::String(url.clone()));
        }
        if let Some(id) = &self.request_id {
            map.insert("request_id".into(), Value::String(id.clone()));
        }
        if let Some(cause) = self.cause() {
            map.insert("cause".into(), Value::String(cause));
        }
        Value::Object(map)
    }

    /// "Retry after 30s" from `Retry-After`, else "Retry after <time>" from
    /// `X-RateLimit-Reset`; None when the API sent neither.
    pub fn retry_phrase(&self) -> Option<String> {
        self.retry_after
            .map(|s| format!("Retry after {}s", s))
            .or_else(|| {
                self.rate_limit_reset
                    .as_ref()
                    .map(|t| format!("Retry after {}", t))
            })
    }

    /// Transport-level failure (no HTTP status): keep the whole cause chain.
    pub fn from_transport(err: &reqwest::Error) -> Self {
        let failure = net::describe(err);
        Self::from_chain(failure.chain, failure.hint, failure.timeout)
    }

    fn from_chain(chain: Vec<String>, hint: Option<String>, timeout: bool) -> Self {
        ApiError {
            error: "Request failed".into(),
            message: chain.first().cloned(),
            transport_hint: hint,
            chain,
            transport: true,
            timeout,
            ..Default::default()
        }
    }

    /// Print the cause and the transport hint under an already-printed
    /// `ui::error` line, for callers without an auth-aware hint (login,
    /// configure-mcp). Search and fetch print `print_cause` plus `hint()`.
    pub fn print_details(&self) {
        self.print_cause();
        if let Some(hint) = &self.transport_hint {
            ui::hint(hint);
        }
    }

    /// Print the cause under an already-printed `ui::error` line. With
    /// --verbose, the full chain replaces the one-line cause.
    pub fn print_cause(&self) {
        if net::verbose() && self.chain.len() > 1 {
            ui::sub_info("Error chain:");
            for (i, msg) in self.chain.iter().enumerate() {
                ui::sub_info(&format!("{}. {}", i + 1, msg));
            }
        } else if let Some(cause) = self.cause() {
            ui::sub_info(&format!("Cause: {}", cause));
        }
    }

    /// The error's `source()` chain below `message`, joined with " → ".
    pub fn cause(&self) -> Option<String> {
        (self.chain.len() > 1).then(|| self.chain[1..].join(CAUSE_SEPARATOR))
    }

    /// The credentials themselves were rejected (401, or a 400 the server
    /// labels as an authentication failure). Logging in is the fix.
    pub fn is_auth_error(&self) -> bool {
        self.status == 401
            || (self.status == 400 && self.error.to_lowercase().contains("authentication"))
    }

    /// The single next step for this error, worded for the caller's auth
    /// state. The CLI's own hint stays where it has one: agents match on its
    /// wording (`keenable login`, "retrying unchanged fails", the retry time),
    /// and the server's `fix` talks about HTTP headers the CLI does not
    /// expose. The server `fix` fills in for statuses the CLI has no hint
    /// for, replaces the CLI hint where the status alone points at the wrong
    /// cause (`status_hint_misleads`), and is always in YAML as its own `fix`
    /// field.
    /// `keenable login` is suggested only where it can help: a rejected key,
    /// a feature that needs an account, or the keyless rate limit. Commands
    /// are wrapped in backticks so pretty mode can color them.
    /// A failure with no HTTP status (network, proxy, certificates) gets
    /// its `transport_hint`.
    pub fn hint(&self, authenticated: bool) -> Option<String> {
        if self.transport_hint.is_some() {
            return self.transport_hint.clone();
        }
        if let Some(cli) = self.cli_hint(authenticated) {
            return Some(cli);
        }
        let fix = self.fix.clone()?;
        // A 429 fix says "wait for Retry-After"; lead with the actual time,
        // as the CLI's own 429 hint does.
        match self.retry_phrase() {
            Some(retry) if self.status == 429 => Some(format!("{}. {}", retry, fix)),
            _ => Some(fix),
        }
    }

    /// Codes whose cause is not what the CLI's status-based hint assumes:
    /// a fetch target refusing or failing (not the key, plan or request), a
    /// private URL (not the key), a pay-per-request payment (not account
    /// credits), or a rate limit on Keenable's side (not the caller's key).
    /// For these the server's `fix` is the right next step.
    fn status_hint_misleads(&self) -> bool {
        self.error_type.as_deref() == Some("target_error")
            || matches!(
                self.code.as_deref(),
                Some(
                    "private_url_not_allowed"
                        | "payment_required"
                        | "payment_settlement_failed"
                        | "search_rate_limited"
                        | "fetch_rate_limited"
                )
            )
    }

    fn cli_hint(&self, authenticated: bool) -> Option<String> {
        if self.fix.is_some() && self.status_hint_misleads() {
            return None;
        }
        if self.is_auth_error() {
            return Some("Run `keenable login` to authenticate.".into());
        }
        match self.status {
            400 | 422 => Some(
                "Fix the request (the message says what was rejected); retrying unchanged fails the same way."
                    .into(),
            ),
            402 => Some(format!(
                "Out of credits. Add credits at {}; retrying before that fails the same way.",
                CONSOLE_URL
            )),
            // Anonymous: the feature needs an account. Authenticated: either
            // the plan doesn't cover this or the key was revoked (the key
            // check treats 403 as a rejected key), so name both fixes — the
            // server message says what was refused.
            403 if authenticated => Some(format!(
                "Your API key or plan does not allow this. If the key was revoked, run `keenable login`; otherwise check your plan at {}.",
                CONSOLE_URL
            )),
            403 => Some("This needs an account. Run `keenable login` to authenticate.".into()),
            // Login can't raise limits for a key that's already in use, so an
            // authenticated 429 suggests retrying or switching accounts.
            429 if authenticated => {
                let retry = self
                    .retry_phrase()
                    .unwrap_or_else(|| "Retry later".to_string());
                Some(format!(
                    "Rate limit reached for your API key. {}, or run `keenable login` to switch accounts.",
                    retry
                ))
            }
            429 => Some(match self.retry_phrase() {
                Some(retry) => format!(
                    "{}, or run `keenable login` to authenticate and increase your limits.",
                    retry
                ),
                None => "Run `keenable login` to authenticate and increase your limits.".into(),
            }),
            _ => None,
        }
    }
}

impl From<net::CertFileError> for ApiError {
    fn from(e: net::CertFileError) -> Self {
        ApiError {
            error: "Invalid SSL_CERT_FILE".into(),
            message: Some(e.to_string()),
            transport_hint: Some(net::CERT_FILE_HINT.into()),
            ..Default::default()
        }
    }
}

/// Exit 1 with a clear error when SSL_CERT_FILE is set but unusable. Called
/// before key validation, which would otherwise treat a bad file as a network
/// blip and let the command succeed.
pub fn exit_on_bad_cert_file() {
    if let Err(e) = net::check_cert_file() {
        let err = ApiError::from(e);
        ui::error(&err.display());
        err.print_details();
        eprintln!();
        std::process::exit(1);
    }
}

pub enum KeyCheck {
    Valid,
    /// The server rejected the key (401/403).
    Invalid,
    /// Network failure or server error — validity unknown. Carries the
    /// failure when there is one worth showing.
    Unreachable(Option<Box<ApiError>>),
}

/// Ping the auth endpoint with a short timeout — this is a pre-flight check,
/// not worth stalling a command for the full 60s client timeout.
pub async fn validate_api_key(api_key: &str) -> KeyCheck {
    let client = match api_key_client(api_key) {
        Ok(c) => c,
        Err(e) => return KeyCheck::Unreachable(Some(Box::new(e.into()))),
    };
    let resp = client
        .get(api_url("/v1/auth/user"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
    match resp {
        Ok(resp) if resp.status().is_success() => KeyCheck::Valid,
        Ok(resp) if resp.status() == 401 || resp.status() == 403 => KeyCheck::Invalid,
        Ok(_) => KeyCheck::Unreachable(None),
        Err(e) => KeyCheck::Unreachable(Some(Box::new(ApiError::from_transport(&e)))),
    }
}

pub fn api_key_client(api_key: &str) -> Result<Client, net::CertFileError> {
    let mut headers = base_headers();
    // Keys from --api-key or a hand-edited config may carry stray whitespace
    // or control chars; a bad header value must yield a 401, not a panic.
    if let Ok(value) = api_key.trim().parse() {
        headers.insert("X-API-Key", value);
    }
    Ok(net::client_builder()?
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("HTTP client config is valid"))
}

pub fn bare_client() -> Result<Client, net::CertFileError> {
    Ok(net::client_builder()?
        .user_agent(USER_AGENT)
        .default_headers(base_headers())
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("HTTP client config is valid"))
}

/// Resolve the API base URL from an optional `KEENABLE_API_URL` value.
/// Unset or blank means the production API. https only, except plain http to
/// localhost or a loopback IP (any spelling: `127.1`, `[0:0::1]`); a host is
/// required, a query or fragment is not allowed, and trailing slashes are
/// dropped. A path prefix (`https://gw.example/keenable`) is kept. The SDKs
/// read the same variable with the same core rule, but accept only the exact
/// strings `127.0.0.1` / `::1`, and the Python SDK allows a query. Pure for
/// unit tests.
fn resolve_api_base(env_value: Option<&str>) -> Result<String, String> {
    let raw = match env_value.map(str::trim) {
        Some(v) if !v.is_empty() => v.trim_end_matches('/'),
        _ => return Ok(DEFAULT_API_BASE_URL.to_string()),
    };
    let invalid = |why: &str| {
        format!(
            "{API_URL_ENV} must be an https:// URL{why} (http is allowed only for localhost, 127.0.0.1 and ::1), got {raw:?}"
        )
    };
    let url = reqwest::Url::parse(raw).map_err(|_| invalid(" with a host"))?;
    // host_str() brackets IPv6 literals ("[::1]"); compare parsed addresses
    // so every spelling of the loopback address (`[0:0::1]`, `127.1`) counts.
    let local = match url.host_str() {
        None | Some("") => return Err(invalid(" with a host")),
        Some(host) => match host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
        {
            Ok(ip) => ip == std::net::Ipv4Addr::LOCALHOST || ip == std::net::Ipv6Addr::LOCALHOST,
            Err(_) => host == "localhost",
        },
    };
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(" with no query or fragment"));
    }
    match url.scheme() {
        "https" => Ok(raw.to_string()),
        "http" if local => Ok(raw.to_string()),
        _ => Err(invalid("")),
    }
}

/// The API base URL for this process: `KEENABLE_API_URL` or production.
/// `main` validates it before any API command runs, so callers can rely on
/// `api_base()`; the daemon reports it so a client never talks to a daemon
/// started against a different base.
pub fn resolved_api_base() -> Result<&'static str, &'static str> {
    static BASE: OnceLock<Result<String, String>> = OnceLock::new();
    BASE.get_or_init(|| resolve_api_base(std::env::var(API_URL_ENV).ok().as_deref()))
        .as_ref()
        .map(String::as_str)
        .map_err(String::as_str)
}

pub fn api_base() -> &'static str {
    match resolved_api_base() {
        Ok(base) => base,
        Err(msg) => exit_invalid_api_base(msg, false),
    }
}

/// Report an invalid `KEENABLE_API_URL` with the fix and exit 1: as a
/// structured YAML error on stdout when `yaml` (agent-facing commands),
/// otherwise as human text on stderr.
pub fn exit_invalid_api_base(msg: &str, yaml: bool) -> ! {
    let hint = format!("Fix {API_URL_ENV} or unset it to use {DEFAULT_API_BASE_URL}.");
    if yaml {
        let out = serde_json::json!({
            "error": format!("Invalid {API_URL_ENV}"),
            "message": msg,
            "hint": hint,
        });
        // Like API errors in YAML mode: the structured error on stdout and
        // a one-line summary on stderr.
        print!("{}", serde_yaml::to_string(&out).unwrap_or_default());
        crate::ui::error_line(msg);
    } else {
        crate::ui::error(msg);
        crate::ui::hint(&hint);
    }
    std::process::exit(1);
}

pub fn api_url(path: &str) -> String {
    format!("{}{}", api_base(), path)
}

/// The error is boxed: it is only built on the error path, and keeps
/// `Result` small as ApiError grows.
pub async fn handle_response(resp: reqwest::Response) -> Result<Value, Box<ApiError>> {
    let status = resp.status();
    let request_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    if status.is_success() {
        return resp.json::<Value>().await.map_err(|e| {
            Box::new(ApiError {
                status: status.as_u16(),
                error: "Failed to parse response".into(),
                message: Some(e.to_string()),
                request_id,
                ..Default::default()
            })
        });
    }

    // The API sends X-RateLimit-* on other statuses too (e.g. 400s); it is
    // only a retry time on a 429.
    let rate_limit_reset = resp
        .headers()
        .get("x-ratelimit-reset")
        .filter(|_| status.as_u16() == 429)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    // Extract retry-after header before consuming body
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    // Parse structured error body
    let body_text = resp.text().await.unwrap_or_default();
    let body_json: Option<Value> = serde_json::from_str(&body_text).ok();

    let error = body_json
        .as_ref()
        .and_then(|v| v["error"].as_str())
        .unwrap_or("Request failed")
        .to_string();

    let message = body_json
        .as_ref()
        .and_then(|v| v["message"].as_str())
        .map(|s| s.to_string());

    // Also check the body: v1 `retry_after_seconds`, else the deprecated
    // `retryAfter` alias.
    let retry_after = retry_after.or_else(|| {
        body_json.as_ref().and_then(|v| {
            v["retry_after_seconds"]
                .as_u64()
                .or_else(|| v["retryAfter"].as_u64())
        })
    });

    let err = ApiError {
        status: status.as_u16(),
        error,
        message,
        retry_after,
        rate_limit_reset,
        request_id,
        ..Default::default()
    };
    Err(Box::new(match &body_json {
        Some(body) => err.with_v1_fields(body),
        None => err,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cause_joins_the_chain_below_the_message() {
        let e = ApiError {
            chain: vec!["top".into(), "a".into(), "b".into()],
            ..Default::default()
        };
        assert_eq!(e.cause().as_deref(), Some("a → b"));
        assert_eq!(ApiError::default().cause(), None);
    }

    fn err(status: u16, error: &str) -> ApiError {
        ApiError {
            status,
            error: error.into(),
            ..Default::default()
        }
    }

    #[test]
    fn auth_error_covers_401_and_400_auth() {
        assert!(err(401, "unauthorized").is_auth_error());
        // 400 counts only when the message names authentication (case-insensitive).
        assert!(err(400, "Authentication failed").is_auth_error());
        assert!(err(400, "AUTHENTICATION required").is_auth_error());
        assert!(!err(400, "bad query").is_auth_error());
        assert!(!err(500, "boom").is_auth_error());
        // 403 is "forbidden", not "who are you": a valid key can get it too.
        assert!(!err(403, "Forbidden").is_auth_error());
    }

    #[test]
    fn exit_code_tells_failures_apart() {
        assert_eq!(err(401, "unauthorized").exit_code(), exit_code::AUTH);
        assert_eq!(
            err(400, "Authentication failed").exit_code(),
            exit_code::AUTH
        );
        assert_eq!(err(403, "Forbidden").exit_code(), exit_code::AUTH);
        assert_eq!(
            err(402, "payment_required").exit_code(),
            exit_code::OUT_OF_CREDITS
        );
        assert_eq!(
            err(429, "Rate limit exceeded").exit_code(),
            exit_code::RATE_LIMITED
        );
        assert_eq!(err(404, "Not found").exit_code(), exit_code::TARGET);
        assert_eq!(
            err(400, "bad query").exit_code(),
            exit_code::INVALID_REQUEST
        );
        assert_eq!(
            err(422, "Invalid request").exit_code(),
            exit_code::INVALID_REQUEST
        );
        assert_eq!(err(500, "boom").exit_code(), exit_code::SERVER);
        assert_eq!(err(503, "busy").exit_code(), exit_code::SERVER);
        assert_eq!(err(418, "teapot").exit_code(), exit_code::ERROR);
        // No HTTP status: a transport failure is a network error, a local
        // setup error (bad SSL_CERT_FILE) is not.
        let transport = ApiError {
            transport: true,
            ..Default::default()
        };
        assert_eq!(transport.exit_code(), exit_code::NETWORK);
        assert_eq!(ApiError::default().exit_code(), exit_code::ERROR);
        // A failure of the page being fetched is a target error whatever its
        // status: the site's 403 is not the key's fault.
        let target = ApiError {
            status: 403,
            error_type: Some("target_error".into()),
            ..Default::default()
        };
        assert_eq!(target.exit_code(), exit_code::TARGET);
        let wrong_path = ApiError {
            status: 404,
            error_type: Some("invalid_request_error".into()),
            ..Default::default()
        };
        assert_eq!(wrong_path.exit_code(), exit_code::INVALID_REQUEST);
    }

    fn hint(status: u16, error: &str, authenticated: bool) -> String {
        err(status, error).hint(authenticated).unwrap_or_default()
    }

    #[test]
    fn hint_401_suggests_login_in_both_auth_states() {
        for authed in [true, false] {
            let h = hint(401, "Authentication failed", authed);
            assert!(h.contains("keenable login"), "{h}");
            let h = hint(400, "Authentication failed", authed);
            assert!(h.contains("keenable login"), "{h}");
        }
    }

    #[test]
    fn hint_403_authenticated_covers_plan_and_revoked_key() {
        let h = hint(403, "Forbidden", true);
        assert!(h.contains("plan"), "{h}");
        assert!(h.contains(CONSOLE_URL), "{h}");
        // A revoked key also gets 403, so login is offered for that case.
        assert!(h.contains("revoked, run `keenable login`"), "{h}");
        assert_ne!(h, hint(403, "Forbidden", false));
    }

    #[test]
    fn hint_403_anonymous_suggests_login() {
        let h = hint(403, "Forbidden", false);
        assert!(h.contains("keenable login"), "{h}");
    }

    #[test]
    fn hint_402_points_at_credits_in_both_auth_states() {
        for authed in [true, false] {
            let h = hint(402, "Payment required", authed);
            assert!(h.contains("credits"), "{h}");
            assert!(h.contains(CONSOLE_URL), "{h}");
            assert!(!h.contains("keenable login"), "{h}");
        }
    }

    #[test]
    fn hint_400_says_fix_the_request_in_both_auth_states() {
        for authed in [true, false] {
            for status in [400, 422] {
                let h = hint(status, "Invalid parameter", authed);
                assert!(h.contains("Fix the request"), "{h}");
                assert!(h.contains("retrying unchanged"), "{h}");
                assert!(!h.contains("keenable login"), "{h}");
            }
        }
    }

    #[test]
    fn hint_429_is_auth_aware() {
        let h = hint(429, "Too many requests", false);
        assert!(h.contains("increase your limits"), "{h}");
        let mut e = err(429, "Too many requests");
        e.retry_after = Some(30);
        let h = e.hint(true).unwrap();
        assert!(h.contains("Retry after 30s"), "{h}");
        assert!(h.contains("switch accounts"), "{h}");
        assert!(hint(429, "x", true).contains("Retry later"));
    }

    #[test]
    fn no_hint_for_server_or_transport_errors() {
        for authed in [true, false] {
            assert!(err(500, "boom").hint(authed).is_none());
            assert!(err(404, "Not found").hint(authed).is_none());
            assert!(
                ApiError::from_chain(vec!["dns".into()], None, false)
                    .hint(authed)
                    .is_none()
            );
        }
    }

    #[test]
    fn yaml_keeps_existing_fields_and_adds_request_id() {
        let mut e = err(403, "Forbidden");
        e.message = Some("mode \"webql\" requires authentication".into());
        e.retry_after = Some(5);
        let v = e.to_yaml_value();
        assert_eq!(v["error"], "Forbidden");
        assert_eq!(v["retry_after"], 5);
        assert!(v.get("request_id").is_none());
        e.request_id = Some("req_123".into());
        assert_eq!(e.to_yaml_value()["request_id"], "req_123");
    }

    #[test]
    fn app_title_defaults_when_env_absent_or_blank() {
        assert_eq!(resolve_app_title(None), "keenable-cli");
        assert_eq!(resolve_app_title(Some("".into())), "keenable-cli");
        assert_eq!(resolve_app_title(Some("   ".into())), "keenable-cli");
    }

    #[test]
    fn app_title_uses_trimmed_override() {
        assert_eq!(
            resolve_app_title(Some("keenable-cli-e2e".into())),
            "keenable-cli-e2e"
        );
        assert_eq!(
            resolve_app_title(Some("  custom-app  ".into())),
            "custom-app"
        );
    }

    #[test]
    fn transport_errors_split_message_and_cause() {
        let e = ApiError::from_chain(
            vec![
                "error sending request for url (https://api.keenable.ai/v1/search)".into(),
                "client error (Connect)".into(),
                "invalid peer certificate: UnknownIssuer".into(),
            ],
            Some("set SSL_CERT_FILE".into()),
            false,
        );
        assert!(e.transport);
        assert_eq!(e.status, 0);
        assert_eq!(
            e.display(),
            "Request failed: error sending request for url (https://api.keenable.ai/v1/search)"
        );
        let v = e.to_yaml_value();
        assert_eq!(
            v["cause"],
            "client error (Connect) → invalid peer certificate: UnknownIssuer"
        );
        // The hint is added by the caller (`hint()`), as for API errors.
        assert!(v.get("hint").is_none());
        for authed in [true, false] {
            assert_eq!(e.hint(authed).as_deref(), Some("set SSL_CERT_FILE"));
        }
    }

    #[test]
    fn single_message_transport_errors_have_no_cause() {
        let e = ApiError::from_chain(vec!["boom".into()], None, false);
        let v = e.to_yaml_value();
        assert!(v.get("cause").is_none());
        assert!(v.get("hint").is_none());
    }

    #[test]
    fn bad_cert_file_becomes_a_clear_error() {
        let e: ApiError = net::CertFileError {
            path: "/nonexistent".into(),
            reason: "No such file or directory (os error 2)".into(),
        }
        .into();
        assert_eq!(
            e.display(),
            "Invalid SSL_CERT_FILE: /nonexistent: No such file or directory (os error 2)"
        );
        assert!(e.transport_hint.unwrap().contains("SSL_CERT_FILE"));
        assert!(!e.transport);
    }

    #[test]
    fn api_base_defaults_when_env_absent_or_blank() {
        assert_eq!(resolve_api_base(None).unwrap(), "https://api.keenable.ai");
        assert_eq!(
            resolve_api_base(Some("")).unwrap(),
            "https://api.keenable.ai"
        );
        assert_eq!(
            resolve_api_base(Some("  ")).unwrap(),
            "https://api.keenable.ai"
        );
    }

    #[test]
    fn api_base_accepts_https_and_strips_trailing_slash() {
        let ok = |v: &str| resolve_api_base(Some(v)).unwrap();
        assert_eq!(
            ok("https://api-test.keenable.ai"),
            "https://api-test.keenable.ai"
        );
        assert_eq!(
            ok("https://api-test.keenable.ai/"),
            "https://api-test.keenable.ai"
        );
        assert_eq!(
            ok(" https://gw.example:8443/keenable// "),
            "https://gw.example:8443/keenable"
        );
        assert_eq!(ok("HTTPS://Gw.Example"), "HTTPS://Gw.Example");
    }

    #[test]
    fn api_base_allows_http_only_for_loopback() {
        let ok = |v: &str| resolve_api_base(Some(v)).unwrap();
        assert_eq!(ok("http://localhost:8080/"), "http://localhost:8080");
        assert_eq!(ok("http://LOCALHOST"), "http://LOCALHOST");
        assert_eq!(ok("http://127.0.0.1:9000"), "http://127.0.0.1:9000");
        assert_eq!(ok("http://[::1]:9000"), "http://[::1]:9000");
        assert_eq!(ok("http://[::1]"), "http://[::1]");
        assert_eq!(
            ok("http://[0:0:0:0:0:0:0:1]:9000/"),
            "http://[0:0:0:0:0:0:0:1]:9000"
        );
        assert_eq!(ok("http://127.1:9000"), "http://127.1:9000");
        for bad in [
            "http://example.com",
            "http://127.0.0.2",
            "http://10.0.0.1:8080",
            "http://localhost.example.com",
            "http://[::2]",
        ] {
            assert!(
                resolve_api_base(Some(bad)).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn api_base_rejects_malformed_values() {
        for bad in [
            "api.keenable.ai",
            "ftp://api.keenable.ai",
            "https://",
            "file:///etc/passwd",
            "https://api.keenable.ai?x=1",
            "https://api.keenable.ai#frag",
            "not a url",
        ] {
            let err = resolve_api_base(Some(bad)).unwrap_err();
            assert!(
                err.starts_with("KEENABLE_API_URL must be an https:// URL"),
                "{bad}: {err}"
            );
            assert!(err.contains(bad.trim_end_matches('/')), "{bad}: {err}");
        }
    }

    #[test]
    fn display_joins_error_and_message() {
        let mut e = err(500, "Server error");
        assert_eq!(e.display(), "Server error");
        e.message = Some("upstream timeout".into());
        assert_eq!(e.display(), "Server error: upstream timeout");
    }

    #[test]
    fn rate_limit_reset_is_the_retry_time_when_no_retry_after() {
        let mut e = err(429, "Too many requests");
        e.rate_limit_reset = Some("2026-11-02T19:48:53.100Z".into());
        assert_eq!(
            e.retry_phrase().as_deref(),
            Some("Retry after 2026-11-02T19:48:53.100Z")
        );
        let h = e.hint(true).unwrap_or_default();
        assert!(h.contains("Retry after 2026-11-02T19:48:53.100Z"), "{h}");
        assert_eq!(
            e.to_yaml_value()["rate_limit_reset"],
            "2026-11-02T19:48:53.100Z"
        );
    }

    #[test]
    fn keyless_429_hint_carries_the_retry_time() {
        let mut e = err(429, "Too many requests");
        e.rate_limit_reset = Some("2026-11-02T19:48:53.100Z".into());
        assert_eq!(
            e.hint(false).as_deref(),
            Some(
                "Retry after 2026-11-02T19:48:53.100Z, or run `keenable login` to authenticate and increase your limits."
            )
        );
        e.rate_limit_reset = None;
        e.retry_after = Some(30);
        let h = e.hint(false).unwrap_or_default();
        assert!(
            h.starts_with("Retry after 30s, or run `keenable login`"),
            "{h}"
        );
        // No retry time: the plain login hint, as before.
        assert_eq!(
            err(429, "x").hint(false).as_deref(),
            Some("Run `keenable login` to authenticate and increase your limits.")
        );
    }

    #[test]
    fn authenticated_429_hint_with_reset_only() {
        let mut e = err(429, "Too many requests");
        e.rate_limit_reset = Some("2026-11-02T19:48:53.100Z".into());
        assert_eq!(
            e.hint(true).as_deref(),
            Some(
                "Rate limit reached for your API key. Retry after 2026-11-02T19:48:53.100Z, or run `keenable login` to switch accounts."
            )
        );
    }

    #[test]
    fn retry_after_seconds_win_over_rate_limit_reset() {
        let mut e = err(429, "Too many requests");
        e.retry_after = Some(30);
        e.rate_limit_reset = Some("2026-11-02T19:48:53.100Z".into());
        assert_eq!(e.retry_phrase().as_deref(), Some("Retry after 30s"));
    }

    #[test]
    fn no_retry_time_means_no_retry_phrase() {
        let e = err(429, "Too many requests");
        assert!(e.retry_phrase().is_none());
        assert!(e.to_yaml_value().get("rate_limit_reset").is_none());
        assert!(e.hint(true).unwrap_or_default().contains("Retry later"));
    }

    /// A 429 the way the API sends it: no Retry-After, only X-RateLimit-*.
    #[tokio::test]
    async fn handle_response_reads_x_ratelimit_reset_on_429() {
        let http_resp = reqwest::Response::from(
            ::http::Response::builder()
                .status(429)
                .header("x-ratelimit-reset", "2026-11-02T19:48:53.100Z")
                .header("x-request-id", "req-429")
                .body(r#"{"error":"Too many requests","message":"Rate limit exceeded"}"#)
                .unwrap(),
        );
        let e = handle_response(http_resp).await.unwrap_err();
        assert_eq!(e.status, 429);
        assert_eq!(e.retry_after, None);
        assert_eq!(
            e.rate_limit_reset.as_deref(),
            Some("2026-11-02T19:48:53.100Z")
        );
        assert_eq!(e.request_id.as_deref(), Some("req-429"));
    }

    /// The API sends X-RateLimit-Reset on 400s too; it must not leak into
    /// a non-429 error as a retry time.
    #[tokio::test]
    async fn handle_response_ignores_x_ratelimit_reset_on_400() {
        let http_resp = reqwest::Response::from(
            ::http::Response::builder()
                .status(400)
                .header("x-ratelimit-reset", "2026-11-02T19:48:53.100Z")
                .body(r#"{"error":"Invalid parameter","message":"\"max_results\": must be an integer between 1 and 50"}"#)
                .unwrap(),
        );
        let e = handle_response(http_resp).await.unwrap_err();
        assert_eq!(e.status, 400);
        assert_eq!(e.rate_limit_reset, None);
        assert!(e.to_yaml_value().get("rate_limit_reset").is_none());
        assert!(e.retry_phrase().is_none());
    }

    /// The backend's error catalog (`docs/error-catalog.json` in the backend
    /// repo), vendored. Refresh by copying the backend file over it.
    const ERROR_CATALOG: &str = include_str!("../tests/fixtures/error-catalog.json");

    const ERROR_TYPES: [&str; 7] = [
        "invalid_request_error",
        "authentication_error",
        "permission_error",
        "billing_error",
        "rate_limit_error",
        "target_error",
        "api_error",
    ];

    fn catalog() -> Vec<Value> {
        serde_json::from_str::<Value>(ERROR_CATALOG)
            .expect("catalog is JSON")
            .as_array()
            .expect("catalog is an array")
            .clone()
    }

    /// The error body a v1 server sends for a catalog entry.
    fn v1_body(entry: &Value) -> Value {
        let mut body = serde_json::json!({
            "error": entry["title"],
            "type": entry["type"],
            "code": entry["code"],
            "message": entry["summary"],
            "fix": entry["fix"],
            "docs_url": entry["docs_url"],
            "request_id": "req_v1",
            "retryable": entry["retryable"],
        });
        if entry["retryable"] == true {
            body["retry_after_seconds"] = serde_json::json!(2);
        }
        body
    }

    async fn respond(status: u16, body: &Value) -> ApiError {
        let http_resp = reqwest::Response::from(
            ::http::Response::builder()
                .status(status)
                .header("x-request-id", "req_v1")
                .body(body.to_string())
                .unwrap(),
        );
        *handle_response(http_resp).await.unwrap_err()
    }

    /// The exit code a script gets for each catalog code, written out by
    /// hand rather than derived the way `exit_code()` derives it, so a wrong
    /// mapping can't pass by being copied into the test. A code missing here
    /// (after refreshing the catalog) fails the test until it is added.
    const EXPECTED_EXIT_CODES: &[(&str, i32)] = &[
        ("auth_unavailable", exit_code::SERVER),
        ("content_not_extracted", exit_code::TARGET),
        ("credentials_forbidden", exit_code::AUTH),
        ("fetch_rate_limited", exit_code::RATE_LIMITED),
        ("forbidden", exit_code::AUTH),
        ("insufficient_credits", exit_code::OUT_OF_CREDITS),
        ("internal_error", exit_code::SERVER),
        ("invalid_api_key", exit_code::AUTH),
        ("invalid_bearer_token", exit_code::AUTH),
        ("invalid_parameter", exit_code::INVALID_REQUEST),
        ("invalid_request", exit_code::INVALID_REQUEST),
        ("missing_api_key", exit_code::AUTH),
        ("missing_app_title", exit_code::INVALID_REQUEST),
        ("missing_parameter", exit_code::INVALID_REQUEST),
        ("not_found", exit_code::INVALID_REQUEST),
        ("page_not_found", exit_code::TARGET),
        ("paid_rate_limit_per_second", exit_code::RATE_LIMITED),
        ("payment_facilitator_error", exit_code::SERVER),
        ("payment_required", exit_code::ERROR),
        ("payment_settlement_failed", exit_code::ERROR),
        ("payments_region_unavailable", exit_code::ERROR),
        ("payments_unavailable", exit_code::SERVER),
        ("private_url_not_allowed", exit_code::INVALID_REQUEST),
        ("public_rate_limit_hourly", exit_code::RATE_LIMITED),
        ("public_rate_limit_per_second", exit_code::RATE_LIMITED),
        ("rate_limit_per_second", exit_code::RATE_LIMITED),
        ("rate_limited", exit_code::RATE_LIMITED),
        ("request_rejected", exit_code::INVALID_REQUEST),
        ("request_too_large", exit_code::INVALID_REQUEST),
        ("search_overloaded", exit_code::SERVER),
        ("search_rate_limited", exit_code::RATE_LIMITED),
        ("target_blocked", exit_code::TARGET),
        ("target_http_error", exit_code::TARGET),
        ("target_paywalled", exit_code::TARGET),
        ("target_rate_limited", exit_code::TARGET),
        ("target_timeout", exit_code::TARGET),
        ("target_unreachable", exit_code::TARGET),
        ("target_wrong_page", exit_code::TARGET),
        ("unauthenticated", exit_code::AUTH),
        ("unknown_parameter", exit_code::INVALID_REQUEST),
        ("unprocessable", exit_code::INVALID_REQUEST),
        ("zdr_prompt_unsupported", exit_code::INVALID_REQUEST),
        ("zdr_unsupported_profile", exit_code::INVALID_REQUEST),
    ];

    #[tokio::test]
    async fn every_catalog_code_has_its_expected_exit_code() {
        for entry in catalog() {
            let code = entry["code"].as_str().unwrap();
            let expected = EXPECTED_EXIT_CODES
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, exit)| *exit)
                .unwrap_or_else(|| panic!("add {code} to EXPECTED_EXIT_CODES"));
            let status = entry["status"].as_u64().unwrap() as u16;
            let err = respond(status, &v1_body(&entry)).await;
            assert_eq!(err.exit_code(), expected, "{code}");
        }
    }

    /// Fails when the vendored catalog drifts from the shape the CLI reads.
    #[test]
    fn vendored_catalog_has_the_fields_the_cli_reads() {
        let entries = catalog();
        assert!(!entries.is_empty());
        for e in &entries {
            let code = e["code"].as_str().expect("code");
            for key in ["title", "summary", "fix", "docs_url"] {
                assert!(
                    e[key].as_str().is_some_and(|s| !s.is_empty()),
                    "{code}: {key}"
                );
            }
            assert!(
                ERROR_TYPES.contains(&e["type"].as_str().unwrap()),
                "{code}: type"
            );
            assert!(e["status"].as_u64().is_some(), "{code}: status");
            assert!(e["retryable"].is_boolean(), "{code}: retryable");
            assert_eq!(
                e["docs_url"].as_str().unwrap(),
                format!("https://docs.keenable.ai/errors#{}", code.replace('_', "-"))
            );
        }
    }

    #[tokio::test]
    async fn every_catalog_error_parses_into_v1_yaml_with_the_server_fix() {
        for entry in catalog() {
            let status = entry["status"].as_u64().unwrap() as u16;
            let e = respond(status, &v1_body(&entry)).await;
            let code = entry["code"].as_str().unwrap();
            let y = e.to_yaml_value();
            // Legacy fields exactly as before.
            assert_eq!(y["error"], entry["title"], "{code}");
            assert_eq!(y["message"], entry["summary"], "{code}");
            assert_eq!(y["request_id"], "req_v1", "{code}");
            // v1 fields, added.
            assert_eq!(y["type"], entry["type"], "{code}");
            assert_eq!(y["code"], entry["code"], "{code}");
            assert_eq!(y["fix"], entry["fix"], "{code}");
            assert_eq!(y["docs_url"], entry["docs_url"], "{code}");
            if entry["retryable"] == true {
                assert_eq!(y["retry_after"], 2, "{code}");
            }
            // The CLI hint stays where it has one (released wording, retry
            // time); the server fix fills in only where it has none.
            for authed in [true, false] {
                let h = e.hint(authed).unwrap_or_default();
                let fix = entry["fix"].as_str().unwrap();
                match e.cli_hint(authed) {
                    Some(cli) => assert_eq!(h, cli, "{code}"),
                    None if status == 429 => {
                        assert_eq!(h, format!("Retry after 2s. {fix}"), "{code}")
                    }
                    None => assert_eq!(h, fix, "{code}"),
                }
                if e.status_hint_misleads() {
                    // Target, private-URL, pay-per-request and Keenable-side
                    // rate-limit errors take the server fix: the status hint
                    // would blame the key, the plan, the request or the
                    // credit balance.
                    assert!(h.ends_with(fix), "{code}: {h}");
                    assert!(!h.contains("Out of credits"), "{code}: {h}");
                    assert!(!h.contains("API key or plan"), "{code}: {h}");
                    assert!(!h.contains("for your API key"), "{code}: {h}");
                    if status == 429 {
                        assert!(h.starts_with("Retry after 2s. "), "{code}: {h}");
                    }
                    continue;
                }
                if status == 401 || status == 429 {
                    assert!(h.contains("`keenable login`"), "{code}: {h}");
                }
                if status == 429 {
                    assert!(h.contains("Retry after 2s"), "{code}: {h}");
                }
                if status == 400 || status == 422 {
                    assert!(h.contains("retrying unchanged fails"), "{code}: {h}");
                }
            }
        }
    }

    #[tokio::test]
    async fn yaml_field_order_keeps_legacy_fields_and_adds_v1_ones() {
        let entry = catalog()
            .into_iter()
            .find(|e| e["code"] == "invalid_api_key")
            .unwrap();
        let e = respond(401, &v1_body(&entry)).await;
        let keys: Vec<String> = e
            .to_yaml_value()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            [
                "error",
                "type",
                "code",
                "message",
                "fix",
                "docs_url",
                "request_id"
            ]
        );
    }

    /// An old server sends no v1 fields: no new keys, and the CLI hint stays.
    #[tokio::test]
    async fn old_server_body_keeps_the_cli_hint_and_adds_no_v1_keys() {
        let e = respond(
            401,
            &serde_json::json!({"error": "Authentication failed", "message": "Invalid API key"}),
        )
        .await;
        let y = e.to_yaml_value();
        for key in ["type", "code", "fix", "docs_url"] {
            assert!(y.get(key).is_none(), "{key}");
        }
        assert_eq!(
            e.hint(true).as_deref(),
            Some("Run `keenable login` to authenticate.")
        );
    }

    /// Blank v1 strings count as absent, so they never blank out the hint.
    #[tokio::test]
    async fn blank_fix_falls_back_to_the_cli_hint() {
        let e = respond(
            402,
            &serde_json::json!({"error": "Insufficient credits", "code": "insufficient_credits", "fix": "  "}),
        )
        .await;
        assert!(e.fix.is_none());
        assert_eq!(e.code.as_deref(), Some("insufficient_credits"));
        assert!(e.hint(true).unwrap().contains("Out of credits"));
    }

    /// Retry-After header wins; the body's v1 `retry_after_seconds` wins
    /// over the deprecated `retryAfter`.
    #[tokio::test]
    async fn retry_after_seconds_in_body_is_read() {
        let e = respond(
            429,
            &serde_json::json!({"error": "Rate limit exceeded", "retry_after_seconds": 3, "retryAfter": 9}),
        )
        .await;
        assert_eq!(e.retry_after, Some(3));
        let e = respond(
            429,
            &serde_json::json!({"error": "Rate limit exceeded", "retryAfter": 9}),
        )
        .await;
        assert_eq!(e.retry_after, Some(9));
    }
}
