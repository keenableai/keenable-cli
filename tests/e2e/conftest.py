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

    def __init__(self, home: str):
        self.home = home

    def __call__(self, *args: str, key: bool = True, timeout: int = 90) -> Result:
        cmd = [BIN, *args]
        if key:
            assert API_KEY, "KEENABLE_API_KEY must be set for API tests"
            cmd += ["--api-key", API_KEY]
        env = {**os.environ, "HOME": self.home}
        return Result(subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=timeout))


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
    res = kn("search", "rust async patterns")
    assert res.code == 0, res.err
    return res.yaml()


def results_of(data: dict) -> list:
    results = data.get("results")
    assert isinstance(results, list), f"missing results list: {list(data)}"
    return results


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
