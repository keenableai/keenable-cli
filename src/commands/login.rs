use colored::Colorize;
use reqwest::Client;
use std::time::Duration;

use crate::api::api_url;
use crate::config;
use crate::constants::*;

fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

struct DeviceCode {
    agent_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

async fn request_code(label: &str) -> Result<DeviceCode, String> {
    let client = Client::new();
    let resp = client
        .post(api_url("/v1/auth/agent/code"))
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "label": label,
        }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to request device code: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, body));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse code response: {}", e))?;

    Ok(DeviceCode {
        agent_code: data["agent_code"]
            .as_str()
            .ok_or("Missing agent_code")?
            .to_string(),
        user_code: data["user_code"]
            .as_str()
            .ok_or("Missing user_code")?
            .to_string(),
        verification_uri: data["verification_uri"]
            .as_str()
            .ok_or("Missing verification_uri")?
            .to_string(),
        expires_in: data["expires_in"].as_u64().unwrap_or(600),
        interval: data["interval"].as_u64().unwrap_or(5),
    })
}

async fn poll_for_token(
    agent_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<String, String> {
    let client = Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);

    loop {
        if std::time::Instant::now() > deadline {
            return Err("Code expired. Please run `keenable login` again.".to_string());
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;

        let resp = client
            .post(api_url("/v1/auth/agent/token"))
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "agent_code": agent_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:agent_code",
            }))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Polling failed: {}", e))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

        if let Some(key) = data["api_key"].as_str() {
            return Ok(key.to_string());
        }

        match data["error"].as_str() {
            Some("authorization_pending") => continue,
            Some("access_denied") => return Err("Authorization was denied.".to_string()),
            Some("expired_token") => {
                return Err("Code expired. Please run `keenable login` again.".to_string());
            }
            Some(e) => return Err(format!("Authorization error: {}", e)),
            None => continue,
        }
    }
}

pub async fn login(api_key: Option<&str>) {
    use crate::ui;

    ui::header("keenable login");

    // Fast path: --api-key provided, skip browser login
    if let Some(key) = api_key {
        config::set_api_key(key);
        ui::success("API key saved");
        ui::hint("You can now use: keenable search \"query\"");
        eprintln!();
        return;
    }

    // Step 1: Request device code
    let label = format!("{}-cli", machine_name());
    let code = match request_code(&label).await {
        Ok(c) => c,
        Err(e) => {
            ui::error(&format!("Failed to start login: {}", e));
            std::process::exit(1);
        }
    };
    ui::step_done("Requested device code");

    // Step 2: Try to open verification link in browser
    open::that(&code.verification_uri).ok();
    ui::step_done("Tried opening link in browser");
    ui::sub_hint(&format!(
        "If you are an agent or on a remote machine, open {} and enter code {}",
        "keenable.ai/link".cyan(),
        code.user_code.yellow().bold()
    ));

    // Step 3: Poll for approval
    ui::step("Waiting for approval...");
    let api_key = match poll_for_token(&code.agent_code, code.interval, code.expires_in).await {
        Ok(key) => key,
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    };
    ui::step_done_replace("Approved");

    // Step 4: Save API key
    config::set_api_key(&api_key);
    ui::success("Logged in");

    ui::hint(&format!("Next: {}", "keenable configure-mcp".cyan()));
    eprintln!();
}

pub fn logout() {
    use crate::ui;

    ui::header("keenable logout");

    config::clear_credentials();
    config::set_config_value("api_key", serde_json::Value::Null);

    ui::step_done("Cleared credentials");
    ui::step_done("Removed API key");
    ui::success("Logged out");
    eprintln!();
}
