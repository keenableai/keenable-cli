"""Unauthenticated / free-tier flow — search & fetch with no API key.

With no stored key, no `KEENABLE_API_KEY`, and no `--api-key`, the CLI hits the
public endpoints (`/v1/{search,fetch}/public`, IP-based rate limits) and the
daemon starts with a bare HTTP client (no `X-API-Key` header). The rest of the
suite always authenticates, so this is the only coverage of the first-run,
never-logged-in path real users start on.

All calls use key=False (the conftest Runner also strips KEENABLE_API_KEY from
the env), so a fresh home means a genuinely unauthenticated request. Public
endpoints are IP-rate-limited, so each test tolerates a 429 carrying the
free-tier login hint instead of asserting results unconditionally.
"""

import json
from pathlib import Path

from conftest import Runner, requires_posix, results_of


def test_free_tier_search_hits_public_endpoint(kn_fresh):
    res = kn_fresh("search", "python tutorial", key=False)
    if res.code == 0:
        results_of(res.yaml())  # public search returns the normal results shape
    else:
        data = res.yaml()
        assert "error" in data
        # Unauthenticated 429 invites login to raise limits (the authenticated
        # wording is "switch accounts" instead).
        hint = data.get("hint", "")
        assert "keenable login" in hint
        assert "increase your limits" in hint


def test_free_tier_fetch_hits_public_endpoint(kn_fresh):
    res = kn_fresh("fetch", "https://example.com", key=False)
    if res.code == 0:
        assert res.yaml()["title"] == "Example Domain"
    else:
        data = res.yaml()
        assert "error" in data
        assert "keenable login" in data.get("hint", "")


@requires_posix
def test_free_tier_daemon_starts_unauthenticated(short_home):
    """A keyless command still routes through the daemon, started bare."""
    kn = Runner(short_home)
    res = kn("search", "rust async patterns", key=False)
    keenable_dir = Path(short_home) / ".keenable"
    try:
        # Whether the request returns results or is rate-limited, the daemon is
        # spun up before the request, so its socket must exist either way.
        assert res.code in (0, 1), res.err
        assert (keenable_dir / "daemon.sock").exists(), "unauthenticated daemon did not start"
        # The daemon ran bare: no API key was persisted. The CLI stores the
        # key in config.json (api_key); credentials.json holds only OAuth
        # device tokens, so it's the wrong file to prove "no key".
        config_file = keenable_dir / "config.json"
        if config_file.exists():
            assert not json.loads(config_file.read_text()).get("api_key")
    finally:
        # Shut the daemon down the way the CLI does — a socket handshake via
        # logout's kill_daemon() (no network) — rather than signaling the
        # pidfile PID, which the daemon module avoids due to PID reuse.
        kn("logout", key=False)
