"""Security hygiene — the CLI handles API keys, so two invariants matter:

1. The key never leaks into stdout/stderr (what users and agents capture/log).
2. The config file that stores it is owner-only on disk (0600).

Both run against the real binary; the key-leak error case and the perms case
need no valid key. Perms are Unix-only (Windows has no equivalent mode bits).
"""

import stat
from pathlib import Path

import pytest

from conftest import API_KEY, requires_posix

# Syntactically plausible but not a real key — used to prove the CLI never
# echoes a provided key back, even when it rejects it.
SENTINEL = "keen_sentinel_DEADBEEF_not_a_real_key"


def test_api_key_not_echoed_on_error(kn_fresh):
    res = kn_fresh("search", "test query", "--api-key", SENTINEL, key=False)
    assert res.code != 0, "an invalid key should be rejected"
    assert SENTINEL not in res.out, "API key leaked into stdout"
    assert SENTINEL not in res.err, "API key leaked into stderr"


@pytest.mark.skipif(not API_KEY, reason="needs a valid key to exercise success output")
def test_api_key_not_echoed_on_success(kn_fresh):
    res = kn_fresh("search", "rust async patterns", "--api-key", API_KEY, key=False)
    # Whether it returns results or is rate-limited, the key must not appear.
    assert API_KEY not in res.out, "API key leaked into stdout"
    assert API_KEY not in res.err, "API key leaked into stderr"


@requires_posix
def test_config_file_is_owner_only(kn_fresh):
    # config.json holds the API key (the api_key field), so it must never be
    # group/world-readable — atomic_write restricts it to 0600.
    res = kn_fresh("config", "set", "default_search_mode", "pro", key=False)
    assert res.code == 0, res.err
    cfg = Path(kn_fresh.home) / ".keenable" / "config.json"
    assert cfg.exists(), "config set did not write config.json"
    mode = stat.S_IMODE(cfg.stat().st_mode)
    assert mode == 0o600, f"config.json is {oct(mode)}, expected 0o600"


# A blank key is never a silent fallback. These exit before any request, so
# they need no API key and no network.
BLANK_KEYS = ["", " ", "\t"]


@pytest.fixture(scope="module")
def refuses_blank_keys(kn):
    """Skip on a binary that predates the blank-key refusal.

    The nightly runs main's tests against the latest release, so tests for
    merged but unreleased behavior must skip there. The refusal shipped with
    its note in the login help.
    """
    res = kn("login", "--help", key=False)
    assert res.code == 0, res.err  # a broken binary fails, not skips
    if "an empty key is refused" not in res.out + res.err:
        pytest.skip("binary predates the blank-key refusal")


@pytest.mark.parametrize("blank", BLANK_KEYS)
@pytest.mark.parametrize("cmd", [["search", "q"], ["fetch", "https://example.com"]])
def test_blank_api_key_flag_is_an_error(kn_fresh, refuses_blank_keys, cmd, blank):
    r = kn_fresh(*cmd, "--api-key", blank, key=False)
    assert r.code == 1
    data = r.yaml()
    assert data["error"] == "Empty API key"
    assert "hint" in data
    assert r.err.strip() == "error: Empty API key: --api-key was given an empty value."

    pretty = kn_fresh(*cmd, "-p", "--api-key", blank, key=False)
    assert pretty.code == 1
    assert pretty.out == ""
    assert "Empty API key" in pretty.err


@pytest.mark.parametrize("blank", BLANK_KEYS)
def test_login_with_a_blank_key_keeps_the_stored_key(kn_fresh, refuses_blank_keys, blank):
    config = Path(kn_fresh.home) / ".keenable" / "config.json"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text('{"api_key": "keen_stored_key"}')

    r = kn_fresh("login", "--api-key", blank, key=False)
    assert r.code == 1
    assert "The API key is empty" in r.err
    assert r.out == ""
    assert '"keen_stored_key"' in config.read_text()


def test_blank_api_key_env_warns_and_goes_anonymous(kn_fresh, refuses_blank_keys):
    """Live: the request itself goes to the public endpoint."""
    r = kn_fresh("search", "rust", key=False, env={"KEENABLE_API_KEY": "  "})
    assert "KEENABLE_API_KEY is set but empty" in r.err
    assert "KEENABLE_API_KEY" not in r.out
