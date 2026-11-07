use reqwest::Client;
use serde_json::Value;

use crate::constants::API_BASE_URL;

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
const CONSOLE_URL: &str = "https://keenable.ai/console";

/// Structured API error matching the backend's `{error, message, retryAfter}` format.
#[derive(Default)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
    pub message: Option<String>,
    pub retry_after: Option<u64>,
    /// The API's `X-Request-Id` for the failed call; users quote it to support.
    pub request_id: Option<String>,
}

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
        if let Some(id) = &self.request_id {
            map.insert("request_id".into(), Value::String(id.clone()));
        }
        Value::Object(map)
    }

    /// Transport-level failure (no HTTP status).
    pub fn request_failed(message: String) -> Self {
        ApiError {
            status: 0,
            error: "Request failed".into(),
            message: Some(message),
            ..Default::default()
        }
    }

    pub fn is_rate_limit(&self) -> bool {
        self.status == 429
    }

    /// The credentials themselves were rejected (401, or a 400 the server
    /// labels as an authentication failure). Logging in is the fix.
    pub fn is_auth_error(&self) -> bool {
        self.status == 401
            || (self.status == 400 && self.error.to_lowercase().contains("authentication"))
    }

    /// The single next step for this error, worded for the caller's auth
    /// state. `keenable login` is suggested only where it can help: a
    /// rejected key, a feature that needs an account, or the keyless rate
    /// limit. Commands are wrapped in backticks so pretty mode can color them.
    pub fn hint(&self, authenticated: bool) -> Option<String> {
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
                    .retry_after
                    .map(|s| format!("Retry after {}s", s))
                    .unwrap_or_else(|| "Retry later".to_string());
                Some(format!(
                    "Rate limit reached for your API key. {}, or run `keenable login` to switch accounts.",
                    retry
                ))
            }
            429 => Some("Run `keenable login` to authenticate and increase your limits.".into()),
            _ => None,
        }
    }
}

pub enum KeyCheck {
    Valid,
    /// The server rejected the key (401/403).
    Invalid,
    /// Network failure or server error — validity unknown.
    Unreachable,
}

/// Ping the auth endpoint with a short timeout — this is a pre-flight check,
/// not worth stalling a command for the full 60s client timeout.
pub async fn validate_api_key(api_key: &str) -> KeyCheck {
    let client = api_key_client(api_key);
    let resp = client
        .get(api_url("/v1/auth/user"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
    match resp {
        Ok(resp) if resp.status().is_success() => KeyCheck::Valid,
        Ok(resp) if resp.status() == 401 || resp.status() == 403 => KeyCheck::Invalid,
        _ => KeyCheck::Unreachable,
    }
}

pub fn api_key_client(api_key: &str) -> Client {
    let mut headers = base_headers();
    // Keys from --api-key or a hand-edited config may carry stray whitespace
    // or control chars; a bad header value must yield a 401, not a panic.
    if let Ok(value) = api_key.trim().parse() {
        headers.insert("X-API-Key", value);
    }
    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap()
}

pub fn bare_client() -> Client {
    Client::builder()
        .user_agent(USER_AGENT)
        .default_headers(base_headers())
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap()
}

pub fn api_url(path: &str) -> String {
    format!("{}{}", API_BASE_URL, path)
}

pub async fn handle_response(resp: reqwest::Response) -> Result<Value, ApiError> {
    let status = resp.status();
    let request_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    if status.is_success() {
        return resp.json::<Value>().await.map_err(|e| ApiError {
            status: status.as_u16(),
            error: "Failed to parse response".into(),
            message: Some(e.to_string()),
            retry_after: None,
            request_id,
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
        request_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
                ApiError::request_failed("dns".into())
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
    fn display_joins_error_and_message() {
        let mut e = err(500, "Server error");
        assert_eq!(e.display(), "Server error");
        e.message = Some("upstream timeout".into());
        assert_eq!(e.display(), "Server error: upstream timeout");
    }
}
