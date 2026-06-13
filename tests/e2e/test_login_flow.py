"""Stored-login + daemon path — the flow real users actually take.

Everything else in the suite passes --api-key, which skips the daemon and goes
direct HTTP. These tests run `keenable login --api-key` once, then exercise
search/fetch/feedback WITHOUT the flag, so the stored key is resolved from
config and requests go through the background daemon. Logout flips the same
commands to the public (free-tier) endpoints.
"""

import contextlib
import json
import os
import shutil
import signal
import tempfile
from pathlib import Path

import pytest

from conftest import (
    API_KEY,
    SEARCH_QUERY as QUERY,
    Runner,
    requires_posix,
    search_results,
    write_feedback,
)


def _kill_daemon(home: str):
    pid_file = Path(home) / ".keenable" / "daemon.pid"
    if pid_file.exists():
        with contextlib.suppress(ValueError, OSError):
            os.kill(int(pid_file.read_text().strip()), signal.SIGTERM)


@pytest.fixture
def short_home(tmp_path):
    """A home dir under a short base path.

    The daemon's Unix-domain socket lives at `<home>/.keenable/daemon.sock`,
    and macOS caps `sun_path` at 104 bytes. pytest's default tmp dir on macOS
    (`/private/var/folders/...`) blows past that, so the daemon can't bind and
    silently falls back to direct HTTP — failing the "daemon started" asserts.
    Anchor the home under `/tmp` on POSIX to keep the socket path short.
    Windows has no daemon, so the default `tmp_path` is fine there.
    """
    if os.name != "posix":
        yield str(tmp_path)
        return
    home = tempfile.mkdtemp(prefix="kn-e2e-", dir="/tmp")
    try:
        yield home
    finally:
        shutil.rmtree(home, ignore_errors=True)


@pytest.fixture
def logged_in(short_home):
    kn = Runner(short_home)
    res = kn("login", "--api-key", API_KEY, key=False)
    assert res.code == 0, res.err
    assert "API key saved" in res.err
    yield kn
    _kill_daemon(kn.home)


@requires_posix
def test_login_then_search_via_daemon(logged_in):
    assert search_results(logged_in, QUERY, key=False)

    keenable_dir = Path(logged_in.home) / ".keenable"
    assert (keenable_dir / "daemon.sock").exists(), "daemon did not start"

    # Second call reuses the same daemon (its whole point: connection reuse).
    pid = (keenable_dir / "daemon.pid").read_text().strip()
    assert logged_in("search", QUERY, key=False).code == 0
    assert (keenable_dir / "daemon.pid").read_text().strip() == pid


def test_login_then_fetch_via_daemon(logged_in):
    res = logged_in("fetch", "https://example.com", key=False)
    assert res.code == 0, res.out + res.err
    assert res.yaml()["title"] == "Example Domain"


@write_feedback
def test_login_then_feedback_via_daemon(logged_in):
    assert logged_in("search", QUERY, key=False).code == 0
    res = logged_in("feedback", QUERY, "https://tokio.rs=5=synthetic e2e feedback, ignore", key=False)
    assert res.code == 0, res.out + res.err
    assert res.yaml()["status"] == "ok"


def test_logout_clears_key_and_falls_back_to_public(logged_in):
    res = logged_in("logout", key=False)
    assert res.code == 0
    assert "Removed API key" in res.err

    config = json.loads((Path(logged_in.home) / ".keenable" / "config.json").read_text())
    assert config.get("api_key") is None  # logout nulls the key

    # Unauthenticated calls hit the public endpoints — IP rate-limited, so
    # accept either results or a rate-limit error carrying the login hint.
    res = logged_in("search", "test query", key=False)
    if res.code == 0:
        assert "results" in res.yaml()
    else:
        data = res.yaml()
        assert "error" in data
        assert "keenable login" in data.get("hint", ""), data


def test_invalid_login_key_rejected_at_login(tmp_path):
    """Since 0.1.19, login --api-key validates the key up front and refuses
    to save one the server rejects."""
    kn = Runner(str(tmp_path))
    res = kn("login", "--api-key", "keen_bad_key", key=False)
    assert res.code == 1
    assert "invalid" in res.err.lower()
    cfg = Path(kn.home) / ".keenable" / "config.json"
    assert not cfg.exists() or "keen_bad_key" not in cfg.read_text()
