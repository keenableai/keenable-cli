use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    #[serde(default)]
    pub body: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Unix implementation (Unix sockets) ──────────────────────────────────────

#[cfg(unix)]
mod platform {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::sync::Mutex;
    use tokio::time::Instant;

    use crate::api::{api_key_client, api_url, handle_response};
    use crate::config;

    const IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

    fn socket_path() -> PathBuf {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(".keenable/daemon.sock")
    }

    fn pid_path() -> PathBuf {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(".keenable/daemon.pid")
    }

    pub async fn run_daemon() {
        let api_key = match config::get_api_key() {
            Some(key) => key,
            None => {
                eprintln!("Daemon: no API key configured");
                std::process::exit(1);
            }
        };

        let sock = socket_path();

        // Clean up stale socket
        if sock.exists() {
            fs::remove_file(&sock).ok();
        }

        // Ensure parent dir exists
        if let Some(parent) = sock.parent() {
            fs::create_dir_all(parent).ok();
        }

        let listener = UnixListener::bind(&sock).expect("failed to bind daemon socket");

        // Write PID file
        fs::write(pid_path(), std::process::id().to_string()).ok();

        let client = Arc::new(api_key_client(&api_key));
        let last_activity = Arc::new(Mutex::new(Instant::now()));

        // Spawn idle timeout watcher
        let last_activity_clone = last_activity.clone();
        let sock_clone = sock.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let elapsed = last_activity_clone.lock().await.elapsed();
                if elapsed >= IDLE_TIMEOUT {
                    // Clean up and exit
                    fs::remove_file(&sock_clone).ok();
                    fs::remove_file(pid_path()).ok();
                    std::process::exit(0);
                }
            }
        });

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    *last_activity.lock().await = Instant::now();
                    let client = client.clone();
                    let last_activity = last_activity.clone();
                    tokio::spawn(async move {
                        let (reader, mut writer) = stream.into_split();
                        let mut lines = BufReader::new(reader).lines();

                        while let Ok(Some(line)) = lines.next_line().await {
                            *last_activity.lock().await = Instant::now();

                            let response = match serde_json::from_str::<DaemonRequest>(&line) {
                                Ok(req) => handle_request(&client, req).await,
                                Err(e) => DaemonResponse {
                                    ok: false,
                                    data: None,
                                    error: Some(format!("Invalid request: {}", e)),
                                },
                            };

                            let mut resp_line =
                                serde_json::to_string(&response).unwrap_or_default();
                            resp_line.push('\n');
                            if writer.write_all(resp_line.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                Err(e) => {
                    eprintln!("Daemon accept error: {}", e);
                }
            }
        }
    }

    /// Send an API request and convert the result into a DaemonResponse.
    async fn send_api(request: reqwest::RequestBuilder) -> DaemonResponse {
        match request.send().await {
            Ok(r) => match handle_response(r).await {
                Ok(data) => DaemonResponse { ok: true, data: Some(data), error: None },
                Err(e) => DaemonResponse { ok: false, data: None, error: Some(e) },
            },
            Err(e) => DaemonResponse { ok: false, data: None, error: Some(e.to_string()) },
        }
    }

    fn err_response(msg: &str) -> DaemonResponse {
        DaemonResponse { ok: false, data: None, error: Some(msg.to_string()) }
    }

    async fn handle_request(client: &reqwest::Client, req: DaemonRequest) -> DaemonResponse {
        match req.command.as_str() {
            "search" => {
                let query = match &req.query {
                    Some(q) => q.as_str(),
                    None => return err_response("Missing query"),
                };
                let count = req.count.unwrap_or(10);
                send_api(
                    client
                        .get(api_url("/v1/search"))
                        .query(&[("query", query), ("count", &count.to_string())]),
                )
                .await
            }
            "fetch" => {
                let urls = match &req.urls {
                    Some(u) => u,
                    None => return err_response("Missing urls"),
                };
                send_api(
                    client
                        .post(api_url("/v1/fetch"))
                        .json(&serde_json::json!({"urls": urls})),
                )
                .await
            }
            "feedback" => {
                let body = match &req.body {
                    Some(b) => b.clone(),
                    None => return err_response("Missing body"),
                };
                send_api(
                    client.post(api_url("/v1/feedback")).json(&body),
                )
                .await
            }
            "ping" => DaemonResponse { ok: true, data: None, error: None },
            _ => err_response(&format!("Unknown command: {}", req.command)),
        }
    }

    // ── Client side: connect to daemon ──────────────────────────────────────

    pub fn is_daemon_running() -> bool {
        let sock = socket_path();
        if !sock.exists() {
            return false;
        }
        // Try to connect to verify it's alive
        match std::os::unix::net::UnixStream::connect(&sock) {
            Ok(_) => true,
            Err(_) => {
                // Stale socket, clean up
                fs::remove_file(&sock).ok();
                fs::remove_file(pid_path()).ok();
                false
            }
        }
    }

    pub fn start_daemon() -> Result<(), String> {
        let exe =
            std::env::current_exe().map_err(|e| format!("Cannot find executable: {}", e))?;

        let child = std::process::Command::new(exe)
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start daemon: {}", e))?;

        // Detach — we don't wait on the child
        drop(child);

        // Wait for daemon to accept connections (up to 3s)
        let sock = socket_path();
        for _ in 0..30 {
            if sock.exists() {
                if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Daemon did not start in time".to_string())
    }

    pub fn ensure_daemon() -> Result<(), String> {
        if !is_daemon_running() {
            start_daemon()?;
        }
        Ok(())
    }

    pub async fn daemon_request(req: &DaemonRequest) -> Result<DaemonResponse, String> {
        let sock = socket_path();
        let stream = tokio::net::UnixStream::connect(&sock)
            .await
            .map_err(|e| format!("Cannot connect to daemon: {}", e))?;

        let (reader, mut writer) = stream.into_split();

        let mut req_line = serde_json::to_string(req).unwrap();
        req_line.push('\n');
        writer
            .write_all(req_line.as_bytes())
            .await
            .map_err(|e| format!("Write error: {}", e))?;

        let mut lines = BufReader::new(reader).lines();
        match lines.next_line().await {
            Ok(Some(line)) => {
                serde_json::from_str(&line).map_err(|e| format!("Invalid daemon response: {}", e))
            }
            Ok(None) => Err("Daemon closed connection".to_string()),
            Err(e) => Err(format!("Read error: {}", e)),
        }
    }
}

// ── Windows stubs (no daemon support) ───────────────────────────────────────

#[cfg(not(unix))]
mod platform {
    use super::*;

    pub async fn run_daemon() {
        eprintln!("Daemon is not supported on Windows");
        std::process::exit(1);
    }

    pub fn ensure_daemon() -> Result<(), String> {
        Err("Daemon is not supported on Windows".to_string())
    }

    pub async fn daemon_request(_req: &DaemonRequest) -> Result<DaemonResponse, String> {
        Err("Daemon is not supported on Windows".to_string())
    }
}

// Re-export platform functions at module level
pub use platform::*;
