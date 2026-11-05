//! The CLI must work behind a TLS-intercepting proxy whose CA is supplied via
//! `SSL_CERT_FILE` (keenableai/keenable-cli#70).
//!
//! Each test runs the real binary with `HTTPS_PROXY` pointing at a local
//! proxy that answers `CONNECT`, terminates TLS with a certificate for the API
//! host signed by a throwaway CA, and returns a canned search response. No
//! request leaves the machine.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

const API_HOST: &str = "api.keenable.ai";
const CANNED_RESULT_URL: &str = "https://intercepted.example/ok";

struct Mitm {
    ca_pem: String,
    acceptor: TlsAcceptor,
}

fn mitm() -> Mitm {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "keenable-cli test MITM CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate().unwrap();
    let leaf_params = CertificateParams::new(vec![API_HOST.to_string()]).unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();

    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![
                CertificateDer::from(leaf_cert.der().to_vec()),
                CertificateDer::from(ca_cert.der().to_vec()),
            ],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
        )
        .unwrap();

    Mitm {
        ca_pem: ca_cert.pem(),
        acceptor: TlsAcceptor::from(Arc::new(config)),
    }
}

/// CONNECT targets the proxy has seen, e.g. `api.github.com:443`.
type Tunnels = Arc<Mutex<Vec<String>>>;

/// Serve CONNECT tunnels forever; each one gets the MITM certificate and a
/// canned JSON response. Returns the proxy URL and the tunnels it opened.
async fn spawn_proxy(acceptor: TlsAcceptor) -> (String, Tunnels) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tunnels = Tunnels::default();
    let seen = tunnels.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let _ = handle_tunnel(stream, acceptor, seen).await;
            });
        }
    });
    (format!("http://{addr}"), tunnels)
}

async fn handle_tunnel(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    seen: Tunnels,
) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    // CONNECT request head; record the target from the request line.
    let mut line = String::new();
    loop {
        line.clear();
        if stream.read_line(&mut line).await? == 0 || line == "\r\n" {
            break;
        }
        if let Some(target) = line.strip_prefix("CONNECT ") {
            let target = target.split_whitespace().next().unwrap_or_default();
            seen.lock().unwrap().push(target.to_string());
        }
    }
    stream
        .get_mut()
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;

    let tls = acceptor.accept(stream.into_inner()).await?;
    let mut tls = BufReader::new(tls);
    let mut content_length = 0usize;
    loop {
        line.clear();
        if tls.read_line(&mut line).await? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; content_length];
    tls.read_exact(&mut body).await?;

    let payload = format!(r#"{{"results":[{{"url":"{CANNED_RESULT_URL}","title":"ok"}}]}}"#);
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    tls.get_mut().write_all(response.as_bytes()).await?;
    tls.get_mut().shutdown().await
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "keenable-tls-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `keenable search` through the proxy. `--api-key` takes the direct
/// path, so no background daemon outlives the test. `extra` args are
/// appended (`-p` also runs the GitHub update check).
async fn search_via_proxy(
    proxy: &str,
    home: &Path,
    ssl_cert_file: Option<&Path>,
    extra: &[&str],
) -> Output {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_keenable"));
    cmd.args(["search", "intercepted", "--api-key", "keen_test"])
        .args(extra)
        .env("KEENABLE_HOME", home)
        .env("HTTPS_PROXY", proxy)
        .env("KEENABLE_APP_TITLE", "keenable-cli-test");
    for var in [
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "KEENABLE_API_KEY",
        // An override would route around the MITM proxy's host.
        "KEENABLE_API_URL",
    ] {
        cmd.env_remove(var);
    }
    if let Some(path) = ssl_cert_file {
        cmd.env("SSL_CERT_FILE", path);
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output())
        .await
        .expect("keenable search timed out")
        .unwrap()
}

#[tokio::test]
async fn trusts_proxy_ca_from_ssl_cert_file() {
    let mitm = mitm();
    let (proxy, _) = spawn_proxy(mitm.acceptor).await;
    let dir = temp_dir("trusted");
    let ca_file = dir.join("proxy-ca.pem");
    std::fs::write(&ca_file, &mitm.ca_pem).unwrap();

    let out = search_via_proxy(&proxy, &dir, Some(&ca_file), &[]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains(CANNED_RESULT_URL),
        "expected the intercepted response\nstatus: {}\nstdout: {}\nstderr: {}",
        out.status,
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn rejects_proxy_ca_that_is_not_trusted() {
    let mitm = mitm();
    let (proxy, _) = spawn_proxy(mitm.acceptor).await;
    let dir = temp_dir("untrusted");

    // An empty SSL_CERT_FILE keeps the test independent of the host's trust
    // store; only the bundled webpki roots remain, which don't know the CA.
    let empty = dir.join("empty.pem");
    std::fs::write(&empty, "").unwrap();

    let out = search_via_proxy(&proxy, &dir, Some(&empty), &[]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    assert!(stdout.contains("Request failed"), "stdout: {stdout}");
    assert!(!stdout.contains(CANNED_RESULT_URL), "stdout: {stdout}");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn corrupt_ssl_cert_file_falls_back_to_bundled_roots() {
    let mitm = mitm();
    let (proxy, _) = spawn_proxy(mitm.acceptor).await;
    let dir = temp_dir("corrupt");

    // A PEM block that is not a certificate: every native cert fails to
    // parse, which makes reqwest refuse to build the client. The CLI must
    // fall back to webpki roots instead of panicking (exit 101).
    let corrupt = dir.join("corrupt.pem");
    std::fs::write(
        &corrupt,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .unwrap();

    let out = search_via_proxy(&proxy, &dir, Some(&corrupt), &[]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("Request failed"), "stdout: {stdout}");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn corrupt_ssl_cert_file_keeps_the_update_check() {
    let mitm = mitm();
    let (proxy, tunnels) = spawn_proxy(mitm.acceptor).await;
    let dir = temp_dir("corrupt-update");
    let corrupt = dir.join("corrupt.pem");
    std::fs::write(
        &corrupt,
        "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
    )
    .unwrap();

    // `-p` runs the update check. Its client must also fall back to webpki
    // roots; if it failed to build, it would never open a tunnel to GitHub.
    let out = search_via_proxy(&proxy, &dir, Some(&corrupt), &["-p"]).await;
    let tunnels = tunnels.lock().unwrap().clone();
    assert!(
        tunnels.iter().any(|t| t == "api.github.com:443"),
        "update check never connected; tunnels: {tunnels:?}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        tunnels.iter().any(|t| t == "api.keenable.ai:443"),
        "{tunnels:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
