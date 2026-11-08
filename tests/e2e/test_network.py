"""Network setup: SSL_CERT_FILE, proxies, cause chains and hints, --verbose.

None of these reach the API: a bad SSL_CERT_FILE fails before any request,
and the proxy cases point HTTPS_PROXY at a closed local port.
"""

import pytest

PROXY_VARS = ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy",
              "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy")
CERT_VARS = ("SSL_CERT_FILE", "SSL_CERT_DIR")


def clean_env(**overrides):
    """Unset every proxy/CA variable, then apply overrides."""
    env = {name: None for name in PROXY_VARS + CERT_VARS}
    env.update(overrides)
    return env


# Port 9 (discard) is closed on CI runners, so the proxy connect fails fast.
DEAD_PROXY = "http://127.0.0.1:9"


@pytest.mark.parametrize("cmd", [("search", "rust"), ("fetch", "https://example.com")])
def test_missing_cert_file_is_a_clear_yaml_error(kn_fresh, tmp_path, cmd):
    missing = str(tmp_path / "no-such-ca.pem")
    res = kn_fresh(*cmd, key=False, env=clean_env(SSL_CERT_FILE=missing))
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid SSL_CERT_FILE"
    assert missing in data["message"]
    assert "SSL_CERT_FILE" in data["hint"]
    assert "panicked" not in res.err


def test_garbage_cert_file_is_rejected(kn_fresh, tmp_path):
    garbage = tmp_path / "garbage.pem"
    garbage.write_text("not a certificate\n")
    res = kn_fresh("search", "rust", "-p", key=False, env=clean_env(SSL_CERT_FILE=str(garbage)))
    assert res.code == 1
    assert "Invalid SSL_CERT_FILE" in res.err
    assert "no certificates found" in res.err
    assert "panicked" not in res.err


def test_login_with_bad_cert_file_names_the_file(kn_fresh, tmp_path):
    missing = str(tmp_path / "no-such-ca.pem")
    res = kn_fresh("login", key=False, env=clean_env(SSL_CERT_FILE=missing), timeout=30)
    assert res.code == 1
    assert "Invalid SSL_CERT_FILE" in res.err
    assert "no-such-ca.pem" in res.err


def test_dead_proxy_yaml_error_has_cause_and_proxy_hint(kn_fresh):
    res = kn_fresh("search", "rust", key=False, env=clean_env(HTTPS_PROXY=DEAD_PROXY), timeout=60)
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Request failed"
    assert data["message"].startswith("error sending request")
    assert data["cause"], "transport errors must carry their cause chain"
    assert "127.0.0.1:9" in data["hint"]
    assert "HTTPS_PROXY" in data["hint"]


def test_dead_proxy_pretty_error_shows_cause(kn_fresh):
    res = kn_fresh("search", "rust", "-p", key=False, env=clean_env(HTTPS_PROXY=DEAD_PROXY), timeout=60)
    assert res.code == 1
    assert "Cause:" in res.err
    assert "HTTPS_PROXY" in res.err


def test_verbose_shows_proxy_cas_and_chain(kn_fresh):
    res = kn_fresh("search", "rust", "-p", "-v", key=False,
                   env=clean_env(HTTPS_PROXY="http://user:secret@127.0.0.1:9"), timeout=60)
    assert res.code == 1
    assert "Proxy: http://127.0.0.1:9" in res.err
    assert "Trusted CAs: bundled Mozilla roots" in res.err
    assert "Error chain:" in res.err
    assert "secret" not in res.err, "proxy password must never be printed"


def test_help_lists_verbose(kn):
    res = kn("--help", key=False)
    assert res.code == 0
    assert "--verbose" in res.out
    assert "SSL_CERT_FILE" in res.out
