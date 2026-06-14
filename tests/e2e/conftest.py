"""E2E suite for the keenable CLI.

Runs the real binary (KEENABLE_BIN, default `keenable` on PATH) against the
live API. Requires KEENABLE_API_KEY. Every test runs under an isolated HOME so
config/daemon state never touches the real user and never leaks between runs.

Assertions are grounded against observed behavior of keenable 0.1.16
(probed live on 2026-06-06), per the spec at
https://paste.keenable.ai/keenable-cli-test-suite — updated where 0.1.16
already fixed the 0.1.15 quirks the spec guards (exit codes, single-URL fetch).
"""

import os
import subprocess
from datetime import datetime, timezone
from urllib.parse import urlparse

import pytest
import yaml

BIN = os.environ.get("KEENABLE_BIN", "keenable")
API_KEY = os.environ.get("KEENABLE_API_KEY", "")

# Seeds basic_search; feedback tests must submit for this exact query
SEARCH_QUERY = "rust async patterns"

# Successful feedback submissions persist synthetic relevance data in the live
# API (no test tenant / dry-run mode exists), so they are opt-in only and must
# never run on a schedule.
write_feedback = pytest.mark.skipif(
    os.environ.get("KEENABLE_E2E_WRITE_FEEDBACK") != "1",
    reason="persists synthetic feedback to the live API — opt in with KEENABLE_E2E_WRITE_FEEDBACK=1",
)

# Tests that rely on Unix-only behavior — the Unix-domain-socket daemon and the
# POSIX shell installer. On Windows the CLI stubs the daemon out and ships a
# PowerShell installer instead, so these self-skip there.
requires_posix = pytest.mark.skipif(
    os.name != "posix", reason="requires a POSIX platform (Unix daemon / shell installer)"
)


class Result:
    def __init__(self, proc: subprocess.CompletedProcess):
        self.code = proc.returncode
        self.out = proc.stdout
        self.err = proc.stderr

    def yaml(self) -> dict:
        data = yaml.safe_load(self.out)
        assert isinstance(data, dict), f"stdout is not a YAML map:\n{self.out[:500]}"
        return data


class Runner:
    """Runs the keenable binary under an isolated HOME."""

    def __init__(self, home: str, bin: str = BIN):
        self.home = home
        self.bin = bin

    def __call__(self, *args: str, key: bool = True, timeout: int = 90) -> Result:
        cmd = [self.bin, *args]
        if key:
            assert API_KEY, "KEENABLE_API_KEY must be set for API tests"
            cmd += ["--api-key", API_KEY]
        # KEENABLE_HOME redirects every keenable file path (config, daemon,
        # MCP client configs) under this temp dir. HOME is set too, but on
        # Windows `dirs::home_dir()` ignores it and queries the real profile —
        # KEENABLE_HOME is what actually isolates the run on every platform.
        env = {**os.environ, "HOME": self.home, "KEENABLE_HOME": self.home}
        # The CLI honors KEENABLE_API_KEY; tests control auth explicitly via
        # the --api-key flag, so keep the suite's own key out of the env.
        env.pop("KEENABLE_API_KEY", None)
        # The CLI always emits UTF-8 (YAML results, the 👀 brand mark). Decode
        # as UTF-8 explicitly: `text=True` alone uses the locale encoding, which
        # on Windows is cp1252 and chokes on any non-Latin-1 byte (e.g. the
        # continuation bytes of a “smart quote” in a result), killing the
        # reader thread and leaving stdout as None.
        return Result(subprocess.run(
            cmd, capture_output=True, text=True, encoding="utf-8",
            env=env, timeout=timeout,
        ))


@pytest.fixture(scope="session")
def kn(tmp_path_factory) -> Runner:
    return Runner(str(tmp_path_factory.mktemp("keenable-home")))


@pytest.fixture
def kn_fresh(tmp_path) -> Runner:
    """Per-test HOME for tests that mutate config."""
    return Runner(str(tmp_path))


@pytest.fixture(scope="session")
def basic_search(kn) -> dict:
    """T-01 search output, reused by schema/count/feedback tests."""
    res = kn("search", SEARCH_QUERY)
    assert res.code == 0, res.err
    return res.yaml()


def results_of(data: dict) -> list:
    results = data.get("results")
    assert isinstance(results, list), f"missing results list: {list(data)}"
    return results


def search_results(kn, *args: str, key: bool = True) -> list:
    """Run a search and return its results list (asserts exit 0)."""
    res = kn("search", *args, key=key)
    assert res.code == 0, res.out + res.err
    return results_of(res.yaml())


def parse_ts(value) -> datetime:
    # PyYAML resolves unquoted ISO timestamps to datetime already.
    if isinstance(value, datetime):
        dt = value
    else:
        dt = datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)


def host_of(url: str) -> str:
    return urlparse(url).hostname or ""


def host_under(host: str, domain: str) -> bool:
    return host == domain or host.endswith("." + domain)


def utc(y, m, d) -> datetime:
    return datetime(y, m, d, tzinfo=timezone.utc)
