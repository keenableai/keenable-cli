use reqwest::Client;
use serde_json::Value;

use crate::constants::API_BASE_URL;

/// Structured API error matching the backend's `{error, message, retryAfter}` format.
pub struct ApiError {
    pub status: u16,
    pub error: String,
    pub message: Option<String>,
    pub retry_after: Option<u64>,
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
        Value::Object(map)
    }

    pub fn is_rate_limit(&self) -> bool {
        self.status == 429
    }

    pub fn is_auth_error(&self) -> bool {
        self.status == 401
            || (self.status == 400
                && self.error.to_lowercase().contains("authentication"))
    }
}

pub fn api_key_client(api_key: &str) -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    // Keys from --api-key or a hand-edited config may carry stray whitespace
    // or control chars; a bad header value must yield a 401, not a panic.
    if let Ok(value) = api_key.trim().parse() {
        headers.insert("X-API-Key", value);
    }
    Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap()
}

pub fn bare_client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap()
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
            retry_after: None,
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
    let retry_after = retry_after.or_else(|| {
        body_json
            .as_ref()
            .and_then(|v| v["retryAfter"].as_u64())
    });

    Err(ApiError {
        status: status.as_u16(),
        error,
        message,
        retry_after,
    })
}
