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
