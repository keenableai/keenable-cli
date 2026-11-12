use colored::Colorize;
use reqwest::Client;
use std::io::IsTerminal;
use std::time::Duration;

use crate::api::{ApiError, api_url};
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
const MAX_SERVER_ERROR_RETRIES: u32 = 3;
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

/// The first non-empty line of `input`, trimmed: the API key for
/// `login --api-key -`.
fn read_key(input: impl std::io::BufRead) -> Result<String, &'static str> {
    for line in input.lines() {
        let line = line.map_err(|_| "Could not read the API key from stdin")?;
        let key = line.trim();
        if !key.is_empty() {
            return Ok(key.to_string());
        }
    }
    Err("No API key on stdin")
}

/// An https link, or http to this machine (a local API under
/// `KEENABLE_API_URL`). Anything else is not opened.
fn is_web_link(link: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(link) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

fn login_error(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: msg.into(),
        ..Default::default()
    }
}

async fn request_code(client: &Client, label: &str) -> Result<DeviceCode, Box<ApiError>> {
    let resp = client
        .post(api_url("/v1/auth/agent/code"))
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "label": label,
        }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| ApiError::from_transport(&e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| error_detail(&v))
            .unwrap_or(body);
        return Err(Box::new(ApiError {
            status: status.as_u16(),
            error: format!("HTTP {}", status),
            message: Some(detail),
            ..Default::default()
        }));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| login_error(format!("Failed to parse code response: {}", e)))?;

    Ok(DeviceCode {
        device_code: data["device_code"]
            .as_str()
            .or_else(|| data["agent_code"].as_str())
            .ok_or_else(|| login_error("Missing device_code"))?
            .to_string(),
        user_code: data["user_code"]
            .as_str()
            .ok_or_else(|| login_error("Missing user_code"))?
            .to_string(),
        verification_uri: data["verification_uri"]
            .as_str()
            .ok_or_else(|| login_error("Missing verification_uri"))?
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
    if status != 400 {
        return false;
    }
    if body["error"].as_str() == Some("unsupported_grant_type")
        || body["param"].as_str() == Some("grant_type")
    {
        return true;
    }
    ["message", "error_description"].iter().any(|field| {
        body[*field]
            .as_str()
            .is_some_and(|m| m.contains("grant_type"))
    })
}

fn body_retry_after(body: &serde_json::Value) -> Option<u64> {
    body["retryAfter"]
        .as_u64()
        .or_else(|| body["retry_after_seconds"].as_u64())
}

fn unknown_error_message(error: &str, body: &serde_json::Value) -> String {
    match error_detail(body) {
        Some(detail) => format!("Authorization error: {}. {}", error, detail),
        None => format!("Authorization error: {}", error),
    }
}

fn grant_rejected_message(body: Option<&serde_json::Value>) -> String {
    let base = "Authorization error: the server rejected the login request.";
    match body.and_then(error_detail) {
        Some(detail) => format!("{} {}", base, detail),
        None => base.to_string(),
    }
}

fn server_error_failure(status: u16, body: Option<&serde_json::Value>) -> Option<String> {
    if status < 500 {
        return None;
    }
    let body = body?;
    body["error"]
        .as_str()
        .map(|error| unknown_error_message(error, body))
}

fn wait_before_poll(next_wait: u64, remaining: Duration) -> Option<Duration> {
    let wait = Duration::from_secs(next_wait);
    (wait < remaining).then_some(wait)
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
    let retry_after = retry_after.or_else(|| body.and_then(body_retry_after));

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
        Some(e) => PollStep::Fail(unknown_error_message(e, body)),
        None => keep,
    }
}

async fn poll_for_token(
    client: &Client,
    device_code: &str,
    interval: u64,
    expires_in: u64,
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    let mut interval = interval.max(1); // a server-sent 0 must not busy-poll
    let mut next_wait = interval;
    let mut legacy = false;
    let mut server_errors = 0;

    loop {
        // A server-sent Retry-After must not keep login waiting past the code's lifetime.
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match wait_before_poll(next_wait, remaining) {
            Some(wait) => tokio::time::sleep(wait).await,
            None => {
                tokio::time::sleep(remaining).await;
                return Err(EXPIRED_MESSAGE.to_string());
            }
        }
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

        // A backend that keeps failing must not hang login until the code expires.
        match server_error_failure(status, body.as_ref()) {
            Some(message) if server_errors >= MAX_SERVER_ERROR_RETRIES => return Err(message),
            Some(_) => server_errors += 1,
            None => server_errors = 0,
        }

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
            PollStep::UseLegacyGrant => return Err(grant_rejected_message(body.as_ref())),
            PollStep::Fail(message) => return Err(message),
        }
    }
}

pub async fn login(api_key: Option<&str>) {
    use crate::ui;

    ui::header("keenable login");

    // A bad SSL_CERT_FILE must not pass for "network error, saving anyway".
    crate::api::exit_on_bad_cert_file();

    // `--api-key -` reads the key from stdin, keeping it out of the process
    // list and shell history.
    let from_stdin = match api_key {
        // At a terminal, ask without echoing instead of waiting silently
        // for input that would be shown on screen.
        Some("-") if std::io::stdin().is_terminal() => {
            match dialoguer::Password::new()
                .with_prompt("   API key")
                .interact()
            {
                Ok(key) if !key.trim().is_empty() => Some(key.trim().to_string()),
                _ => {
                    ui::error("No API key entered");
                    eprintln!();
                    std::process::exit(1);
                }
            }
        }
        Some("-") => match read_key(std::io::stdin().lock()) {
            Ok(key) => Some(key),
            Err(why) => {
                ui::error(why);
                ui::hint("Pipe the key in: printenv KEENABLE_API_KEY | keenable login --api-key -");
                eprintln!();
                std::process::exit(1);
            }
        },
        _ => None,
    };
    let api_key = from_stdin.as_deref().or(api_key);

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
            crate::api::KeyCheck::Unreachable(err) => {
                ui::warning("Could not verify API key (network error) — saving anyway");
                if let Some(err) = err {
                    ui::sub_info(&err.display());
                    err.print_details();
                    eprintln!();
                }
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

    let fail = |e: ApiError| -> ! {
        ui::error(&format!("Failed to start login: {}", e.display()));
        e.print_details();
        eprintln!();
        std::process::exit(1);
    };
    let client = crate::api::bare_client().unwrap_or_else(|e| fail(e.into()));

    // Step 1: Request device code
    let label = format!("{}-cli", machine_name());
    let code = match request_code(&client, &label).await {
        Ok(c) => c,
        Err(e) => fail(*e),
    };
    ui::step_done("Requested device code");

    // Step 2: Try to open verification link in browser. The link comes from
    // the server: only a web page is handed to the OS opener, never a file
    // or an app URL scheme.
    let link = ui::one_line(&code.verification_uri);
    if !is_web_link(&link) {
        ui::error("The server sent a sign-in link that is not a web page; not using it");
        ui::hint("Save a key from https://keenable.ai/console instead: keenable login --api-key -");
        eprintln!();
        std::process::exit(1);
    }
    open::that(&link).ok();
    ui::step_done("Tried opening link in browser");
    ui::sub_hint(&format!(
        "If you are an agent or on a remote machine, open {} and enter code {}",
        link.cyan(),
        ui::one_line(&code.user_code).yellow().bold()
    ));

    // Step 3: Poll for approval
    let step_lines = ui::step("Waiting for approval...");
    let api_key = match poll_for_token(&client, &code.device_code, code.interval, code.expires_in)
        .await
    {
        Ok(key) => key,
        Err(e) => {
            ui::error(&ui::printable(&e));
            ui::hint(
                "Run keenable login again, or save a key from https://keenable.ai/console with: keenable login --api-key -",
            );
            eprintln!();
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
    config::remove_api_key();
    daemon::kill_daemon();

    ui::step_done("Cleared credentials");
    ui::step_done("Removed API key");
    ui::success("Logged out");
    // configure-mcp copied the key into these clients' own config files;
    // logging out of the CLI does not reach them.
    let holding = crate::commands::mcp_common::clients_holding_api_key();
    if !holding.is_empty() {
        ui::warning(&format!(
            "An API key is still stored in the MCP config of: {}",
            holding.join(", ")
        ));
        ui::hint(&format!(
            "Remove it there too with: {}",
            "keenable reset --all".cyan()
        ));
    }
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_key_takes_the_first_non_empty_line() {
        assert_eq!(
            read_key(&b"\n  keen_abc \r\nignored\n"[..]),
            Ok("keen_abc".into())
        );
        assert_eq!(read_key(&b"keen_abc"[..]), Ok("keen_abc".into()));
        assert_eq!(read_key(&b""[..]), Err("No API key on stdin"));
        assert_eq!(read_key(&b" \n\n"[..]), Err("No API key on stdin"));
    }

    #[test]
    fn only_web_links_are_opened() {
        for ok in [
            "https://keenable.ai/link?code=ABCD",
            "http://localhost:3000/link",
            "http://127.0.0.1:3000/link",
            "http://[::1]:3000/link",
        ] {
            assert!(is_web_link(ok), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "http://keenable.ai/link",
            "ms-msdt:/id",
            "javascript:alert(1)",
            "keenable.ai/link",
            "",
        ] {
            assert!(!is_web_link(bad), "{bad}");
        }
    }
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
        let no_hint = json!({"error": "Too many requests. Please try again later."});
        assert_eq!(classify_poll(429, None, Some(&no_hint), 5), wait(10, 10));
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
                "Authorization error: invalid_grant. This device_code was issued to a different client_id. Poll with the same client_id.".into()
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
    fn body_retry_after_is_used_when_the_header_is_missing() {
        let body = json!({"error": "Too many requests. Please try again later.", "retryAfter": 30});
        assert_eq!(classify_poll(429, None, Some(&body), 5), wait(10, 30));
        let body = json!({"error": "temporarily_unavailable", "retry_after_seconds": 12});
        assert_eq!(classify_poll(503, None, Some(&body), 5), wait(5, 12));
        let body = json!({"error": "x", "retryAfter": 60});
        assert_eq!(classify_poll(429, Some(20), Some(&body), 5), wait(10, 20));
    }

    #[test]
    fn unknown_errors_without_detail_keep_the_old_message() {
        assert_eq!(
            classify_poll(404, None, Some(&json!({"error": "Not found"})), 5),
            PollStep::Fail("Authorization error: Not found".into())
        );
    }

    #[test]
    fn grant_rejection_is_detected_across_error_shapes() {
        let current = json!({
            "error": "Invalid parameter",
            "message": "\"grant_type\": must be \"urn:ietf:params:oauth:grant-type:agent_code\""
        });
        let with_code = json!({
            "error": "Invalid parameter",
            "code": "invalid_parameter",
            "param": "grant_type",
            "message": "\"grant_type\": must be \"urn:ietf:params:oauth:grant-type:agent_code\""
        });
        let rfc = json!({"error": "unsupported_grant_type"});
        for body in [current, with_code, rfc] {
            assert_eq!(
                classify_poll(400, None, Some(&body), 5),
                PollStep::UseLegacyGrant
            );
        }
        let missing_code = json!({
            "error": "invalid_request",
            "code": "missing_parameter",
            "param": "device_code"
        });
        assert_ne!(
            classify_poll(400, None, Some(&missing_code), 5),
            PollStep::UseLegacyGrant
        );
        let rejected = json!({"error": "Invalid parameter", "message": "\"grant_type\": bad"});
        assert_ne!(
            classify_poll(429, None, Some(&rejected), 5),
            PollStep::UseLegacyGrant
        );
    }

    #[test]
    fn waits_never_outlive_the_code() {
        assert_eq!(
            wait_before_poll(5, Duration::from_secs(600)),
            Some(Duration::from_secs(5))
        );
        assert_eq!(wait_before_poll(3600, Duration::from_secs(30)), None);
        assert_eq!(wait_before_poll(30, Duration::from_secs(30)), None);
        assert_eq!(wait_before_poll(0, Duration::ZERO), None);
        assert_eq!(
            wait_before_poll(0, Duration::from_secs(1)),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn persistent_server_errors_fail_with_the_old_message() {
        assert_eq!(
            server_error_failure(500, Some(&json!({"error": "Internal server error"}))),
            Some("Authorization error: Internal server error".to_string())
        );
        let body = json!({
            "error": "temporarily_unavailable",
            "error_description": "Try again shortly.",
            "retry_after_seconds": 3
        });
        assert_eq!(
            server_error_failure(503, Some(&body)),
            Some("Authorization error: temporarily_unavailable. Try again shortly.".to_string())
        );
        assert_eq!(server_error_failure(502, None), None);
        assert_eq!(
            server_error_failure(502, Some(&json!({"message": "x"}))),
            None
        );
        assert_eq!(
            server_error_failure(429, Some(&json!({"error": "Too many requests"}))),
            None
        );
        assert_eq!(
            server_error_failure(400, Some(&json!({"error": "authorization_pending"}))),
            None
        );
    }

    #[test]
    fn a_second_grant_rejection_keeps_the_server_reason() {
        let base = "Authorization error: the server rejected the login request.";
        assert_eq!(grant_rejected_message(None), base);
        assert_eq!(grant_rejected_message(Some(&json!({"error": "x"}))), base);
        let body =
            json!({"error": "Invalid parameter", "message": "\"grant_type\": must be agent_code"});
        assert_eq!(
            grant_rejected_message(Some(&body)),
            format!("{} \"grant_type\": must be agent_code", base)
        );
    }

    #[test]
    fn unparseable_bodies_keep_polling() {
        assert_eq!(classify_poll(200, None, None, 5), wait(5, 5));
        assert_eq!(classify_poll(502, None, None, 5), wait(5, 5));
    }
}
