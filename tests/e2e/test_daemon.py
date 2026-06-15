"""Daemon concurrency / integrity — the "load test", done locally.

The daemon is the CLI's one piece of shared mutable state: keyless commands
all route through the same ~/.keenable/daemon.sock for connection reuse, and
the first commands race to spawn it. CLAUDE.md / daemon.rs: "two commands
racing can both spawn a daemon, and the loser must not unlink the winner's
live socket" — the bind-loser connect-checks and exits quietly, the winner
writes the pid file.

This fans out a burst of keyless searches from a fresh home (no daemon yet),
so they race to spawn one, and asserts the race resolves to exactly one
healthy daemon: no crashes, well-formed responses, a stable pid afterward (no
orphan, no respawn).

Free-tier (keyless) so it needs no key and exercises the bare-client daemon.
Unix-only (the daemon is stubbed on Windows). Public endpoints are
IP-rate-limited, so a 429 is tolerated — the test is about daemon integrity,
not search success — but a non-rate-limit error (e.g. a daemon race) fails it.
"""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from conftest import Runner, requires_posix

BURST = 10
QUERY = "rust async patterns"  # seeded/cached → fast, lighter on the public limit


def _healthy(res) -> bool:
    """Well-formed whether it succeeded or was merely rate-limited.

    A daemon race that returns a request-failure would lack the rate-limit
    login hint, so it fails this check rather than passing as "tolerated".
    """
    if res.code == 0:
        return "results" in res.yaml()
    if res.code == 1:
        data = res.yaml()
        return "error" in data and "keenable login" in data.get("hint", "")
    return False  # panic (101), timeout, etc.


@requires_posix
def test_concurrent_commands_resolve_to_one_daemon(short_home):
    kn = Runner(short_home)
    keenable_dir = Path(short_home) / ".keenable"
    try:
        # Fresh home → no daemon → this burst races to spawn one.
        with ThreadPoolExecutor(max_workers=BURST) as pool:
            results = list(pool.map(lambda _: kn("search", QUERY, key=False), range(BURST)))

        bad = [(r.code, (r.err or r.out)[:120]) for r in results if not _healthy(r)]
        assert not bad, f"unhealthy responses from the burst: {bad}"

        # The race resolved to exactly one live daemon.
        assert (keenable_dir / "daemon.sock").exists(), "no daemon after the burst"
        pid = (keenable_dir / "daemon.pid").read_text().strip()

        # Two more serial calls reuse that same daemon — no respawn, no orphan.
        for _ in range(2):
            res = kn("search", QUERY, key=False)
            assert _healthy(res), (res.code, (res.err or res.out)[:200])
            assert (keenable_dir / "daemon.pid").read_text().strip() == pid, \
                "pid changed — daemon respawned instead of being reused"
    finally:
        kn("logout", key=False)  # socket-handshake shutdown
