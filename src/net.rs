//! Network setup shared by every HTTP client the CLI builds.
//!
//! - Trust: the bundled Mozilla roots, plus the OS certificate store, plus
//!   `SSL_CERT_FILE` / `SSL_CERT_DIR`. Corporate networks that inspect TLS
//!   install their root CA in the OS store (or hand out a PEM bundle); without
//!   these, every request fails with an opaque "error sending request".
//! - Proxy: reqwest honors `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` (and the
//!   OS proxy settings via the `system-proxy` feature). This module only
//!   *reports* which proxy applies, for `--verbose` and error hints.
//! - Errors: transport failures are flattened into their `source()` chain,
//!   with a hint when the chain points at a certificate or proxy problem.

use std::collections::HashSet;
use std::error::Error as StdError;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::constants::API_BASE_URL;
use crate::ui;

// ── --verbose ───────────────────────────────────────────────────────

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

// ── Environment ─────────────────────────────────────────────────────

/// Env vars that change how a request reaches the API. The daemon captures
/// its environment when spawned, so a change to any of these must restart it.
#[cfg_attr(not(unix), allow(dead_code))] // the daemon is Unix-only
const NETWORK_ENV_VARS: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "KEENABLE_APP_TITLE",
];

/// Fingerprint of the network-relevant environment, including the size and
/// mtime of `SSL_CERT_FILE` so appending a CA to the bundle counts as a change.
#[cfg_attr(not(unix), allow(dead_code))] // the daemon is Unix-only
pub fn env_fingerprint() -> String {
    let mut h = DefaultHasher::new();
    for name in NETWORK_ENV_VARS {
        name.hash(&mut h);
        std::env::var_os(name).hash(&mut h);
    }
    if let Some(meta) = cert_file_path().and_then(|p| std::fs::metadata(p).ok()) {
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
    }
    // Adding or removing a CA in SSL_CERT_DIR changes the directory's mtime
    // and entry count.
    if let Some(dirs) = std::env::var_os("SSL_CERT_DIR") {
        for dir in std::env::split_paths(&dirs) {
            dir.hash(&mut h);
            std::fs::metadata(&dir)
                .ok()
                .and_then(|m| m.modified().ok())
                .hash(&mut h);
            std::fs::read_dir(&dir).map(|d| d.count()).ok().hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

/// Cert env vars that are set but empty. OpenSSL and this CLI treat those
/// as unset, but rustls-native-certs would read "" as a path and skip the
/// OS store; `main` removes them before anything reads the environment.
pub fn empty_cert_vars(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<&'static str> {
    ["SSL_CERT_FILE", "SSL_CERT_DIR"]
        .into_iter()
        .filter(|n| get(n).is_some_and(|v| v.is_empty()))
        .collect()
}

fn cert_file_path() -> Option<PathBuf> {
    std::env::var_os("SSL_CERT_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

// ── SSL_CERT_FILE ───────────────────────────────────────────────────

/// `SSL_CERT_FILE` points at something that isn't a usable PEM bundle.
#[derive(Clone, Debug, PartialEq)]
pub struct CertFileError {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for CertFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

pub const CERT_FILE_HINT: &str =
    "Point SSL_CERT_FILE at a PEM file containing your CA certificates, or unset it.";

/// Read a PEM bundle. Fails — naming the file — when it can't be read, holds
/// malformed PEM, or holds no certificate rustls can use as a trust anchor.
pub fn load_cert_file(path: &Path) -> Result<Vec<CertificateDer<'static>>, CertFileError> {
    let fail = |reason: String| CertFileError {
        path: path.to_path_buf(),
        reason,
    };
    let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let certs = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(format!("invalid PEM ({})", e)))?;
    if certs.is_empty() {
        return Err(fail(
            "no certificates found (expected PEM \"-----BEGIN CERTIFICATE-----\" blocks)".into(),
        ));
    }
    let mut store = rustls::RootCertStore::empty();
    let usable: Vec<_> = certs
        .into_iter()
        .filter(|c| store.add(c.clone()).is_ok())
        .collect();
    if usable.is_empty() {
        return Err(fail("none of its certificates could be parsed".into()));
    }
    Ok(usable)
}

/// Validate `SSL_CERT_FILE` (when set) without loading the OS store — cheap
/// enough to run before handing a request to the daemon.
pub fn check_cert_file() -> Result<(), CertFileError> {
    match cert_file_path() {
        Some(p) => load_cert_file(&p).map(|_| ()),
        None => Ok(()),
    }
}

// ── Trust roots ─────────────────────────────────────────────────────

/// Extra trust anchors on top of the bundled Mozilla roots.
struct Roots {
    certs: Vec<CertificateDer<'static>>,
    /// Human-readable list of where the anchors came from (for --verbose).
    sources: Vec<String>,
    /// Non-fatal load problems, e.g. an unreadable file in SSL_CERT_DIR.
    warnings: Vec<String>,
}

struct RootSet {
    store: rustls::RootCertStore,
    seen: HashSet<Vec<u8>>,
    certs: Vec<CertificateDer<'static>>,
}

impl RootSet {
    /// Add one anchor; returns whether it was new and usable.
    fn add(&mut self, cert: CertificateDer<'static>) -> bool {
        if self.seen.contains(cert.as_ref()) || self.store.add(cert.clone()).is_err() {
            return false;
        }
        self.seen.insert(cert.as_ref().to_vec());
        self.certs.push(cert);
        true
    }
}

fn certificates(n: usize) -> String {
    if n == 1 {
        "1 certificate".to_string()
    } else {
        format!("{} certificates", n)
    }
}

fn load_roots() -> Result<Roots, CertFileError> {
    let mut set = RootSet {
        store: rustls::RootCertStore::empty(),
        seen: HashSet::new(),
        certs: Vec::new(),
    };
    let mut sources = vec!["bundled Mozilla roots".to_string()];
    let mut warnings = Vec::new();

    if let Some(path) = cert_file_path() {
        let added = load_cert_file(&path)?
            .into_iter()
            .filter(|c| set.add(c.clone()))
            .count();
        sources.push(format!(
            "SSL_CERT_FILE {} ({})",
            path.display(),
            certificates(added)
        ));
    }

    // rustls-native-certs reads SSL_CERT_FILE / SSL_CERT_DIR *instead of* the
    // OS store when either is set (the OpenSSL convention).
    let native = rustls_native_certs::load_native_certs();
    let added = native
        .certs
        .into_iter()
        .filter(|c| set.add(c.clone()))
        .count();
    // Describe what rustls-native-certs actually read: it switches on the
    // variables being present at all, so mirror that (main drops empty ones).
    let file_var = std::env::var_os("SSL_CERT_FILE");
    let dir_var = std::env::var_os("SSL_CERT_DIR");
    if let Some(dir) = &dir_var {
        sources.push(format!(
            "SSL_CERT_DIR {} ({})",
            PathBuf::from(dir).display(),
            certificates(added)
        ));
    } else if file_var.is_none() {
        sources.push(format!("OS certificate store ({})", certificates(added)));
    }
    if file_var.is_some() || dir_var.is_some() {
        sources.push("OS certificate store skipped (SSL_CERT_FILE/SSL_CERT_DIR is set)".into());
    }
    for e in native.errors {
        warnings.push(format!("Couldn't load some CA certificates: {}", e));
    }

    Ok(Roots {
        certs: set.certs,
        sources,
        warnings,
    })
}

static ROOTS: OnceLock<Result<Roots, CertFileError>> = OnceLock::new();
static REPORTED: Once = Once::new();

fn roots() -> Result<&'static Roots, CertFileError> {
    ROOTS.get_or_init(load_roots).as_ref().map_err(Clone::clone)
}

/// A reqwest builder that trusts the bundled roots plus the OS store and
/// SSL_CERT_FILE / SSL_CERT_DIR. Proxy env vars are honored by reqwest itself.
pub fn client_builder() -> Result<reqwest::ClientBuilder, CertFileError> {
    let roots = roots()?;
    if verbose() {
        REPORTED.call_once(|| report(roots));
    }
    let mut builder = reqwest::Client::builder();
    for cert in &roots.certs {
        if let Ok(c) = reqwest::Certificate::from_der(cert.as_ref()) {
            builder = builder.add_root_certificate(c);
        }
    }
    Ok(builder)
}

fn report(roots: &Roots) {
    match proxy_for_api() {
        Some(p) => ui::info(&format!("Proxy: {} (from {})", p.url, p.source)),
        None => ui::info("Proxy: none (direct connection)"),
    }
    ui::info(&format!("Trusted CAs: {}", roots.sources.join("; ")));
    for w in &roots.warnings {
        ui::warning(w);
    }
}

// ── Proxy ───────────────────────────────────────────────────────────

pub struct ProxyInfo {
    /// Proxy URL with any password masked.
    pub url: String,
    /// Where it came from: an env var name or "system proxy settings".
    pub source: String,
}

/// The proxy reqwest will use for the Keenable API, if any. Mirrors reqwest's
/// own lookup (env vars first, then OS settings, minus NO_PROXY).
pub fn proxy_for_api() -> Option<ProxyInfo> {
    let uri: http::Uri = API_BASE_URL.parse().ok()?;
    let intercept = hyper_util::client::proxy::matcher::Matcher::from_system().intercept(&uri)?;
    let source = proxy_env_source(|n| std::env::var(n).ok())
        .unwrap_or("system proxy settings")
        .to_string();
    Some(ProxyInfo {
        url: redact_userinfo(&intercept.uri().to_string()),
        source,
    })
}

/// The env var the HTTPS proxy comes from, using hyper-util's selection: the
/// first *present* of HTTPS_PROXY/https_proxy, used only if it parses as a
/// URI, else the first present of ALL_PROXY/all_proxy under the same rule.
/// None means no env proxy (reqwest then falls back to the OS settings).
fn proxy_env_source(get: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let usable = |names: [&'static str; 2]| {
        let name = names.into_iter().find(|n| get(n).is_some())?;
        let value = get(name)?;
        (!value.trim().is_empty() && value.parse::<http::Uri>().is_ok()).then_some(name)
    };
    usable(["HTTPS_PROXY", "https_proxy"]).or_else(|| usable(["ALL_PROXY", "all_proxy"]))
}

/// Mask the password in `scheme://user:pass@host` URLs.
fn redact_userinfo(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };
    let user = userinfo.split(':').next().unwrap_or("");
    let masked = if userinfo.contains(':') {
        format!("{}:***", user)
    } else {
        user.to_string()
    };
    let prefix = if scheme.is_empty() {
        String::new()
    } else {
        format!("{}://", scheme)
    };
    format!("{}{}@{}{}", prefix, masked, host, path)
}

// ── Transport errors ────────────────────────────────────────────────

/// Messages of `err` and each of its `source()`s, skipping ones the previous
/// message already contains (hyper and reqwest often repeat their inner error).
pub fn error_chain(err: &(dyn StdError + 'static)) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = Some(err);
    while let Some(e) = current {
        let msg = e.to_string();
        let msg = msg.trim();
        if !msg.is_empty() && !out.last().is_some_and(|prev| prev.contains(msg)) {
            out.push(msg.to_string());
        }
        current = e.source();
    }
    out
}

/// What a transport failure looked like, for picking a hint.
#[derive(Default)]
pub struct FailureContext<'a> {
    pub timeout: bool,
    pub connect: bool,
    pub proxy: Option<&'a ProxyInfo>,
    pub cert_file: Option<&'a Path>,
}

const PROXY_EXAMPLE: &str = "for example HTTPS_PROXY=http://proxy.example.com:8080";

/// A one-line fix suggestion for a transport failure, from its error chain.
pub fn transport_hint(chain: &[String], ctx: &FailureContext) -> Option<String> {
    let text = chain.join(" ").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));

    if has(&[
        "certificate",
        "unknownissuer",
        "unknown issuer",
        "self-signed",
        "self signed",
    ]) {
        return Some(match ctx.cert_file {
            Some(path) => format!(
                "The server's certificate isn't trusted. Check that SSL_CERT_FILE ({}) contains your network's CA certificate.",
                path.display()
            ),
            None => "The server's certificate isn't trusted. If you're behind a TLS-inspecting proxy, set SSL_CERT_FILE to your company's CA bundle (PEM), or add that CA to your OS certificate store.".to_string(),
        });
    }

    let via = ctx
        .proxy
        .map(|p| format!("the proxy at {} (from {})", p.url, p.source));

    if has(&["proxy authorization required"]) {
        return Some(format!(
            "{} requires authentication. Put the credentials in the proxy URL: HTTPS_PROXY=http://user:password@host:port",
            capitalize(via.as_deref().unwrap_or("the proxy"))
        ));
    }

    let connect = ctx.connect
        || has(&[
            "tunnel error",
            "connect",
            "dns error",
            "failed to lookup",
            "unreachable",
            "connection reset",
        ]);

    match (&via, ctx.timeout, connect) {
        (Some(via), true, _) => Some(format!(
            "The request timed out through {}. Check that the proxy is reachable, or add api.keenable.ai to NO_PROXY to bypass it.",
            via
        )),
        (Some(via), false, true) => Some(format!(
            "Couldn't connect through {}. Check the proxy address, or add api.keenable.ai to NO_PROXY to bypass it.",
            via
        )),
        (None, true, _) => Some(format!(
            "The request timed out. If your network requires a proxy, set HTTPS_PROXY ({}).",
            PROXY_EXAMPLE
        )),
        (None, false, true) => Some(format!(
            "Couldn't connect to api.keenable.ai. If your network requires a proxy, set HTTPS_PROXY ({}).",
            PROXY_EXAMPLE
        )),
        _ => None,
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A transport failure broken into display parts.
pub struct TransportFailure {
    /// Every message in the error chain, outermost first.
    pub chain: Vec<String>,
    pub hint: Option<String>,
    pub timeout: bool,
}

pub fn describe(err: &reqwest::Error) -> TransportFailure {
    let chain = error_chain(err);
    let proxy = proxy_for_api();
    let cert_file = cert_file_path();
    let ctx = FailureContext {
        timeout: err.is_timeout(),
        connect: err.is_connect(),
        proxy: proxy.as_ref(),
        cert_file: cert_file.as_deref(),
    };
    let hint = transport_hint(&chain, &ctx);
    TransportFailure {
        chain,
        hint,
        timeout: err.is_timeout(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<Layer>>);

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl StdError for Layer {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            self.1.as_deref().map(|l| l as &(dyn StdError + 'static))
        }
    }

    fn layers(msgs: &[&'static str]) -> Layer {
        let mut inner: Option<Box<Layer>> = None;
        for m in msgs.iter().rev() {
            inner = Some(Box::new(Layer(m, inner)));
        }
        *inner.unwrap()
    }

    fn chain(msgs: &[&str]) -> Vec<String> {
        msgs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn error_chain_walks_sources_outermost_first() {
        let err = layers(&[
            "error sending request for url (https://api.keenable.ai/v1/search)",
            "client error (Connect)",
            "invalid peer certificate: UnknownIssuer",
        ]);
        assert_eq!(
            error_chain(&err),
            chain(&[
                "error sending request for url (https://api.keenable.ai/v1/search)",
                "client error (Connect)",
                "invalid peer certificate: UnknownIssuer",
            ])
        );
    }

    #[test]
    fn error_chain_skips_repeated_and_empty_messages() {
        let err = layers(&[
            "tcp connect error: Connection refused (os error 111)",
            "Connection refused (os error 111)",
            "",
            "Connection refused (os error 111)",
        ]);
        assert_eq!(
            error_chain(&err),
            chain(&["tcp connect error: Connection refused (os error 111)"])
        );
    }

    fn proxy() -> ProxyInfo {
        ProxyInfo {
            url: "http://proxy.corp:3128/".into(),
            source: "HTTPS_PROXY".into(),
        }
    }

    #[test]
    fn certificate_errors_suggest_ssl_cert_file() {
        let c = chain(&[
            "client error (Connect)",
            "invalid peer certificate: UnknownIssuer",
        ]);
        let hint = transport_hint(&c, &FailureContext::default()).unwrap();
        assert!(hint.contains("TLS-inspecting proxy"), "{hint}");
        assert!(hint.contains("SSL_CERT_FILE"), "{hint}");
    }

    #[test]
    fn certificate_errors_win_over_proxy_hints() {
        let p = proxy();
        let ctx = FailureContext {
            connect: true,
            proxy: Some(&p),
            ..Default::default()
        };
        let c = chain(&["invalid peer certificate: UnknownIssuer"]);
        assert!(transport_hint(&c, &ctx).unwrap().contains("SSL_CERT_FILE"));
    }

    #[test]
    fn certificate_errors_name_an_already_set_cert_file() {
        let path = PathBuf::from("/etc/corp-ca.pem");
        let ctx = FailureContext {
            cert_file: Some(&path),
            ..Default::default()
        };
        let c = chain(&["invalid peer certificate: UnknownIssuer"]);
        assert!(
            transport_hint(&c, &ctx)
                .unwrap()
                .contains("SSL_CERT_FILE (/etc/corp-ca.pem)")
        );
    }

    #[test]
    fn connect_errors_without_a_proxy_suggest_https_proxy() {
        let c = chain(&[
            "client error (Connect)",
            "dns error: failed to lookup address information",
        ]);
        let hint = transport_hint(&c, &FailureContext::default()).unwrap();
        assert!(
            hint.starts_with("Couldn't connect to api.keenable.ai"),
            "{hint}"
        );
        assert!(hint.contains("HTTPS_PROXY"), "{hint}");
    }

    #[test]
    fn connect_errors_through_a_proxy_name_it() {
        let p = proxy();
        let ctx = FailureContext {
            proxy: Some(&p),
            ..Default::default()
        };
        let c = chain(&[
            "client error (Connect)",
            "tunnel error: failed to create underlying connection",
            "Connection refused (os error 111)",
        ]);
        let hint = transport_hint(&c, &ctx).unwrap();
        assert!(
            hint.contains("the proxy at http://proxy.corp:3128/ (from HTTPS_PROXY)"),
            "{hint}"
        );
        assert!(hint.contains("NO_PROXY"), "{hint}");
    }

    #[test]
    fn proxy_auth_failures_explain_credentials() {
        let p = proxy();
        let ctx = FailureContext {
            connect: true,
            proxy: Some(&p),
            ..Default::default()
        };
        let c = chain(&["tunnel error: proxy authorization required"]);
        let hint = transport_hint(&c, &ctx).unwrap();
        assert!(hint.starts_with("The proxy at"), "{hint}");
        assert!(hint.contains("user:password@"), "{hint}");
    }

    #[test]
    fn timeouts_get_a_timeout_hint() {
        let c = chain(&["operation timed out"]);
        let ctx = FailureContext {
            timeout: true,
            ..Default::default()
        };
        assert!(transport_hint(&c, &ctx).unwrap().contains("timed out"));
    }

    #[test]
    fn unrelated_errors_get_no_hint() {
        let c = chain(&["error decoding response body", "expected value at line 1"]);
        assert_eq!(transport_hint(&c, &FailureContext::default()), None);
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |n| map.get(n).cloned()
    }

    #[test]
    fn proxy_source_skips_an_empty_https_proxy() {
        let get = env_of(&[("HTTPS_PROXY", ""), ("ALL_PROXY", "http://proxy:3128")]);
        assert_eq!(proxy_env_source(get), Some("ALL_PROXY"));
    }

    #[test]
    fn proxy_source_follows_hyper_precedence() {
        let get = env_of(&[("https_proxy", "http://a:1"), ("ALL_PROXY", "http://b:2")]);
        assert_eq!(proxy_env_source(get), Some("https_proxy"));
        // The first *present* name wins even when empty, like hyper-util.
        let get = env_of(&[("HTTPS_PROXY", ""), ("https_proxy", "http://a:1")]);
        assert_eq!(proxy_env_source(get), None);
        assert_eq!(proxy_env_source(env_of(&[])), None);
    }

    #[test]
    fn empty_cert_vars_are_flagged_for_removal() {
        use std::ffi::OsString;
        let get = |n: &str| match n {
            "SSL_CERT_FILE" => Some(OsString::new()),
            "SSL_CERT_DIR" => Some(OsString::from("/etc/ssl/certs")),
            _ => None,
        };
        assert_eq!(empty_cert_vars(get), vec!["SSL_CERT_FILE"]);
        let get = |n: &str| (n == "SSL_CERT_DIR").then(OsString::new);
        assert_eq!(empty_cert_vars(get), vec!["SSL_CERT_DIR"]);
        assert!(empty_cert_vars(|_| None).is_empty());
    }

    #[test]
    fn redacts_proxy_passwords_only() {
        assert_eq!(
            redact_userinfo("http://bob:s3cret@proxy:8080/"),
            "http://bob:***@proxy:8080/"
        );
        assert_eq!(
            redact_userinfo("http://bob@proxy:8080"),
            "http://bob@proxy:8080"
        );
        assert_eq!(redact_userinfo("http://proxy:8080/"), "http://proxy:8080/");
        assert_eq!(
            redact_userinfo("http://proxy:8080/a@b"),
            "http://proxy:8080/a@b"
        );
    }

    // ── SSL_CERT_FILE loading ──

    const TEST_CA: &str = include_str!("../tests/fixtures/test-ca.pem");

    fn temp_file(name: &str, contents: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("keenable-net-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn cert_file_loads_a_pem_bundle() {
        let path = temp_file("good.pem", TEST_CA.as_bytes());
        assert_eq!(load_cert_file(&path).unwrap().len(), 1);
        let twice = format!("{TEST_CA}\n{TEST_CA}");
        let path = temp_file("bundle.pem", twice.as_bytes());
        assert_eq!(load_cert_file(&path).unwrap().len(), 2);
    }

    #[test]
    fn cert_file_missing_names_the_path() {
        let path = PathBuf::from("/nonexistent/keenable-ca.pem");
        let err = load_cert_file(&path).unwrap_err();
        assert_eq!(err.path, path);
        assert!(
            err.to_string()
                .starts_with("/nonexistent/keenable-ca.pem: "),
            "{err}"
        );
    }

    #[test]
    fn cert_file_without_certificates_is_rejected() {
        let path = temp_file("garbage.pem", b"this is not a certificate\n");
        let err = load_cert_file(&path).unwrap_err();
        assert!(err.reason.contains("no certificates found"), "{err}");
        let path = temp_file("binary.der", &[0x30, 0x82, 0x00, 0x01, 0xff]);
        assert!(load_cert_file(&path).is_err());
    }

    #[test]
    fn cert_file_with_broken_pem_is_rejected() {
        let path = temp_file(
            "broken.pem",
            b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n",
        );
        let err = load_cert_file(&path).unwrap_err();
        assert!(err.to_string().contains("broken.pem"), "{err}");
    }

    #[test]
    fn cert_file_with_unparseable_certificates_is_rejected() {
        let path = temp_file(
            "junk-der.pem",
            b"-----BEGIN CERTIFICATE-----\naGVsbG8gd29ybGQ=\n-----END CERTIFICATE-----\n",
        );
        let err = load_cert_file(&path).unwrap_err();
        assert!(err.reason.contains("could be parsed"), "{err}");
    }
}
