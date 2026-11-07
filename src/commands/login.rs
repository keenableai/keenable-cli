use colored::Colorize;
use reqwest::Client;
use std::time::Duration;

use crate::api::api_url;
use crate::config;
use crate::constants::*;
use crate::daemon;

fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const LEGACY_AGENT_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:agent_code";
const SLOW_DOWN_STEP_SECS: u64 = 5;
const EXPIRED_MESSAGE: &str = "Code expired. Please run `keenable login` again.";

struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

fn error_detail(body: &serde_json::Value) -> Option<String> {
    let reason = body["error_description"]
        .as_str()
        .or_else(|| body["message"].as_str());
    let fix = body["fix"].as_str();
    match (reason, fix) {
        (Some(r), Some(f)) => Some(format!("{} {}", r, f)),
        (Some(r), None) => Some(r.to_string()),
        (None, Some(f)) => Some(f.to_string()),
        (None, None) => None,
    }
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
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| error_detail(&v))
            .unwrap_or(body);
        return Err(format!("HTTP {}: {}", status, detail));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse code response: {}", e))?;

    Ok(DeviceCode {
        device_code: data["device_code"]
            .as_str()
            .or_else(|| data["agent_code"].as_str())
            .ok_or("Missing device_code")?
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

#[derive(Debug, PartialEq)]
enum PollStep {
    Done(String),
    Wait { interval: u64, next_wait: u64 },
    UseLegacyGrant,
    Fail(String),
}

fn grant_type_rejected(status: u16, body: &serde_json::Value) -> bool {
    status == 400
        && body["error"].as_str() == Some("Invalid parameter")
        && body["message"]
            .as_str()
            .is_some_and(|m| m.contains("grant_type"))
}

fn classify_poll(
    status: u16,
    retry_after: Option<u64>,
    body: Option<&serde_json::Value>,
    interval: u64,
) -> PollStep {
    let keep = PollStep::Wait {
        interval,
        next_wait: interval,
    };

    if status == 429 {
        let backed_off = interval + SLOW_DOWN_STEP_SECS;
        return PollStep::Wait {
            interval: backed_off,
            next_wait: retry_after.unwrap_or(backed_off).max(backed_off),
        };
    }

    if status >= 500 {
        return PollStep::Wait {
            interval,
            next_wait: retry_after.unwrap_or(interval).max(interval),
        };
    }

    let Some(body) = body else {
        return keep;
    };

    if (200..300).contains(&status)
        && let Some(key) = body["api_key"]
            .as_str()
            .or_else(|| body["access_token"].as_str())
    {
        return PollStep::Done(key.to_string());
    }

    if grant_type_rejected(status, body) {
        return PollStep::UseLegacyGrant;
    }

    let server_interval = body["interval"].as_u64().unwrap_or(0);

    match body["error"].as_str() {
        Some("authorization_pending") => {
            let interval = interval.max(server_interval);
            PollStep::Wait {
                interval,
                next_wait: interval,
            }
        }
        Some("slow_down") => {
            let interval = (interval + SLOW_DOWN_STEP_SECS).max(server_interval);
            PollStep::Wait {
                interval,
                next_wait: interval,
            }
        }
        Some("access_denied") => PollStep::Fail("Authorization was denied.".to_string()),
        Some("expired_token") => PollStep::Fail(EXPIRED_MESSAGE.to_string()),
        Some(e) => PollStep::Fail(match error_detail(body) {
            Some(detail) => format!("Authorization error: {} ({})", detail, e),
            None => format!("Authorization error: {}", e),
        }),
        None => keep,
    }
}

async fn poll_for_token(
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<String, String> {
    let client = Client::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    let mut interval = interval.max(1); // a server-sent 0 must not busy-poll
    let mut next_wait = interval;
    let mut legacy = false;

    loop {
        if std::time::Instant::now() > deadline {
            return Err(EXPIRED_MESSAGE.to_string());
        }

        tokio::time::sleep(Duration::from_secs(next_wait)).await;
        next_wait = interval;

        let payload = if legacy {
            serde_json::json!({
                "client_id": CLIENT_ID,
                "agent_code": device_code,
                "grant_type": LEGACY_AGENT_CODE_GRANT,
            })
        } else {
            serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": device_code,
                "grant_type": DEVICE_CODE_GRANT,
            })
        };

        // The user is mid-authorization in the browser; a dropped poll must
        // not abort the whole flow — keep polling until the code expires.
        let resp = match client
            .post(api_url("/v1/auth/agent/token"))
            .json(&payload)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };

        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let body: Option<serde_json::Value> = resp.json().await.ok();

        match classify_poll(status, retry_after, body.as_ref(), interval) {
            PollStep::Done(key) => return Ok(key),
            PollStep::Wait {
                interval: new_interval,
                next_wait: wait,
            } => {
                interval = new_interval.max(1);
                next_wait = wait.max(1);
            }
            // A server that predates RFC 8628 support only knows the agent_code grant.
            PollStep::UseLegacyGrant if !legacy => {
                legacy = true;
                next_wait = 0;
            }
            PollStep::UseLegacyGrant => {
                return Err(
                    "Authorization error: the server rejected the login request.".to_string(),
                );
            }
            PollStep::Fail(message) => return Err(message),
        }
    }
}

pub async fn login(api_key: Option<&str>) {
    use crate::ui;

    ui::header("keenable login");

    // Fast path: --api-key provided, skip browser login
    if let Some(key) = api_key {
        // Catch a typo'd key now rather than at first search; an unreachable
        // server must not block saving a possibly-good key.
        match crate::api::validate_api_key(key.trim()).await {
            crate::api::KeyCheck::Invalid => {
                ui::error("API key is invalid — not saved");
                ui::hint("Check the key, or run keenable login for the browser flow");
                eprintln!();
                std::process::exit(1);
            }
            crate::api::KeyCheck::Unreachable => {
                ui::warning("Could not verify API key (network error) — saving anyway");
            }
            crate::api::KeyCheck::Valid => {}
        }
        config::set_api_key(key);
        daemon::kill_daemon();
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
    let step_lines = ui::step("Waiting for approval...");
    let api_key = match poll_for_token(&code.device_code, code.interval, code.expires_in).await {
        Ok(key) => key,
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    };
    ui::step_done_replace("Approved", step_lines);

    // Step 4: Save API key and restart daemon with new credentials
    config::set_api_key(&api_key);
    daemon::kill_daemon();
    ui::success("Logged in");

    ui::hint(&format!("Next: {}", "keenable configure-mcp".cyan()));
    eprintln!();
}

pub fn logout() {
    use crate::ui;

    ui::header("keenable logout");

    config::clear_credentials();
    config::remove_config_value("api_key");
    daemon::kill_daemon();

    ui::step_done("Cleared credentials");
    ui::step_done("Removed API key");
    ui::success("Logged out");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wait(interval: u64, next_wait: u64) -> PollStep {
        PollStep::Wait {
            interval,
            next_wait,
        }
    }

    #[test]
    fn rfc_pending_is_400_and_keeps_polling() {
        let body = json!({"error": "authorization_pending", "interval": 5});
        assert_eq!(classify_poll(400, None, Some(&body), 5), wait(5, 5));
    }

    #[test]
    fn legacy_pending_is_200_and_keeps_polling() {
        let body = json!({"error": "authorization_pending"});
        assert_eq!(classify_poll(200, None, Some(&body), 5), wait(5, 5));
    }

    #[test]
    fn slow_down_adds_five_seconds_or_takes_the_server_interval() {
        let body = json!({"error": "slow_down"});
        assert_eq!(classify_poll(400, None, Some(&body), 5), wait(10, 10));
        let body = json!({"error": "slow_down", "interval": 20});
        assert_eq!(classify_poll(400, None, Some(&body), 5), wait(20, 20));
    }

    #[test]
    fn rate_limit_backs_off_and_respects_retry_after() {
        let body = json!({"error": "Too many requests. Please try again later.", "retryAfter": 30});
        assert_eq!(classify_poll(429, Some(30), Some(&body), 5), wait(10, 30));
        assert_eq!(classify_poll(429, None, Some(&body), 5), wait(10, 10));
        assert_eq!(classify_poll(429, Some(2), None, 5), wait(10, 10));
    }

    #[test]
    fn server_errors_wait_for_retry_after_and_keep_polling() {
        let body = json!({"error": "temporarily_unavailable", "retry_after_seconds": 3});
        assert_eq!(classify_poll(503, Some(3), Some(&body), 5), wait(5, 5));
        assert_eq!(classify_poll(503, Some(12), Some(&body), 5), wait(5, 12));
    }

    #[test]
    fn success_returns_the_key_from_either_shape() {
        assert_eq!(
            classify_poll(200, None, Some(&json!({"api_key": "keen_a"})), 5),
            PollStep::Done("keen_a".into())
        );
        assert_eq!(
            classify_poll(
                200,
                None,
                Some(&json!({"access_token": "keen_b", "token_type": "Bearer"})),
                5
            ),
            PollStep::Done("keen_b".into())
        );
    }

    #[test]
    fn terminal_errors_fail_on_both_status_codes() {
        for status in [200, 400] {
            assert_eq!(
                classify_poll(status, None, Some(&json!({"error": "expired_token"})), 5),
                PollStep::Fail(EXPIRED_MESSAGE.into())
            );
            assert_eq!(
                classify_poll(status, None, Some(&json!({"error": "access_denied"})), 5),
                PollStep::Fail("Authorization was denied.".into())
            );
        }
    }

    #[test]
    fn unknown_errors_show_the_server_reason_and_fix() {
        let body = json!({
            "error": "invalid_grant",
            "error_description": "This device_code was issued to a different client_id.",
            "fix": "Poll with the same client_id."
        });
        assert_eq!(
            classify_poll(400, None, Some(&body), 5),
            PollStep::Fail(
                "Authorization error: This device_code was issued to a different client_id. Poll with the same client_id. (invalid_grant)".into()
            )
        );
    }

    #[test]
    fn an_older_server_rejecting_the_standard_grant_switches_to_legacy() {
        let body = json!({
            "error": "Invalid parameter",
            "message": "\"grant_type\" must be \"urn:ietf:params:oauth:grant-type:agent_code\""
        });
        assert_eq!(
            classify_poll(400, None, Some(&body), 5),
            PollStep::UseLegacyGrant
        );
    }

    #[test]
    fn unparseable_bodies_keep_polling() {
        assert_eq!(classify_poll(200, None, None, 5), wait(5, 5));
        assert_eq!(classify_poll(502, None, None, 5), wait(5, 5));
    }
}
