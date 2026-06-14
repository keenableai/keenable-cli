"""reset functional tests — the inverse of configure-mcp, local files only.

reset removes the Keenable MCP entry (and other entries pointing at Keenable's
URL) and, for Claude Code, restores the standard tools it disabled — removing
only the denies configure-mcp recorded as managed, never ones the user set
themselves. Runs under the isolated `mcp` fixture; no live API needed.
"""

import json
from pathlib import Path

from conftest import MCP_URL


def read_json(path: Path) -> dict:
    return json.loads(path.read_text())


def cursor_cfg(home: Path) -> Path:
    return home / ".cursor" / "mcp.json"


def test_reset_removes_keenable_entry(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0
    assert "keenable" in read_json(cursor_cfg(home))["mcpServers"]

    res = mcp("reset", "--cursor", "--yes", key=False)
    assert res.code == 0, res.err
    assert "keenable" not in read_json(cursor_cfg(home)).get("mcpServers", {})


def test_reset_roundtrip_preserves_unrelated(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    cursor_cfg(home).write_text(json.dumps({
        "mcpServers": {"filesystem": {"command": "npx", "args": ["server-filesystem"]}}
    }))

    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0
    assert mcp("reset", "--cursor", "--yes", key=False).code == 0

    servers = read_json(cursor_cfg(home))["mcpServers"]
    assert "keenable" not in servers
    assert servers["filesystem"]["command"] == "npx"  # unrelated entry intact


def test_reset_removes_other_keenable_url_entries(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0
    # Add a second, differently-named entry pointing at Keenable's URL.
    cfg = read_json(cursor_cfg(home))
    cfg["mcpServers"]["stray-keen"] = {"url": MCP_URL, "type": "streamable-http"}
    cursor_cfg(home).write_text(json.dumps(cfg))

    assert mcp("reset", "--cursor", "--yes", key=False).code == 0
    servers = read_json(cursor_cfg(home)).get("mcpServers", {})
    assert "keenable" not in servers and "stray-keen" not in servers


def test_reset_claude_code_restores_standard_tools(mcp):
    home = Path(mcp.home)
    (home / ".claude").mkdir()
    assert mcp("configure-mcp", "--claude-code", "--yes", key=False).code == 0
    assert "WebSearch" in read_json(home / ".claude" / "settings.json")["permissions"]["deny"]

    res = mcp("reset", "--claude-code", "--yes", key=False)
    assert res.code == 0, res.err

    deny = read_json(home / ".claude" / "settings.json").get("permissions", {}).get("deny", [])
    assert "WebSearch" not in deny and "WebFetch" not in deny
    assert "keenable" not in read_json(home / ".claude.json").get("mcpServers", {})


def test_reset_keeps_user_set_deny(mcp):
    """reset removes only the denies configure recorded as managed.

    The user denies WebSearch themselves before configuring; configure then
    only adds WebFetch (the one not already present), so reset restores
    WebFetch but leaves the user's WebSearch deny in place.
    """
    home = Path(mcp.home)
    (home / ".claude").mkdir()
    (home / ".claude" / "settings.json").write_text(json.dumps({
        "permissions": {"deny": ["WebSearch"]}
    }))

    assert mcp("configure-mcp", "--claude-code", "--yes", key=False).code == 0
    deny = read_json(home / ".claude" / "settings.json")["permissions"]["deny"]
    assert "WebSearch" in deny and "WebFetch" in deny

    assert mcp("reset", "--claude-code", "--yes", key=False).code == 0
    deny = read_json(home / ".claude" / "settings.json").get("permissions", {}).get("deny", [])
    assert "WebSearch" in deny  # user's own deny preserved
    assert "WebFetch" not in deny  # only the managed one removed


def test_reset_when_not_configured_is_noop(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()  # detected but not configured

    res = mcp("reset", "--cursor", "--yes", key=False)
    assert res.code == 0
    assert "no clients" in res.err.lower() or "configured" in res.err.lower()


def test_reset_without_yes_non_tty_exits(mcp):
    home = Path(mcp.home)
    (home / ".cursor").mkdir()
    assert mcp("configure-mcp", "--cursor", "--yes", key=False).code == 0

    # reset is destructive: only --yes skips the prompt, and a non-terminal
    # stdin must fail fast rather than hang.
    res = mcp("reset", "--cursor", key=False)
    assert res.code == 1
    assert "--yes" in res.err or "not a terminal" in res.err.lower()
