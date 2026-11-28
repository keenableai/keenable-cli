# /// script
# requires-python = ">=3.10"
# dependencies = ["pyyaml", "keenable"]
# ///
"""Smoke checks: does Keenable work for users right now?

Runs a handful of basic calls through what users actually have: the installed
`keenable` CLI (KEENABLE_BIN, default `keenable` on PATH), the latest `keenable`
Python SDK from PyPI (uv installs it fresh for each run), and the hosted MCP
server. Keyed and keyless paths. Only the stable contract is checked (a call
succeeds and returns something sensible), so the checks hold across releases.

A failed check is retried once after a short pause, so a single blip doesn't
page anyone. Exit 0 when every check passed, 1 otherwise. Writes a table to
$GITHUB_STEP_SUMMARY and the failed check names to $GITHUB_OUTPUT (`failed`).

    KEENABLE_API_KEY=keen_... uv run tests/smoke/smoke.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

import yaml

BIN = os.environ.get("KEENABLE_BIN", "keenable")
API_KEY = os.environ.get("KEENABLE_API_KEY", "")
MCP_BASE = os.environ.get("KEENABLE_MCP_BASE", "https://api.keenable.ai")
QUERY = "python programming language"
FETCH_URL = "https://example.com"
FETCH_TEXT = "Example Domain"
RETRY_PAUSE_S = int(os.environ.get("SMOKE_RETRY_PAUSE_S", "20"))

# Keyless calls share the runner's IP with other GitHub users, so a keyless
# rate limit says nothing about our service. Raised by a check to pass with a note.
class Note(Exception):
    pass


# -- CLI ---------------------------------------------------------------------

def cli(*args: str, key: bool) -> subprocess.CompletedProcess:
    env = {k: v for k, v in os.environ.items() if k != "KEENABLE_API_KEY"}
    cmd = [BIN, *args] + (["--api-key", API_KEY] if key else [])
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8",
                          stdin=subprocess.DEVNULL, env=env, timeout=90)


def check_exit(proc: subprocess.CompletedProcess, key: bool) -> None:
    if proc.returncode == 4 and not key:
        raise Note("keyless rate limit on the shared runner IP")
    assert proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr.strip()[:300]}"


def cli_search(key: bool) -> str:
    proc = cli("search", QUERY, key=key)
    check_exit(proc, key)
    results = (yaml.safe_load(proc.stdout) or {}).get("results") or []
    assert results and all(r.get("url") for r in results), f"no results: {proc.stdout[:300]}"
    return f"{len(results)} results"


def cli_fetch(key: bool) -> str:
    proc = cli("fetch", FETCH_URL, key=key)
    check_exit(proc, key)
    assert FETCH_TEXT in proc.stdout, f"page text missing: {proc.stdout[:300]}"
    return "page read"


# -- Python SDK --------------------------------------------------------------

def sdk_search() -> str:
    from keenable import Keenable
    results = list(Keenable(API_KEY).search(QUERY))
    assert results and all(r.url for r in results), "no results"
    return f"{len(results)} results"


def sdk_fetch() -> str:
    from keenable import Keenable
    page = Keenable(API_KEY).fetch([FETCH_URL]).results[0]
    assert not page.error, f"page error: {page.error}"
    assert FETCH_TEXT in (page.content or ""), "page text missing"
    return "page read"


# -- Hosted MCP server -------------------------------------------------------

def mcp_post(url: str, headers: dict, msg: dict) -> tuple[dict | None, dict]:
    req = urllib.request.Request(url, data=json.dumps(msg).encode(), method="POST", headers={
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        **headers,
    })
    try:
        with urllib.request.urlopen(req, timeout=60) as res:
            body = res.read().decode()
            ctype, resp_headers = res.headers.get("content-type", ""), dict(res.headers)
    except urllib.error.HTTPError as e:
        raise AssertionError(f"HTTP {e.code}: {e.read().decode()[:300]}") from None
    if "id" not in msg:  # a notification: no answer expected
        return None, resp_headers
    if "text/event-stream" in ctype:
        # One JSON-RPC message per SSE event; data lines of one event join with \n.
        for event in body.replace("\r\n", "\n").split("\n\n"):
            data = "\n".join(line[5:].lstrip(" ") for line in event.split("\n") if line.startswith("data:"))
            try:
                parsed = json.loads(data) if data else None
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict) and parsed.get("id") == msg["id"]:
                return parsed, resp_headers
        raise AssertionError(f"no answer to request {msg['id']}: {body[:300]}")
    return json.loads(body), resp_headers


def mcp_call(key: bool, tool: str, arguments: dict) -> str:
    url = f"{MCP_BASE}/mcp" if key else f"{MCP_BASE}/mcp-public"
    headers = {"X-API-Key": API_KEY} if key else {}
    init, resp_headers = mcp_post(url, headers, {
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                   "clientInfo": {"name": "keenable-smoke", "version": "1"}},
    })
    assert init and "result" in init, f"initialize failed: {init}"
    session = {k: v for k, v in resp_headers.items() if k.lower() == "mcp-session-id"}
    headers = {**headers, **session}
    mcp_post(url, headers, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    res, _ = mcp_post(url, headers, {
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    })
    assert res and "result" in res, f"JSON-RPC error: {res}"
    text = "".join(c.get("text", "") for c in res["result"].get("content", []))
    if res["result"].get("isError"):
        if not key and "rate" in text.lower():
            raise Note("keyless rate limit on the shared runner IP")
        raise AssertionError(f"tool error: {text[:300]}")
    assert text.strip(), "empty tool result"
    return text


def mcp_search(key: bool) -> str:
    mcp_call(key, "search_web_pages", {"query": QUERY})
    return "results returned"


def mcp_fetch() -> str:
    assert FETCH_TEXT in mcp_call(True, "fetch_page_content", {"urls": [FETCH_URL]}), "page text missing"
    return "page read"


CHECKS = [
    ("cli search (key)", lambda: cli_search(True)),
    ("cli search (keyless)", lambda: cli_search(False)),
    ("cli fetch (key)", lambda: cli_fetch(True)),
    ("cli fetch (keyless)", lambda: cli_fetch(False)),
    ("python sdk search (key)", sdk_search),
    ("python sdk fetch (key)", sdk_fetch),
    ("mcp search (key)", lambda: mcp_search(True)),
    ("mcp search (keyless)", lambda: mcp_search(False)),
    ("mcp fetch (key)", mcp_fetch),
]


def run(fn) -> tuple[str, str, int]:
    start = time.monotonic()
    try:
        detail, status = fn(), "pass"
    except Note as e:
        detail, status = str(e), "note"
    except Exception as e:  # noqa: BLE001 - any failure is a failed check
        detail, status = f"{type(e).__name__}: {e}", "fail"
    return status, detail, round((time.monotonic() - start) * 1000)


def main() -> int:
    if not API_KEY:
        print("KEENABLE_API_KEY must be set", file=sys.stderr)
        return 2
    rows, failed = [], []
    for name, fn in CHECKS:
        status, detail, ms = run(fn)
        if status == "fail":
            print(f"retrying {name} in {RETRY_PAUSE_S}s: {detail}", file=sys.stderr)
            time.sleep(RETRY_PAUSE_S)
            status, detail, ms = run(fn)
            if status != "fail":
                detail += " (passed on retry)"
        if status == "fail":
            failed.append(name)
        icon = {"pass": "✅", "note": "⚠️", "fail": "❌"}[status]
        rows.append(f"| {icon} | {name} | {ms} ms | {detail.replace('|', '/')} |")
        print(f"{icon} {name:<26} {ms:>6} ms  {detail}")

    table = "| | check | time | detail |\n|---|---|---|---|\n" + "\n".join(rows) + "\n"
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a", encoding="utf-8") as f:
            f.write(table)
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a", encoding="utf-8") as f:
            f.write(f"failed={', '.join(failed)}\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
