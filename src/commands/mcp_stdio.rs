//! Stdio↔HTTP bridge for MCP.
//!
//! Reads JSON-RPC messages from stdin, forwards them to the remote MCP
//! endpoint over HTTP (Streamable HTTP transport), and writes responses
//! back to stdout.  Used by Claude Desktop which requires stdio-based
//! MCP servers.

use reqwest::header::HeaderMap;
use reqwest::Client;
use serde_json::Value;
use std::process;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config;
use crate::constants::API_BASE_URL;

/// Headers with this prefix are round-tripped between server and bridge.
const MCP_HEADER_PREFIX: &str = "mcp-";

pub async fn run(api_key_override: Option<&str>, url_override: Option<&str>) {
    let key = match api_key_override {
        Some(k) => Some(k.to_string()),
        None => config::get_api_key(),
    };

    let mcp_url = match url_override {
        Some(url) => url.to_string(),
        None => format!("{}/mcp", API_BASE_URL),
    };

    // API key is required for header-based auth (both Keenable and WebQL).
    // Legacy WebQL entries with ?token= in URL still work without a separate key.
    let api_key = if key.is_some() {
        key
    } else if !mcp_url.contains("token=") {
        eprintln!("No API key found. Run `keenable login` or pass --api-key.");
        process::exit(1);
    } else {
        None
    };

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = io::stdout();
    let mut line = String::new();

    // Accumulated mcp-* headers from server responses, forwarded on each request.
    let mut server_headers: HeaderMap = HeaderMap::new();

    // Store the initialize handshake so we can replay it on session expiry.
    let mut init_request: Option<Value> = None;
    let mut initialized_notification: Option<Value> = None;

    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break; // EOF
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Parse to validate it's JSON, then forward
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let error_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    },
                    "id": null
                });
                let mut out = serde_json::to_string(&error_resp).unwrap();
                out.push('\n');
                let _ = stdout.write_all(out.as_bytes()).await;
                let _ = stdout.flush().await;
                continue;
            }
        };

        // Remember the initialize handshake for session recovery.
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method == "initialize" {
            init_request = Some(request.clone());
        } else if method == "notifications/initialized" {
            initialized_notification = Some(request.clone());
        }

        let (body, is_session_expired) =
            send_request(&client, &mcp_url, &api_key, &mut server_headers, &request).await;

        // If the server-side session expired, transparently re-initialize and retry.
        if is_session_expired {
            if let Some(ref init_req) = init_request {
                eprintln!("[keenable] session expired, re-initializing…");
                server_headers.clear();

                // Replay initialize
                let (init_resp, _) =
                    send_request(&client, &mcp_url, &api_key, &mut server_headers, init_req)
                        .await;

                let init_ok = init_resp
                    .as_ref()
                    .map(|r| r.get("result").is_some())
                    .unwrap_or(false);

                if init_ok {
                    // Replay initialized notification
                    if let Some(ref notif) = initialized_notification {
                        let _ = send_request(
                            &client,
                            &mcp_url,
                            &api_key,
                            &mut server_headers,
                            notif,
                        )
                        .await;
                    }

                    // Retry the original request
                    let (retry_body, _) = send_request(
                        &client,
                        &mcp_url,
                        &api_key,
                        &mut server_headers,
                        &request,
                    )
                    .await;

                    if let Some(resp_val) = retry_body {
                        write_response(&mut stdout, &resp_val).await;
                    }
                    continue;
                }
                // Re-init failed — fall through and emit the original error
            }
        }

        if let Some(resp_val) = body {
            write_response(&mut stdout, &resp_val).await;
        }
    }
}

/// JSON-RPC error code returned by the server when the session has expired.
const SESSION_NOT_FOUND: i64 = -32001;

/// Write a JSON-RPC response to stdout.
async fn write_response(stdout: &mut io::Stdout, value: &Value) {
    let mut out = serde_json::to_string(value).unwrap();
    out.push('\n');
    let _ = stdout.write_all(out.as_bytes()).await;
    let _ = stdout.flush().await;
}

/// Send a JSON-RPC request to the MCP endpoint.
///
/// Returns the parsed response body (if any) and whether the error is a
/// session-not-found error that can be recovered by re-initializing.
async fn send_request(
    client: &Client,
    mcp_url: &str,
    api_key: &Option<String>,
    server_headers: &mut HeaderMap,
    request: &Value,
) -> (Option<Value>, bool) {
    let mut req = client
        .post(mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(key) = api_key {
        req = req.header("X-API-Key", key.as_str());
    }
    for (name, value) in server_headers.iter() {
        req = req.header(name.clone(), value.clone());
    }
    let resp = req.json(request).send().await;

    match resp {
        Ok(response) => {
            // Capture all mcp-* headers from the response
            for (name, value) in response.headers().iter() {
                if name.as_str().starts_with(MCP_HEADER_PREFIX) {
                    server_headers.insert(name.clone(), value.clone());
                }
            }

            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();

            if content_type.contains("text/event-stream") {
                // SSE streaming: parse event stream and emit each JSON data line.
                // Collect all data frames; the last one is typically the JSON-RPC response.
                let body = response.text().await.unwrap_or_default();
                let mut last: Option<Value> = None;
                for sse_line in body.lines() {
                    // spec allows both "data: x" and "data:x"
                    if let Some(data) = sse_line.strip_prefix("data:") {
                        let trimmed_data = data.trim();
                        if trimmed_data.is_empty() {
                            continue;
                        }
                        if let Ok(val) = serde_json::from_str::<Value>(trimmed_data) {
                            last = Some(val);
                        }
                    }
                }
                if last.is_none() {
                    // A request must get *some* answer or the client hangs.
                    return (
                        rpc_error(request, format!("HTTP {}: empty event stream", status)),
                        false,
                    );
                }
                let is_session_expired = is_session_not_found(last.as_ref());
                (last, is_session_expired)
            } else {
                // Regular JSON response
                let body = response.text().await.unwrap_or_default();
                let trimmed = body.trim();
                if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                    if is_session_not_found(Some(&val)) {
                        return (Some(val), true);
                    }
                    if status.is_success() || val.get("jsonrpc").is_some() {
                        (Some(val), false)
                    } else {
                        // Backend error format ({"error", "message"}) — wrap it,
                        // a non-JSON-RPC object on stdout violates the protocol.
                        (
                            rpc_error(request, format!("HTTP {}: {}", status, snippet(trimmed))),
                            false,
                        )
                    }
                } else if trimmed.is_empty() && status.is_success() {
                    // e.g. 202 Accepted for a notification
                    (None, false)
                } else {
                    (
                        rpc_error(request, format!("HTTP {}: {}", status, snippet(trimmed))),
                        false,
                    )
                }
            }
        }
        Err(e) => (rpc_error(request, format!("Transport error: {}", e)), false),
    }
}

/// Build a JSON-RPC error response for `request`, or `None` when the request
/// is a notification (no `id`) — JSON-RPC forbids responding to those.
fn rpc_error(request: &Value, message: String) -> Option<Value> {
    let id = request.get("id")?.clone();
    Some(serde_json::json!({
        "jsonrpc": "2.0",
        "error": { "code": -32000, "message": message },
        "id": id
    }))
}

/// Trim server bodies for error messages (an HTML error page can be huge).
fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

fn is_session_not_found(body: Option<&Value>) -> bool {
    body.and_then(|b| b.get("error"))
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
        == Some(SESSION_NOT_FOUND)
}
