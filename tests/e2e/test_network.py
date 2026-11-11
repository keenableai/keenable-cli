"""Network setup: SSL_CERT_FILE, proxies, cause chains and hints, --verbose.

None of these reach the API: a bad SSL_CERT_FILE fails before any request,
the proxy cases point HTTPS_PROXY at a closed local port, and the daemon
cases talk to a local fake API behind a fake TLS-inspecting proxy.
"""

import os
import time
from pathlib import Path

import pytest

from conftest import Runner, requires_posix
from fake_network import FakeInspectingProxy, openssl_available

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
    assert res.code == 7  # network
    data = res.yaml()
    assert data["error"] == "Request failed"
    assert data["message"].startswith("error sending request")
    assert data["cause"], "transport errors must carry their cause chain"
    assert "127.0.0.1:9" in data["hint"]
    assert "HTTPS_PROXY" in data["hint"]


def test_dead_proxy_pretty_error_shows_cause(kn_fresh):
    res = kn_fresh("search", "rust", "-p", key=False, env=clean_env(HTTPS_PROXY=DEAD_PROXY), timeout=60)
    assert res.code == 7
    assert "Cause:" in res.err
    assert "HTTPS_PROXY" in res.err


def test_verbose_shows_proxy_cas_and_chain(kn_fresh):
    res = kn_fresh("search", "rust", "-p", "-v", key=False,
                   env=clean_env(HTTPS_PROXY="http://user:secret@127.0.0.1:9"), timeout=60)
    assert res.code == 7
    proxy_line = next(l for l in res.err.splitlines() if "Proxy:" in l)
    assert "127.0.0.1:9" in proxy_line
    assert "Trusted CAs: bundled Mozilla roots" in res.err
    assert "Error chain:" in res.err
    assert "secret" not in res.err, "proxy password must never be printed"


def test_help_lists_verbose(kn):
    res = kn("--help", key=False)
    assert res.code == 0
    assert "--verbose" in res.out
    assert "SSL_CERT_FILE" in res.out


# ── Daemon and network settings (Unix: the daemon is stubbed on Windows) ──

needs_openssl = pytest.mark.skipif(not openssl_available(), reason="needs the openssl CLI")


@pytest.fixture
def fake_proxy(tmp_path):
    proxies = []

    def make(fail_connects: int = 0) -> FakeInspectingProxy:
        d = tmp_path / f"fake-{len(proxies)}"
        d.mkdir()
        p = FakeInspectingProxy(d, fail_connects=fail_connects)
        proxies.append(p)
        return p

    yield make
    for p in proxies:
        p.close()


def _fake_env(proxy: FakeInspectingProxy, proxy_url: str | None = None) -> dict:
    return clean_env(HTTPS_PROXY=proxy_url or proxy.proxy_url, SSL_CERT_FILE=proxy.ca_file)


def _daemon_pid(home: str) -> str | None:
    pid_file = Path(home) / ".keenable" / "daemon.pid"
    return pid_file.read_text().strip() if pid_file.exists() else None


def _wait_gone(pid: str, timeout: float = 5.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            os.kill(int(pid), 0)
        except OSError:
            return True
        time.sleep(0.1)
    return False


@requires_posix
@needs_openssl
def test_changed_proxy_settings_respawn_the_daemon(short_home, fake_proxy):
    kn = Runner(short_home)
    proxy = fake_proxy()
    try:
        res = kn("search", "x", key=False, env=_fake_env(proxy))
        assert res.code == 0, res.err or res.out
        assert res.yaml()["results"][0]["title"] == "Fake inspected result"
        first = _daemon_pid(short_home)
        assert first, "the first command should have started the daemon"

        # Same proxy, different spelling: a changed HTTPS_PROXY value. The old
        # daemon steps aside and this request is served directly.
        changed = _fake_env(proxy, proxy_url=f"http://localhost:{proxy.port}")
        res = kn("search", "x", key=False, env=changed)
        assert res.code == 0, res.err or res.out
        assert _wait_gone(first), "the stale daemon should exit"

        res = kn("search", "x", key=False, env=changed)
        assert res.code == 0, res.err or res.out
        second = _daemon_pid(short_home)
        assert second and second != first, "a fresh daemon should serve the new settings"
    finally:
        kn("logout", key=False, env=clean_env())


@requires_posix
@needs_openssl
def test_network_failure_through_the_daemon_is_retried_directly(short_home, fake_proxy):
    kn = Runner(short_home)
    # The daemon's CONNECT is refused once; the client's direct retry gets through.
    proxy = fake_proxy(fail_connects=1)
    try:
        res = kn("search", "x", key=False, env=_fake_env(proxy))
        assert res.code == 0, res.err or res.out
        assert res.yaml()["results"][0]["title"] == "Fake inspected result"
        assert proxy.connects == 2, "expected one failed daemon attempt and one direct retry"
        # The daemon that failed was stopped so the next command gets a fresh one.
        assert _daemon_pid(short_home) is None
    finally:
        kn("logout", key=False, env=clean_env())
