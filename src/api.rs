use reqwest::Client;
use serde_json::Value;

use crate::constants::API_BASE_URL;
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

/// Structured API error matching the backend's `{error, message, retryAfter}` format.
/// Transport failures (no HTTP status) also carry the error's cause chain
/// and a fix suggestion.
#[derive(Default)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
    pub message: Option<String>,
    pub retry_after: Option<u64>,
    /// Fix suggestion, e.g. set SSL_CERT_FILE or HTTPS_PROXY.
    pub hint: Option<String>,
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

impl ApiError {
    /// Human-readable summary: "error: message" or just "error".
    pub fn display(&self) -> String {
        match &self.message {
            Some(msg) => format!("{}: {}", self.error, msg),
            None => self.error.clone(),
        }
    }

    /// Structured YAML output for agent consumption.
    pub fn to_yaml_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("error".into(), Value::String(self.error.clone()));
        if let Some(msg) = &self.message {
            map.insert("message".into(), Value::String(msg.clone()));
        }
        if let Some(ra) = self.retry_after {
            map.insert("retry_after".into(), Value::Number(ra.into()));
        }
        if let Some(cause) = self.cause() {
            map.insert("cause".into(), Value::String(cause));
        }
        if let Some(hint) = &self.hint {
            map.insert("hint".into(), Value::String(hint.clone()));
        }
        Value::Object(map)
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
            hint,
            chain,
            transport: true,
            timeout,
            ..Default::default()
        }
    }

    /// Print the cause and hint under an already-printed `ui::error` line.
    /// With --verbose, the full chain replaces the one-line cause.
    pub fn print_details(&self) {
        if net::verbose() && self.chain.len() > 1 {
            ui::sub_info("Error chain:");
            for (i, msg) in self.chain.iter().enumerate() {
                ui::sub_info(&format!("{}. {}", i + 1, msg));
            }
        } else if let Some(cause) = self.cause() {
            ui::sub_info(&format!("Cause: {}", cause));
        }
        if let Some(hint) = &self.hint {
            ui::hint(hint);
        }
    }

    /// The error's `source()` chain below `message`, joined with " → ".
    pub fn cause(&self) -> Option<String> {
        (self.chain.len() > 1).then(|| self.chain[1..].join(CAUSE_SEPARATOR))
    }

    pub fn is_rate_limit(&self) -> bool {
        self.status == 429
    }

    pub fn is_auth_error(&self) -> bool {
        self.status == 401
            || self.status == 403
            || (self.status == 400 && self.error.to_lowercase().contains("authentication"))
    }
}

impl From<net::CertFileError> for ApiError {
    fn from(e: net::CertFileError) -> Self {
        ApiError {
            error: "Invalid SSL_CERT_FILE".into(),
            message: Some(e.to_string()),
            hint: Some(net::CERT_FILE_HINT.into()),
            ..Default::default()
        }
    }
}

/// Exit 1 with a clear error when SSL_CERT_FILE is set but unusable. For
/// commands whose key validation would otherwise read it as a network blip,
/// carry on and exit 0.
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
    Unreachable(Option<ApiError>),
}

/// Ping the auth endpoint with a short timeout — this is a pre-flight check,
/// not worth stalling a command for the full 60s client timeout.
pub async fn validate_api_key(api_key: &str) -> KeyCheck {
    let client = match api_key_client(api_key) {
        Ok(c) => c,
        Err(e) => return KeyCheck::Unreachable(Some(e.into())),
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
        Err(e) => KeyCheck::Unreachable(Some(ApiError::from_transport(&e))),
    }
}

/// A client with the CLI's trust roots and proxy handling but no Keenable
/// headers — for non-API hosts and endpoints that need no default headers.
pub fn plain_client() -> Result<Client, net::CertFileError> {
    Ok(net::client_builder()?
        .user_agent(USER_AGENT)
        .build()
        .expect("HTTP client config is valid"))
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

pub fn api_url(path: &str) -> String {
    format!("{}{}", API_BASE_URL, path)
}

pub async fn handle_response(resp: reqwest::Response) -> Result<Value, ApiError> {
    let status = resp.status();

    if status.is_success() {
        return resp.json::<Value>().await.map_err(|e| ApiError {
            status: status.as_u16(),
            error: "Failed to parse response".into(),
            message: Some(e.to_string()),
            ..Default::default()
        });
    }

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

    // Also check retryAfter in body (backend may include it)
    let retry_after =
        retry_after.or_else(|| body_json.as_ref().and_then(|v| v["retryAfter"].as_u64()));

    Err(ApiError {
        status: status.as_u16(),
        error,
        message,
        retry_after,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_stays_under_clippy_large_err_threshold() {
        assert!(
            std::mem::size_of::<ApiError>() <= 128,
            "{}",
            std::mem::size_of::<ApiError>()
        );
    }

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
    fn rate_limit_is_429_only() {
        assert!(err(429, "Too many requests").is_rate_limit());
        assert!(!err(401, "nope").is_rate_limit());
        assert!(!err(500, "boom").is_rate_limit());
    }

    #[test]
    fn auth_error_covers_401_403_and_400_auth() {
        assert!(err(401, "unauthorized").is_auth_error());
        assert!(err(403, "forbidden").is_auth_error());
        // 400 counts only when the message names authentication (case-insensitive).
        assert!(err(400, "Authentication failed").is_auth_error());
        assert!(err(400, "AUTHENTICATION required").is_auth_error());
        assert!(!err(400, "bad query").is_auth_error());
        assert!(!err(500, "boom").is_auth_error());
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
        assert_eq!(v["hint"], "set SSL_CERT_FILE");
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
        assert!(e.hint.unwrap().contains("SSL_CERT_FILE"));
        assert!(!e.transport);
    }

    #[test]
    fn display_joins_error_and_message() {
        let mut e = err(500, "Server error");
        assert_eq!(e.display(), "Server error");
        e.message = Some("upstream timeout".into());
        assert_eq!(e.display(), "Server error: upstream timeout");
    }
}
