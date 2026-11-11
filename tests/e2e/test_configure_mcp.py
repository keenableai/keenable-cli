"""configure-mcp functional tests — local config-file manipulation, no API.

configure-mcp doesn't take --api-key; with no stored key it configures for the
free tier (no network call). Each test runs under an isolated KEENABLE_HOME
(the `mcp` fixture) with fake client config dirs, then asserts the exact
mutations the command makes to each client's config.

A client is "detected" when the parent dir of its config path exists (Claude
Code: ~/.claude or ~/.claude.json), so the tests create those dirs to opt a
client in. All commands run with key=False (configure-mcp has no --api-key
flag) and --yes (skip the interactive confirmation).
"""

import json
from pathlib import Path

import pytest

from conftest import API_KEY, MCP_PUBLIC_URL, MCP_URL


def read_json(path: Path) -> dict:
    return json.loads(path.read_text())


def cursor_cfg(home: Path) -> Path:
    return home / ".cursor" / "mcp.json"


@pytest.fixture(scope="module")
def devin_and_mcp_public(kn):
    """Skip on a binary that predates --devin and the keyless /mcp-public URL.

    The nightly runs main's tests against the latest release, so tests for
    merged but unreleased behavior must skip there. --devin (#104) landed on
    top of keyless /mcp-public (#103), so a binary with the flag has both.
    """
    res = kn("configure-mcp", "--help", key=False)
    if "--devin" not in res.out + res.err:
        pytest.skip("binary predates --devin and keyless /mcp-public")


# ── Adding the entry ────────────────────────────────────────────────


def test_configure_cursor_adds_entry(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()  # makes Cursor "detected"

    res = mcp("configure-mcp", "--cursor", "--yes", key=False)
    assert res.code == 0, res.err

    entry = read_json(cursor_cfg(home))["mcpServers"]["keenable"]
    assert entry["url"] == MCP_URL
    assert entry["type"] == "streamable-http"
    assert "headers" not in entry  # free tier → no API key embedded


def test_configure_is_idempotent(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()

    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0
    first = cursor_cfg(home).read_text()

    res = mcp("configure-mcp", "--cursor", "--yes", key=False)
    assert res.code == 0
    assert cursor_cfg(home).read_text() == first  # second run is a no-op
    assert "already configured" in res.err.lower()


def test_configure_preserves_unrelated_mcp(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    cursor_cfg(home).write_text(json.dumps({
        "mcpServers": {"filesystem": {"command": "npx", "args": ["server-filesystem"]}}
    }))

    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0

    servers = read_json(cursor_cfg(home))["mcpServers"]
    assert "keenable" in servers
    assert servers["filesystem"]["command"] == "npx"  # untouched


def test_configure_removes_duplicate_keenable_entries(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    # A differently-named entry that points at Keenable's URL is a duplicate.
    cursor_cfg(home).write_text(json.dumps({
        "mcpServers": {"old-keen": {"url": MCP_URL, "type": "streamable-http"}}
    }))

    res = mcp("configure-mcp", "--cursor", "--yes", key=False)
    assert res.code == 0, res.err

    servers = read_json(cursor_cfg(home))["mcpServers"]
    assert "keenable" in servers
    assert "old-keen" not in servers  # collapsed into the canonical entry


# ── Per-client formats ──────────────────────────────────────────────


def test_configure_claude_code_disables_standard_tools(mcp):
    home = Path(mcp.home)
    (home / ".claude").mkdir()  # detect Claude Code

    res = mcp("configure-mcp", "--claude-code", "--yes", key=False)
    assert res.code == 0, res.err

    entry = read_json(home / ".claude.json")["mcpServers"]["keenable"]
    assert entry["url"] == MCP_URL and entry["type"] == "http"

    # Claude Code reads denies from ~/.claude/settings.json.
    deny = read_json(home / ".claude" / "settings.json")["permissions"]["deny"]
    assert "WebSearch" in deny and "WebFetch" in deny


def test_configure_codex_writes_toml(mcp, devin_and_mcp_public):
    home = Path(mcp.home)
    (home / ".codex").mkdir()

    res = mcp("configure-mcp", "--codex", "--yes", key=False)
    assert res.code == 0, res.err

    text = (home / ".codex" / "config.toml").read_text()
    assert "[mcp_servers.keenable]" in text
    # Keyless Codex gets /mcp-public (no OAuth discovery), and no key header.
    assert f'url = "{MCP_PUBLIC_URL}"' in text
    assert "http_headers" not in text

    # Re-running leaves one entry, and reset removes it.
    res = mcp("configure-mcp", "--codex", "--yes", key=False)
    assert res.code == 0, res.err
    assert (home / ".codex" / "config.toml").read_text().count("[mcp_servers.keenable]") == 1
    res = mcp("reset", "--codex", "--yes", key=False)
    assert res.code == 0, res.err
    assert "keenable" not in (home / ".codex" / "config.toml").read_text()


def test_configure_devin_writes_url_entry(mcp, devin_and_mcp_public):
    home = Path(mcp.home)
    devin_dir = home / ".config" / "devin"
    devin_dir.mkdir(parents=True)  # detect Devin Desktop
    devin_cfg = devin_dir / "mcp_config.json"
    # A differently-named entry pointing at Keenable is a duplicate.
    devin_cfg.write_text(json.dumps({
        "mcpServers": {
            "old-keen": {"serverUrl": MCP_URL},
            "github": {"command": "npx", "args": ["server-github"]},
        }
    }))

    res = mcp("configure-mcp", "--devin", "--yes", key=False)
    assert res.code == 0, res.err

    servers = read_json(devin_cfg)["mcpServers"]
    # Devin Desktop: URL under "url", no transport type; keyless uses /mcp-public.
    assert servers["keenable"] == {"url": MCP_PUBLIC_URL}
    assert "old-keen" not in servers
    assert servers["github"]["command"] == "npx"
    # The legacy Windsurf file is a separate client and stays untouched.
    assert not (home / ".codeium").exists()

    # Status mode lists it; reset removes the entry and keeps the rest.
    res = mcp("configure-mcp", key=False)
    assert res.code == 0, res.err
    assert "Devin Desktop" in res.err
    res = mcp("reset", "--devin", "--yes", key=False)
    assert res.code == 0, res.err
    servers = read_json(devin_cfg).get("mcpServers", {})
    assert "keenable" not in servers
    assert servers["github"]["command"] == "npx"


def test_reset_devin_removes_a_renamed_keenable_entry(mcp, devin_and_mcp_public):
    home = Path(mcp.home)
    devin_dir = home / ".config" / "devin"
    devin_dir.mkdir(parents=True)
    devin_cfg = devin_dir / "mcp_config.json"
    # Only a differently named entry points at Keenable.
    devin_cfg.write_text(json.dumps({
        "mcpServers": {
            "old-keen": {"url": MCP_URL},
            "github": {"command": "npx", "args": ["server-github"]},
        }
    }))

    res = mcp("reset", "--devin", "--yes", key=False)
    assert res.code == 0, res.err
    servers = read_json(devin_cfg)["mcpServers"]
    assert "old-keen" not in servers
    assert servers["github"]["command"] == "npx"


def test_configure_windsurf_keeps_legacy_path(mcp):
    home = Path(mcp.home)
    (home / ".codeium" / "windsurf").mkdir(parents=True)

    res = mcp("configure-mcp", "--windsurf", "--yes", key=False)
    assert res.code == 0, res.err

    entry = read_json(home / ".codeium" / "windsurf" / "mcp_config.json")["mcpServers"]["keenable"]
    assert entry == {"serverUrl": MCP_URL}
    assert not (home / ".config" / "devin").exists()


def test_configure_all_detected_clients(mcp, devin_and_mcp_public):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    (home / ".codex").mkdir()
    (home / ".config" / "devin").mkdir(parents=True)
    (home / ".codeium" / "windsurf").mkdir(parents=True)

    res = mcp("configure-mcp", "--all", "--yes", key=False)
    assert res.code == 0, res.err

    assert "keenable" in read_json(cursor_cfg(home))["mcpServers"]
    devin = read_json(home / ".config" / "devin" / "mcp_config.json")
    assert devin["mcpServers"]["keenable"]["url"] == MCP_PUBLIC_URL
    windsurf = read_json(home / ".codeium" / "windsurf" / "mcp_config.json")
    assert windsurf["mcpServers"]["keenable"]["serverUrl"] == MCP_URL
    assert "[mcp_servers.keenable]" in (home / ".codex" / "config.toml").read_text()


# ── API key embedding (uses the live API for the pre-flight check) ───


@pytest.mark.skipif(not API_KEY, reason="needs KEENABLE_API_KEY for the authenticated path")
def test_configure_embeds_stored_api_key(mcp):
    """With a logged-in key, the entry carries it as an X-API-Key header.

    login --api-key and configure-mcp's pre-flight both validate the key
    against the live API, so this one exercises the authenticated path.
    """
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    assert mcp("login", "--api-key", API_KEY, key=False).code == 0

    res = mcp("configure-mcp", "--cursor", "--yes", key=False)
    assert res.code == 0, res.err

    entry = read_json(cursor_cfg(home))["mcpServers"]["keenable"]
    assert entry["headers"]["X-API-Key"] == API_KEY


# ── Error / guard paths ─────────────────────────────────────────────


def test_configure_no_detected_client_errors(mcp):
    # No client dirs created → nothing detected to configure.
    res = mcp("configure-mcp", "--cursor", "--yes", key=False)
    assert res.code == 1
    assert "no matching clients" in res.err.lower()


def test_configure_without_yes_non_tty_exits(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    # No --yes and stdin is not a terminal (DEVNULL) → fail fast, don't hang.
    res = mcp("configure-mcp", "--cursor", key=False)
    assert res.code == 1
    assert "--yes" in res.err or "not a terminal" in res.err.lower()
