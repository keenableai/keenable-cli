use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    #[serde(default)]
    pub body: Option<Value>,
    /// Fetch only: request live content instead of the cached copy.
    #[serde(default)]
    pub live: bool,
    /// Fetch only: LLM extraction instruction — the response content is the
    /// instruction's output instead of the full page.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Fetch only: content-length cap; the API defaults to 50000 when unset.
    #[serde(default)]
    pub max_chars: Option<u64>,
    /// Fetch only: groups the call with related searches and fetches.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The client's resolved API base URL. Filled in by `daemon_request`;
    /// a daemon started against another base refuses the request.
    #[serde(default)]
    pub api_base: Option<String>,
}

impl DaemonRequest {
    /// Query params for GET /v1/fetch, shared by the daemon and the direct
    /// HTTP path so fetch params can't drift between them. None when `urls`
    /// is missing.
    pub fn fetch_query(&self) -> Option<Vec<(&str, String)>> {
        let mut query: Vec<(&str, String)> = self
            .urls
            .as_ref()?
            .iter()
            .map(|u| ("url", u.clone()))
            .collect();
        if self.live {
            query.push(("live", "true".to_string()));
        }
        if let Some(p) = &self.prompt {
            query.push(("prompt", p.clone()));
        }
        if let Some(m) = self.max_chars {
            query.push(("max_chars", m.to_string()));
        }
        if let Some(id) = &self.session_id {
            query.push(("session_id", id.clone()));
        }
        Some(query)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// File stem for the daemon serving `base`: `daemon` for the production API
/// (the historical `daemon.sock` / `daemon.pid`), `daemon-<hash>` for an
/// override, so switching `KEENABLE_API_URL` never stops another base's
/// daemon. The 12-hex-digit FNV-1a hash is stable across builds and keeps the
/// socket path short (macOS caps it at 104 bytes).
#[cfg_attr(not(unix), allow(dead_code))]
fn daemon_stem(base: &str) -> String {
    if base == crate::constants::DEFAULT_API_BASE_URL {
        return "daemon".to_string();
    }
    let hash = base.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("daemon-{:012x}", hash >> 16)
}

/// Daemon side: whether a daemon serving `daemon_base` must refuse a request
/// tagged with `req_base`. Untagged requests come from CLIs older than the
/// tag, which only know the default base — serve them only if that's ours.
#[cfg_attr(not(unix), allow(dead_code))]
fn refuses_base(req_base: Option<&str>, daemon_base: &str) -> bool {
    match req_base {
        Some(base) => base != daemon_base,
        None => daemon_base != crate::constants::DEFAULT_API_BASE_URL,
    }
}

/// A daemon round-trip failed. Every command the daemon proxies is a read, so
/// callers always retry directly — whether the failure happened before or
/// after the request may have reached the API.
pub enum DaemonError {
    Unavailable,
}

// ── Unix implementation (Unix sockets) ──────────────────────────────────────

#[cfg(unix)]
mod platform {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::sync::Mutex;
    use tokio::time::Instant;

    use crate::api::{api_base, api_key_client, bare_client, handle_response};
    use crate::config;

    const IDLE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

    /// Command a client sends first on each connection. A current daemon answers
    /// with its tagged identity and does nothing else; an older daemon answers
    /// "Unknown command" without a tag, which the client treats as a mismatch —
    /// so the real request never reaches an incompatible daemon.
    const HANDSHAKE_COMMAND: &str = "identity";

    /// Error string for a request whose API base differs from the daemon's.
    const BASE_MISMATCH: &str = "API base URL mismatch";

    /// sun_path size, including the trailing NUL.
    const MAX_SOCKET_PATH: usize = if cfg!(target_os = "macos") { 104 } else { 108 };

    /// Serialize a response as one protocol line, tagged with the daemon's
    /// API base so clients can tell which base served it. Older daemons omit
    /// the tag, which clients treat as a mismatch.
    fn response_line(resp: &DaemonResponse, base: &str) -> String {
        let mut value = serde_json::to_value(resp).unwrap_or_default();
        value["api_base"] = Value::String(base.to_string());
        let mut line = serde_json::to_string(&value).unwrap_or_default();
        line.push('\n');
        line
    }

    fn daemon_dir() -> PathBuf {
        config::home_dir()
            .expect("cannot determine home directory")
            .join(".keenable")
    }

    /// Socket of the daemon for this process's API base.
    fn socket_path() -> PathBuf {
        daemon_dir().join(format!("{}.sock", daemon_stem(api_base())))
    }

    fn pid_path() -> PathBuf {
        daemon_dir().join(format!("{}.pid", daemon_stem(api_base())))
    }

    pub async fn run_daemon() {
        let api_key = config::get_api_key();
        let authenticated = api_key.is_some();
        let base: Arc<str> = Arc::from(api_base());

        let sock = socket_path();

        // Clean up stale socket — but two commands racing can both spawn a
        // daemon, and the loser must not unlink the winner's live socket.
        if sock.exists() {
            if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
                return; // another daemon is already serving
            }
            fs::remove_file(&sock).ok();
        }

        // Ensure parent dir exists; it holds credentials and the daemon
        // socket, so keep it private to the user.
        if let Some(parent) = sock.parent() {
            fs::create_dir_all(parent).ok();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
        }

        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            // Lost the spawn race to another daemon — exit quietly.
            Err(_) if std::os::unix::net::UnixStream::connect(&sock).is_ok() => return,
            Err(e) => {
                eprintln!("Failed to bind daemon socket: {}", e);
                std::process::exit(1);
            }
        };

        // Anyone who can write to the socket can use the stored API key —
        // don't rely on umask.
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&sock, fs::Permissions::from_mode(0o600)).ok();
        }

        // Write PID file
        fs::write(pid_path(), std::process::id().to_string()).ok();

        let client = Arc::new(match &api_key {
            Some(key) => api_key_client(key),
            None => bare_client(),
        });
        let authenticated = Arc::new(authenticated);
        let last_activity = Arc::new(Mutex::new(Instant::now()));

        // Spawn idle timeout watcher
        let last_activity_clone = last_activity.clone();
        let sock_clone = sock.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let elapsed = last_activity_clone.lock().await.elapsed();
                if elapsed >= IDLE_TIMEOUT {
                    // Clean up only if the files are still ours — a newer
                    // daemon may have replaced them.
                    let owns_files = fs::read_to_string(pid_path())
                        .map(|p| p.trim() == std::process::id().to_string())
                        .unwrap_or(false);
                    if owns_files {
                        fs::remove_file(&sock_clone).ok();
                        fs::remove_file(pid_path()).ok();
                    }
                    // A client may have connected between the idle check and
                    // the unlink; exiting now would EOF its in-flight request.
                    // The socket is gone, so no new connections can race in —
                    // re-check and keep serving the straggler if one arrived.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if last_activity_clone.lock().await.elapsed() >= IDLE_TIMEOUT {
                        std::process::exit(0);
                    }
                }
            }
        });

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    *last_activity.lock().await = Instant::now();
                    let client = client.clone();
                    let authenticated = authenticated.clone();
                    let base = base.clone();
                    let last_activity = last_activity.clone();
                    tokio::spawn(async move {
                        let (reader, mut writer) = stream.into_split();
                        let mut lines = BufReader::new(reader).lines();

                        while let Ok(Some(line)) = lines.next_line().await {
                            *last_activity.lock().await = Instant::now();

                            let response = match serde_json::from_str::<DaemonRequest>(&line) {
                                // Reply first, then exit — login/logout use this
                                // to restart the daemon with new auth state.
                                Ok(req) if req.command == "shutdown" => {
                                    let resp = DaemonResponse {
                                        ok: true,
                                        data: None,
                                        error: None,
                                    };
                                    let resp_line = response_line(&resp, &base);
                                    let _ = writer.write_all(resp_line.as_bytes()).await;
                                    let owns_files = fs::read_to_string(pid_path())
                                        .map(|p| p.trim() == std::process::id().to_string())
                                        .unwrap_or(false);
                                    if owns_files {
                                        fs::remove_file(socket_path()).ok();
                                        fs::remove_file(pid_path()).ok();
                                    }
                                    std::process::exit(0);
                                }
                                // Handshake: the tag added below is the answer.
                                Ok(req) if req.command == HANDSHAKE_COMMAND => DaemonResponse {
                                    ok: true,
                                    data: None,
                                    error: None,
                                },
                                Ok(req) if refuses_base(req.api_base.as_deref(), &base) => {
                                    err_response(BASE_MISMATCH)
                                }
                                Ok(req) => {
                                    handle_request(&client, &base, *authenticated, req).await
                                }
                                Err(e) => DaemonResponse {
                                    ok: false,
                                    data: None,
                                    error: Some(format!("Invalid request: {}", e)),
                                },
                            };

                            let resp_line = response_line(&response, &base);
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
    /// Errors carry `authenticated` (whether this daemon sent a key), so the
    /// client's hint matches the request rather than the current config.
    async fn send_api(request: reqwest::RequestBuilder, authenticated: bool) -> DaemonResponse {
        match request.send().await {
            Ok(r) => match handle_response(r).await {
                Ok(data) => DaemonResponse {
                    ok: true,
                    data: Some(data),
                    error: None,
                },
                Err(api_err) => api_error_response(&api_err, authenticated),
            },
            Err(e) => DaemonResponse {
                ok: false,
                data: None,
                error: Some(e.to_string()),
            },
        }
    }

    /// Forward the structured error so the client can reconstruct ApiError,
    /// plus `status` and whether this daemon sent the request with a key.
    fn api_error_response(api_err: &crate::api::ApiError, authenticated: bool) -> DaemonResponse {
        let mut data = api_err.to_yaml_value();
        data["status"] = serde_json::json!(api_err.status);
        data["authenticated"] = serde_json::json!(authenticated);
        DaemonResponse {
            ok: false,
            data: Some(data),
            error: Some(api_err.display()),
        }
    }

    fn err_response(msg: &str) -> DaemonResponse {
        DaemonResponse {
            ok: false,
            data: None,
            error: Some(msg.to_string()),
        }
    }

    fn endpoint(base: &str, path: &str, authenticated: bool) -> String {
        if authenticated {
            format!("{base}{path}")
        } else {
            format!("{base}{path}/public")
        }
    }

    async fn handle_request(
        client: &reqwest::Client,
        base: &str,
        authenticated: bool,
        req: DaemonRequest,
    ) -> DaemonResponse {
        match req.command.as_str() {
            "search" => {
                let body = match &req.body {
                    Some(b) => b.clone(),
                    None => return err_response("Missing body"),
                };
                send_api(
                    client
                        .post(endpoint(base, "/v1/search", authenticated))
                        .json(&body),
                    authenticated,
                )
                .await
            }
            "fetch" => {
                let query = match req.fetch_query() {
                    Some(q) => q,
                    None => return err_response("Missing urls"),
                };
                send_api(
                    client
                        .get(endpoint(base, "/v1/fetch", authenticated))
                        .query(&query),
                    authenticated,
                )
                .await
            }
            "ping" => DaemonResponse {
                ok: true,
                data: None,
                error: None,
            },
            _ => err_response(&format!("Unknown command: {}", req.command)),
        }
    }

    // ── Client side: connect to daemon ──────────────────────────────────────

    /// Stop every running daemon (one per API base) and clean up their
    /// files. Called after login/logout so daemons restart with the new auth
    /// state.
    pub fn kill_daemon() {
        let Ok(entries) = fs::read_dir(daemon_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".sock")
                && stem.starts_with("daemon")
            {
                stop_daemon(&entry.path(), &daemon_dir().join(format!("{stem}.pid")));
            }
        }
    }

    /// Ask the daemon at `sock` to shut down, then remove its files — but
    /// only if they still belong to it. A replacement started concurrently
    /// (by another command) rewrites the pid file; its files must survive.
    fn stop_daemon(sock: &Path, pid: &Path) {
        use std::io::{BufRead, Write};
        let before = fs::read_to_string(pid).ok();
        // Ask over the socket rather than signaling the pid file's process —
        // a stale pid may have been recycled by an unrelated process.
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(sock) {
            stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            if stream.write_all(b"{\"command\":\"shutdown\"}\n").is_ok() {
                // Wait for the ack so the exit (and file cleanup) lands before
                // we remove/recreate the files ourselves.
                let mut line = String::new();
                std::io::BufReader::new(&stream).read_line(&mut line).ok();
            }
        }
        // A pre-shutdown-command daemon answers "Unknown command" and stays
        // up; removing its files orphans it until its idle exit, while new
        // commands spawn a fresh daemon. A current daemon removed its own
        // files already, so the pid file no longer matches and we skip this.
        if fs::read_to_string(pid).ok() == before {
            fs::remove_file(sock).ok();
            fs::remove_file(pid).ok();
        }
    }

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
        let exe = std::env::current_exe().map_err(|e| format!("Cannot find executable: {}", e))?;

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
            if sock.exists() && std::os::unix::net::UnixStream::connect(&sock).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err("Daemon did not start in time".to_string())
    }

    pub fn ensure_daemon() -> Result<(), String> {
        // bind() would fail on an over-long path; go direct right away
        // instead of waiting for a daemon that can never start.
        if socket_path().as_os_str().len() >= MAX_SOCKET_PATH {
            return Err("Daemon socket path is too long".to_string());
        }
        if !is_daemon_running() {
            start_daemon()?;
        }
        Ok(())
    }

    /// Send a request to the daemon. A daemon started against a different
    /// API base, or an older CLI that doesn't report its base, is detected by
    /// the handshake before the request is sent, and replaced once. If the
    /// mismatch persists (e.g. a concurrent command won the restart) the
    /// caller goes direct.
    pub async fn daemon_request(req: &DaemonRequest) -> Result<DaemonResponse, DaemonError> {
        let base = api_base();
        let tagged_line = |value: &mut Value| {
            value["api_base"] = Value::String(base.to_string());
            let mut line = serde_json::to_string(value).map_err(|_| DaemonError::Unavailable)?;
            line.push('\n');
            Ok::<_, DaemonError>(line)
        };
        let handshake = tagged_line(&mut serde_json::json!({ "command": HANDSHAKE_COMMAND }))?;
        let request =
            tagged_line(&mut serde_json::to_value(req).map_err(|_| DaemonError::Unavailable)?)?;

        for attempt in 0..2 {
            if let Some(resp) = exchange(&handshake, &request, base).await? {
                return Ok(resp);
            }
            if attempt == 0 {
                stop_daemon(&socket_path(), &pid_path());
                start_daemon().map_err(|_| DaemonError::Unavailable)?;
            }
        }
        Err(DaemonError::Unavailable)
    }

    /// Handshake, then the request, on one connection. Ok(None) means the
    /// daemon serves another base (or predates the tag): checked on the
    /// handshake so the request is never sent, and again on the response as
    /// a backstop.
    async fn exchange(
        handshake: &str,
        request: &str,
        base: &str,
    ) -> Result<Option<DaemonResponse>, DaemonError> {
        let stream = tokio::net::UnixStream::connect(socket_path())
            .await
            .map_err(|_| DaemonError::Unavailable)?;
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        writer
            .write_all(handshake.as_bytes())
            .await
            .map_err(|_| DaemonError::Unavailable)?;
        let hello = read_value(&mut lines, Duration::from_secs(5)).await?;
        if hello["api_base"].as_str() != Some(base) {
            return Ok(None);
        }

        writer
            .write_all(request.as_bytes())
            .await
            .map_err(|_| DaemonError::Unavailable)?;
        // Wait longer than the daemon's own 60s HTTP timeout, so a slow
        // upstream yields the daemon's structured error instead of a client
        // timeout that falls back to a second, equally slow direct request.
        let value = read_value(&mut lines, Duration::from_secs(75)).await?;
        if value["api_base"].as_str() != Some(base) {
            return Ok(None);
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|_| DaemonError::Unavailable)
    }

    async fn read_value(
        lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        timeout: Duration,
    ) -> Result<Value, DaemonError> {
        match tokio::time::timeout(timeout, lines.next_line()).await {
            Ok(Ok(Some(line))) => serde_json::from_str(&line).map_err(|_| DaemonError::Unavailable),
            _ => Err(DaemonError::Unavailable),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::api::ApiError;

        fn forbidden() -> ApiError {
            ApiError {
                status: 403,
                error: "Forbidden".into(),
                message: Some("no".into()),
                request_id: Some("req-1".into()),
                ..Default::default()
            }
        }

        #[test]
        fn forwarded_error_carries_the_daemons_auth_state() {
            for authenticated in [true, false] {
                let resp = api_error_response(&forbidden(), authenticated);
                assert!(!resp.ok);
                let data = resp.data.expect("structured error data");
                assert_eq!(data["authenticated"], serde_json::json!(authenticated));
                assert_eq!(data["status"], 403);
                assert_eq!(data["error"], "Forbidden");
                assert_eq!(data["request_id"], "req-1");
                assert_eq!(resp.error.as_deref(), Some("Forbidden: no"));
            }
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

    pub fn kill_daemon() {}

    pub fn ensure_daemon() -> Result<(), String> {
        Err("Daemon is not supported on Windows".to_string())
    }

    pub async fn daemon_request(_req: &DaemonRequest) -> Result<DaemonResponse, DaemonError> {
        Err(DaemonError::Unavailable)
    }
}

// Re-export platform functions at module level
pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_API_BASE_URL;

    #[test]
    fn default_base_keeps_the_historical_daemon_files() {
        assert_eq!(daemon_stem(DEFAULT_API_BASE_URL), "daemon");
    }

    #[test]
    fn override_bases_get_their_own_stable_short_stem() {
        let a = daemon_stem("http://127.0.0.1:8080");
        let b = daemon_stem("http://127.0.0.1:8081");
        assert_ne!(a, b);
        assert_ne!(a, "daemon");
        // Stable: the same base always maps to the same file (FNV-1a 64,
        // cross-checked in Python).
        assert_eq!(a, daemon_stem("http://127.0.0.1:8080"));
        assert_eq!(daemon_stem("http://127.0.0.1:8080"), "daemon-75327da3b04b");
        // Short: "daemon-" + 12 hex digits, whatever the base length.
        let long = daemon_stem(&format!("https://{}.example/prefix", "x".repeat(200)));
        assert_eq!(long.len(), 19);
        assert!(long[7..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tagged_requests_must_match_the_daemon_base() {
        let local = "http://127.0.0.1:8080";
        assert!(!refuses_base(
            Some(DEFAULT_API_BASE_URL),
            DEFAULT_API_BASE_URL
        ));
        assert!(!refuses_base(Some(local), local));
        assert!(refuses_base(Some(local), DEFAULT_API_BASE_URL));
        assert!(refuses_base(Some(DEFAULT_API_BASE_URL), local));
    }

    #[test]
    fn untagged_requests_are_served_only_on_the_default_base() {
        // An older CLI always means the default base.
        assert!(!refuses_base(None, DEFAULT_API_BASE_URL));
        assert!(refuses_base(None, "http://127.0.0.1:8080"));
    }
}
