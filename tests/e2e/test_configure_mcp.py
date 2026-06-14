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

from conftest import API_KEY, MCP_URL


def read_json(path: Path) -> dict:
    return json.loads(path.read_text())


def cursor_cfg(home: Path) -> Path:
    return home / ".cursor" / "mcp.json"


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


def test_configure_codex_writes_toml(mcp):
    home = Path(mcp.home)
    (home / ".codex").mkdir()

    res = mcp("configure-mcp", "--codex", "--yes", key=False)
    assert res.code == 0, res.err

    text = (home / ".codex" / "config.toml").read_text()
    assert "[mcp_servers.keenable]" in text
    assert MCP_URL in text


def test_configure_all_detected_clients(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    (home / ".codex").mkdir()

    res = mcp("configure-mcp", "--all", "--yes", key=False)
    assert res.code == 0, res.err

    assert "keenable" in read_json(cursor_cfg(home))["mcpServers"]
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
