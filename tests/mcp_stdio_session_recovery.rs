//! Integration test: verifies the stdio bridge transparently re-initializes
//! when the server returns -32001 "Session not found".
//!
//! Spins up a tiny HTTP server that:
//!   1. Accepts `initialize` → returns capabilities + `mcp-session-id` header
//!   2. Accepts `notifications/initialized` → returns 202
//!   3. Accepts the first `tools/call` → returns a normal result
//!   4. Accepts the second `tools/call` → returns -32001 (session expired)
//!   5. Expects the bridge to replay initialize + initialized
//!   6. Accepts the retried `tools/call` → returns a normal result

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Start a mock MCP server. Returns the port it's listening on.
fn start_mock_server() -> (u16, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    // Track how many tool calls we've seen (across all sessions).
    let tool_call_count = Arc::new(AtomicUsize::new(0));
    // Track how many initialize requests we've seen.
    let init_count = Arc::new(AtomicUsize::new(0));

    let handle = thread::spawn(move || {
        let mut methods_seen: Vec<String> = Vec::new();

        // We expect exactly these interactions:
        //   Session 1: initialize, initialized, tools/call (ok)
        //   Session 1: tools/call → -32001
        //   Session 2: initialize, initialized, tools/call (ok, retry)
        // Total: 7 HTTP requests
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            // Read HTTP request line
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();

            // Read headers
            let mut content_length: usize = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if header.to_lowercase().starts_with("content-length:") {
                    content_length = header
                        .split(':')
                        .nth(1)
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                }
            }

            // Read body
            let mut body_bytes = vec![0u8; content_length];
            if content_length > 0 {
                std::io::Read::read_exact(&mut reader, &mut body_bytes).unwrap();
            }

            let body_str = String::from_utf8_lossy(&body_bytes);
            let request: Value = if content_length > 0 {
                serde_json::from_str(&body_str).unwrap_or(json!({}))
            } else {
                json!({})
            };

            let method = request
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let id = request.get("id").cloned().unwrap_or(Value::Null);

            methods_seen.push(method.clone());

            match method.as_str() {
                "initialize" => {
                    let n = init_count.fetch_add(1, Ordering::SeqCst) + 1;
                    let session_id = format!("session-{}", n);
                    let resp_body = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "mock", "version": "1.0" }
                        }
                    });
                    let resp_str = serde_json::to_string(&resp_body).unwrap();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nmcp-session-id: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        session_id,
                        resp_str.len(),
                        resp_str
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
                "notifications/initialized" => {
                    let response =
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    stream.write_all(response.as_bytes()).unwrap();
                }
                "tools/call" => {
                    let n = tool_call_count.fetch_add(1, Ordering::SeqCst) + 1;
                    if n == 2 {
                        // Second tool call → session expired
                        let resp_body = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32001, "message": "Session not found" }
                        });
                        let resp_str = serde_json::to_string(&resp_body).unwrap();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_str.len(),
                            resp_str
                        );
                        stream.write_all(response.as_bytes()).unwrap();
                    } else {
                        // 1st and 3rd tool calls → success
                        let resp_body = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": format!("result-{}", n) }]
                            }
                        });
                        let resp_str = serde_json::to_string(&resp_body).unwrap();
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_str.len(),
                            resp_str
                        );
                        stream.write_all(response.as_bytes()).unwrap();
                    }
                }
                _ => {
                    let response =
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    stream.write_all(response.as_bytes()).unwrap();
                }
            }

            // After the retried tool call (3rd), we're done
            if tool_call_count.load(Ordering::SeqCst) >= 3 {
                break;
            }
        }
        methods_seen
    });

    (port, handle)
}

#[test]
fn session_recovery_on_expiry() {
    // Build the binary first
    let status = Command::new("cargo")
        .args(["build"])
        .status()
        .expect("cargo build failed");
    assert!(status.success(), "cargo build failed");

    let (port, server_handle) = start_mock_server();
    let mock_url = format!("http://127.0.0.1:{}/mcp", port);

    // Spawn the bridge process
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--",
            "mcp-stdio",
            "--api-key",
            "test-key",
            "--url",
            &mock_url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn bridge");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut stdout_reader = BufReader::new(stdout);

    let send = |stdin: &mut std::process::ChildStdin, msg: &Value| {
        let s = serde_json::to_string(msg).unwrap();
        writeln!(stdin, "{}", s).unwrap();
        stdin.flush().unwrap();
    };

    let recv = |reader: &mut BufReader<std::process::ChildStdout>| -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).unwrap()
    };

    // 1. Send initialize
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1.0" }
        }
    });
    send(&mut stdin, &init);
    let resp = recv(&mut stdout_reader);
    assert!(resp.get("result").is_some(), "initialize should succeed: {resp}");

    // 2. Send initialized notification (no response expected — but bridge may
    //    receive an empty 202 which produces no stdout output)
    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    send(&mut stdin, &initialized);
    // Small pause to let the notification go through
    thread::sleep(Duration::from_millis(200));

    // 3. First tools/call — should succeed
    let call1 = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "search", "arguments": { "query": "test1" } }
    });
    send(&mut stdin, &call1);
    let resp1 = recv(&mut stdout_reader);
    assert!(
        resp1.get("result").is_some(),
        "first tools/call should succeed: {resp1}"
    );
    let text1 = resp1["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text1, "result-1");

    // 4. Second tools/call — server will return -32001, bridge should
    //    transparently re-init and retry, returning the successful result.
    let call2 = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "search", "arguments": { "query": "test2" } }
    });
    send(&mut stdin, &call2);
    let resp2 = recv(&mut stdout_reader);
    assert!(
        resp2.get("result").is_some(),
        "second tools/call should succeed after session recovery: {resp2}"
    );
    let text2 = resp2["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text2, "result-3"); // 3rd tool call on server side (2nd was the -32001)

    // Close stdin to let the bridge exit
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("session expired, re-initializing"),
        "should log session recovery on stderr: {stderr}"
    );

    // Verify the mock server saw the expected sequence
    let methods = server_handle.join().unwrap();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "notifications/initialized",
            "tools/call",  // success
            "tools/call",  // → -32001
            "initialize",  // re-init
            "notifications/initialized",
            "tools/call",  // retry → success
        ],
        "unexpected request sequence: {:?}",
        methods
    );
}
