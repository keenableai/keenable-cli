//! Stdio↔HTTP bridge for MCP.
//!
//! Reads JSON-RPC messages from stdin, forwards them to the remote MCP
//! endpoint over HTTP (Streamable HTTP transport), and writes responses
//! back to stdout.  Used by Claude Desktop which requires stdio-based
//! MCP servers.

use reqwest::Client;
use serde_json::Value;
use std::process;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config;
use crate::constants::API_BASE_URL;

pub async fn run(api_key_override: Option<&str>, url_override: Option<&str>) {
    // When --url is provided, use it directly (token is embedded in URL).
    // Otherwise, build URL from API_BASE_URL and require an API key for headers.
    let (mcp_url, api_key) = if let Some(url) = url_override {
        (url.to_string(), None)
    } else {
        let key = match api_key_override {
            Some(k) => k.to_string(),
            None => match config::get_api_key() {
                Some(k) => k,
                None => {
                    eprintln!("No API key found. Run `keenable login` or pass --api-key.");
                    process::exit(1);
                }
            },
        };
        (format!("{}/mcp", API_BASE_URL), Some(key))
    };

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = io::stdout();
    let mut line = String::new();

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

        let mut req = client
            .post(&mcp_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(ref key) = api_key {
            req = req.header("X-API-Key", key);
        }
        let resp = req.json(&request).send().await;

        match resp {
            Ok(response) => {
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                if content_type.contains("text/event-stream") {
                    // SSE streaming: parse event stream and emit each JSON data line
                    let body = response.text().await.unwrap_or_default();
                    for sse_line in body.lines() {
                        if let Some(data) = sse_line.strip_prefix("data: ") {
                            let trimmed_data = data.trim();
                            if trimmed_data.is_empty() {
                                continue;
                            }
                            let mut out = trimmed_data.to_string();
                            out.push('\n');
                            let _ = stdout.write_all(out.as_bytes()).await;
                            let _ = stdout.flush().await;
                        }
                    }
                } else {
                    // Regular JSON response
                    let body = response.text().await.unwrap_or_default();
                    if !body.trim().is_empty() {
                        let mut out = body.trim().to_string();
                        out.push('\n');
                        let _ = stdout.write_all(out.as_bytes()).await;
                        let _ = stdout.flush().await;
                    }
                }
            }
            Err(e) => {
                // Return a JSON-RPC error for transport failures
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                let error_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32000,
                        "message": format!("Transport error: {}", e)
                    },
                    "id": id
                });
                let mut out = serde_json::to_string(&error_resp).unwrap();
                out.push('\n');
                let _ = stdout.write_all(out.as_bytes()).await;
                let _ = stdout.flush().await;
            }
        }
    }
}
