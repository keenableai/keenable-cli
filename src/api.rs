use reqwest::Client;
use serde_json::Value;

use crate::constants::API_BASE_URL;

pub fn api_key_client(api_key: &str) -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("X-API-Key", api_key.parse().unwrap());
    Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap()
}

pub fn bearer_client(token: &str) -> Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {}", token).parse().unwrap(),
    );
    Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap()
}

pub fn api_url(path: &str) -> String {
    format!("{}{}", API_BASE_URL, path)
}

pub async fn handle_response(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    if status == 401 {
        return Err("Authentication failed. Run `keenable login`.".to_string());
    }
    if status == 429 {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("?");
        return Err(format!("Rate limited. Retry after {}s.", retry_after));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))
}
