"""Installer e2e — download keenable-cli-installer.sh from GitHub Releases,
run it under a clean HOME, and smoke-test the binary it installs.

Tests the latest release by default; pin a version via
KEENABLE_INSTALLER_URL (the CI workflow sets it when given a version input).
"""

import os
import subprocess

import pytest

from conftest import Runner, requires_posix, search_results

# Downloads and runs the POSIX shell installer (curl | sh). Windows ships a
# separate PowerShell installer (keenable-cli-installer.ps1); the CI workflow
# installs the binary via that script on Windows, so this self-skips there.
pytestmark = [pytest.mark.install, requires_posix]

# `or` so an empty env var (CI without a version input) falls back to latest.
INSTALLER_URL = os.environ.get("KEENABLE_INSTALLER_URL") or \
    "https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.sh"


@pytest.fixture(scope="module")
def installed(tmp_path_factory) -> Runner:
    home = tmp_path_factory.mktemp("install-home")
    script = home / "installer.sh"

    # --retry covers transient HTTP 5xx from GitHub (observed 502s).
    curl = subprocess.run(
        ["curl", "--proto", "=https", "--tlsv1.2", "-LsSf", "--retry", "3",
         "-o", str(script), INSTALLER_URL],
        capture_output=True, text=True, timeout=300,
    )
    assert curl.returncode == 0, f"installer download failed: {curl.stderr}"

    # Clean HOME, no CARGO_HOME → installs to $HOME/.cargo/bin (install-path = CARGO_HOME).
    env = {k: v for k, v in os.environ.items() if k != "CARGO_HOME"}
    env["HOME"] = str(home)
    install = subprocess.run(["sh", str(script)], capture_output=True, text=True, env=env, timeout=300)
    assert install.returncode == 0, f"installer failed:\n{install.stdout}\n{install.stderr}"

    binary = home / ".cargo" / "bin" / "keenable"
    assert binary.is_file(), f"binary not at {binary}; installer output:\n{install.stdout}\n{install.stderr}"
    return Runner(str(home), bin=str(binary))


def test_installer_installs_working_binary(installed):
    res = installed("--version", key=False)
    assert res.code == 0
    assert res.out.startswith("keenable ")
    print(f"\ninstalled: {res.out.strip()} via {INSTALLER_URL}")


def test_installed_binary_smoke_search(installed):
    assert search_results(installed, "rust async patterns")
