//! `KEENABLE_API_URL` end to end: the built binary against a loopback stub
//! API. No request leaves the machine.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// One request the stub API received.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    api_key: Option<String>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// Serve canned JSON on a loopback port. Each result URL echoes the request
/// path, so output shows which endpoint answered.
async fn spawn_stub() -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Log::default();
    let seen = log.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let _ = serve(stream, seen).await;
            });
        }
    });
    (port, log)
}

async fn serve(stream: TcpStream, seen: Log) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let (mut content_length, mut api_key) = (0usize, None);
    loop {
        line.clear();
        if stream.read_line(&mut line).await? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("x-api-key") {
                api_key = Some(value.trim().to_string());
            }
        }
    }
    let mut body = vec![0; content_length];
    stream.read_exact(&mut body).await?;

    let payload =
        format!(r#"{{"results":[{{"url":"https://stub.example{path}","title":"stub"}}]}}"#);
    // `/redirect/<port>/...` answers with a redirect to another loopback port.
    let redirect = path
        .strip_prefix("/redirect/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(port, rest)| format!("http://127.0.0.1:{port}/{rest}"));
    seen.lock().unwrap().push(Seen {
        method,
        path,
        api_key,
    });
    if let Some(location) = redirect {
        let response = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        stream.get_mut().write_all(response.as_bytes()).await?;
        return stream.get_mut().shutdown().await;
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    stream.get_mut().write_all(response.as_bytes()).await?;
    stream.get_mut().shutdown().await
}

/// A fresh `KEENABLE_HOME`. Under /tmp on Unix: the daemon socket path must
/// stay below the 104-byte macOS limit, which the default temp dir breaks.
fn temp_home(name: &str) -> PathBuf {
    let root = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = root.join(format!(
        "kn-{name}-{}-{}",
        std::process::id(),
        nanos % 1_000_000_000
    ));
    std::fs::create_dir_all(dir.join(".keenable")).unwrap();
    dir
}

async fn keenable(home: &Path, api_url: &str, args: &[&str]) -> Output {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_keenable"));
    cmd.args(args)
        .env("KEENABLE_HOME", home)
        .env("KEENABLE_API_URL", api_url)
        .env("KEENABLE_APP_TITLE", "keenable-cli-test")
        .stdin(std::process::Stdio::null());
    for var in [
        "KEENABLE_API_KEY",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output())
        .await
        .expect("keenable timed out")
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn invalid_value_exits_with_message_and_hint() {
    let home = temp_home("invalid");
    let message = r#"KEENABLE_API_URL must be an https:// URL (http is allowed only for localhost, 127.0.0.1 and ::1), got "http://example.com""#;
    let hint = "Fix KEENABLE_API_URL or unset it to use https://api.keenable.ai.";

    // Agent-facing: structured YAML on stdout and a one-line summary on
    // stderr, like API errors.
    let out = keenable(&home, "http://example.com", &["search", "q"]).await;
    assert_eq!(out.status.code(), Some(1));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("error: Invalid KEENABLE_API_URL"),
        "{stdout}"
    );
    assert!(stdout.contains(&format!("hint: {hint}")), "{stdout}");
    let stderr = text(&out.stderr);
    assert_eq!(stderr.trim(), format!("error: {message}"), "{stderr}");
    assert!(stdout.contains("http://example.com"), "{stdout}");

    // Management commands: the same message and fix on stderr.
    for args in [
        &["login", "--api-key", "keen_test"][..],
        &["configure-mcp"][..],
        &["fetch", "https://example.org", "-p"][..],
    ] {
        let out = keenable(&home, "http://example.com", args).await;
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        // ui::error wraps long lines; compare without the wrapping.
        let stderr = text(&out.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(stderr.contains(hint), "{args:?}: {stderr}");
    }
    // Nothing was written: login never got as far as saving the key.
    assert!(!home.join(".keenable/credentials.json").exists());
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn keyed_direct_search_goes_to_the_override() {
    let (port, log) = spawn_stub().await;
    let home = temp_home("direct");
    let out = keenable(
        &home,
        &format!("http://127.0.0.1:{port}/"),
        &["search", "q", "--api-key", "keen_test"],
    )
    .await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("https://stub.example/v1/search"));

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, "/v1/search");
    assert_eq!(seen[0].api_key.as_deref(), Some("keen_test"));
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn api_key_does_not_follow_a_redirect_to_another_origin() {
    let (port, log) = spawn_stub().await;
    let (other_port, other_log) = spawn_stub().await;
    let home = temp_home("redirect");
    let out = keenable(
        &home,
        &format!("http://127.0.0.1:{port}/redirect/{other_port}"),
        &["search", "q", "--api-key", "keen_test"],
    )
    .await;
    assert_eq!(out.status.code(), Some(7), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("refused a redirect to another origin"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(log.lock().unwrap().len(), 1);
    let leaked = other_log.lock().unwrap().clone();
    assert!(leaked.is_empty(), "{leaked:?}");
    std::fs::remove_dir_all(&home).ok();
}

fn write_config(home: &Path, json: &str) {
    std::fs::write(home.join(".keenable/config.json"), json).unwrap();
}

#[tokio::test]
async fn stored_production_key_is_not_sent_to_an_override() {
    let (port, log) = spawn_stub().await;
    let home = temp_home("bound");
    write_config(&home, r#"{"api_key":"keen_stored"}"#);
    let base = format!("http://127.0.0.1:{port}");

    for args in [&["search", "q"][..], &["fetch", "https://example.org"][..]] {
        let out = keenable(&home, &base, args).await;
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&out.stderr)
        );
        let stdout = text(&out.stdout);
        assert!(stdout.contains("Stored API key not used"), "{stdout}");
        assert!(stdout.contains("KEENABLE_API_KEY"), "{stdout}");
        assert!(!stdout.contains("keen_stored"), "{stdout}");
    }
    let seen = log.lock().unwrap().clone();
    assert!(seen.is_empty(), "{seen:?}");
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn key_saved_under_an_override_is_sent_only_there() {
    let (port, log) = spawn_stub().await;
    let home = temp_home("saved");
    let base = format!("http://127.0.0.1:{port}");

    // A production key saved earlier must survive logging in elsewhere.
    write_config(&home, r#"{"api_key":"keen_prod"}"#);
    let out = keenable(&home, &base, &["login", "--api-key", "keen_local"]).await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".keenable/config.json")).unwrap())
            .unwrap();
    assert_eq!(config["api_key"], "keen_prod", "{config}");
    assert_eq!(config["api_keys"][&base], "keen_local", "{config}");

    let out = keenable(&home, &format!("{base}/"), &["search", "q", "-v"]).await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = log.lock().unwrap().clone();
    let last = seen.last().unwrap();
    assert_eq!(last.path, "/v1/search");
    assert_eq!(last.api_key.as_deref(), Some("keen_local"));

    // Another base: the key stays home.
    let (other_port, other_log) = spawn_stub().await;
    let out = keenable(
        &home,
        &format!("http://127.0.0.1:{other_port}"),
        &["search", "q"],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert!(other_log.lock().unwrap().is_empty());

    // Logging out under the override removes only that base's key.
    let out = keenable(&home, &base, &["logout"]).await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("were kept"),
        "{}",
        text(&out.stderr)
    );
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".keenable/config.json")).unwrap())
            .unwrap();
    assert_eq!(config["api_key"], "keen_prod", "{config}");
    assert!(config.get("api_keys").is_none(), "{config}");
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn configure_mcp_under_an_override_never_sends_the_stored_key_there() {
    let (port, log) = spawn_stub().await;
    let home = temp_home("mcp");
    write_config(
        &home,
        r#"{"api_key":"keen_stored","skip_setup_confirmation":true}"#,
    );
    std::fs::create_dir_all(home.join(".cursor")).unwrap();

    let out = keenable(
        &home,
        &format!("http://127.0.0.1:{port}"),
        &["configure-mcp", "--cursor", "-y"],
    )
    .await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    // The key is checked against production (the MCP URL configure-mcp
    // writes), never against the override.
    let leaked: Vec<_> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.api_key.is_some())
        .cloned()
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
    let cursor = std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap();
    assert!(cursor.contains("https://api.keenable.ai/mcp"), "{cursor}");
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn unparseable_config_is_never_overwritten() {
    let (port, _log) = spawn_stub().await;
    let home = temp_home("broken");
    let broken = r#"{"api_key":"keen_stored""#;
    write_config(&home, broken);
    let base = format!("http://127.0.0.1:{port}");

    for args in [
        &["login", "--api-key", "keen_new"][..],
        &["config", "set", "default_search_mode", "pro"][..],
        &["logout"][..],
    ] {
        let out = keenable(&home, &base, args).await;
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(text(&out.stderr).contains("Not changing"), "{args:?}");
        let now = std::fs::read_to_string(home.join(".keenable/config.json")).unwrap();
        assert_eq!(now, broken, "{args:?}");
    }
    std::fs::remove_dir_all(&home).ok();
}

#[tokio::test]
async fn path_prefix_base_is_kept() {
    let (port, log) = spawn_stub().await;
    let home = temp_home("prefix");
    let out = keenable(
        &home,
        &format!("http://localhost:{port}/gateway/keenable//"),
        &["fetch", "https://example.org", "--api-key", "keen_test"],
    )
    .await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "GET");
    assert!(
        seen[0]
            .path
            .starts_with("/gateway/keenable/v1/fetch?url=https%3A%2F%2Fexample.org"),
        "{seen:?}"
    );
    std::fs::remove_dir_all(&home).ok();
}

/// Daemon file stem for a base, as in `daemon::daemon_stem` (FNV-1a 64).
#[cfg(unix)]
fn daemon_stem(base: &str) -> String {
    let hash = base.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("daemon-{:012x}", hash >> 16)
}

/// A daemon from a CLI that predates the identity handshake: it answers
/// unknown commands with an untagged error and would serve any search. The
/// new client must replace it without ever sending it the real request.
#[cfg(unix)]
#[tokio::test]
async fn old_daemon_never_sees_the_real_request() {
    incompatible_daemon_never_sees_the_real_request("old", |_| None).await;
}

/// A daemon that answers the handshake for the right base but from another
/// CLI version (left running across an upgrade): replaced the same way.
#[cfg(unix)]
#[tokio::test]
async fn other_version_daemon_never_sees_the_real_request() {
    incompatible_daemon_never_sees_the_real_request("version", |base| {
        Some(format!(
            r#"{{"ok":true,"api_base":"{base}","cli_version":"0.0.1","build":"1-1"}}"#
        ))
    })
    .await;
}

/// `hello` builds the fake's handshake reply for a base; None answers like a
/// daemon that doesn't know the handshake.
#[cfg(unix)]
async fn incompatible_daemon_never_sees_the_real_request(
    name: &str,
    hello: fn(&str) -> Option<String>,
) {
    use tokio::net::UnixListener;

    let (port, log) = spawn_stub().await;
    let base = format!("http://127.0.0.1:{port}");
    let home = temp_home(name);
    let hello = hello(&base);
    let sock = home.join(format!(".keenable/{}.sock", daemon_stem(&base)));
    let listener = UnixListener::bind(&sock).unwrap();
    let commands = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = commands.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = seen.clone();
            let hello = hello.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let command = req["command"].as_str().unwrap_or_default().to_string();
                    seen.lock().unwrap().push(command.clone());
                    let reply = match command.as_str() {
                        "search" => {
                            r#"{"ok":true,"data":{"results":[{"url":"https://stale.example"}]}}"#
                                .to_string()
                        }
                        "shutdown" => r#"{"ok":true}"#.to_string(),
                        "identity" if hello.is_some() => hello.clone().unwrap(),
                        other => format!(r#"{{"ok":false,"error":"Unknown command: {other}"}}"#),
                    };
                    let _ = writer.write_all(format!("{reply}\n").as_bytes()).await;
                }
            });
        }
    });

    let out = keenable(&home, &base, &["search", "q"]).await;
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        stdout.contains("https://stub.example/v1/search/public"),
        "{stdout}"
    );
    assert!(!stdout.contains("stale.example"), "{stdout}");
    assert_eq!(
        *commands.lock().unwrap(),
        ["identity", "shutdown"],
        "the old daemon must only see the handshake and the shutdown"
    );
    assert_eq!(log.lock().unwrap().len(), 1, "{:?}", log.lock().unwrap());

    // Stop the daemon the CLI started in place of the fake one.
    if let Ok(mut stream) = tokio::net::UnixStream::connect(&sock).await {
        let _ = stream.write_all(b"{\"command\":\"shutdown\"}\n").await;
        let mut ack = String::new();
        let _ = BufReader::new(stream).read_line(&mut ack).await;
    }
    std::fs::remove_dir_all(&home).ok();
}

/// Fake daemon on the default socket for `keenable logout`. On shutdown it
/// optionally rewrites the pid file first, as a replacement daemon started
/// by a concurrent command would.
#[cfg(unix)]
async fn logout_with_fake_daemon(replaced: bool) -> (bool, Option<String>) {
    use tokio::net::UnixListener;

    let home = temp_home(if replaced { "replaced" } else { "stopped" });
    let sock = home.join(".keenable/daemon.sock");
    let pid = home.join(".keenable/daemon.pid");
    std::fs::write(&pid, "111").unwrap();
    let listener = UnixListener::bind(&sock).unwrap();
    let pid_file = pid.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            if let Ok(Some(_)) = lines.next_line().await {
                if replaced {
                    std::fs::write(&pid_file, "222").unwrap();
                }
                let _ = writer.write_all(b"{\"ok\":true}\n").await;
            }
        }
    });

    let out = keenable(&home, "", &["logout"]).await;
    assert!(out.status.success(), "{}", text(&out.stderr));
    let result = (sock.exists(), std::fs::read_to_string(&pid).ok());
    std::fs::remove_dir_all(&home).ok();
    result
}

#[cfg(unix)]
#[tokio::test]
async fn logout_removes_files_of_the_daemon_it_stopped() {
    assert_eq!(logout_with_fake_daemon(false).await, (false, None));
}

#[cfg(unix)]
#[tokio::test]
async fn logout_keeps_files_of_a_concurrently_started_daemon() {
    assert_eq!(
        logout_with_fake_daemon(true).await,
        (true, Some("222".to_string()))
    );
}
