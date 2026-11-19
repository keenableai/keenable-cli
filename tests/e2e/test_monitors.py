"""monitors — the Monitors API (/v1/monitors) through the CLI, against the live API.

Read-only calls (list, stats, an unknown id), the keyless refusal, and one
full lifecycle: create → get → pause → resume → delete. The lifecycle monitor
has no schedule and no webhook, so it never runs or sends anything, and it is
deleted at the end (monitors are free; an organization holds up to 100 active
ones). Monitors left behind by an interrupted run carry the same name prefix
and are deleted before the next create.
"""

import uuid

import pytest

# Every monitor this suite creates is named with this prefix.
E2E_PREFIX = "keenable-cli-e2e-"


def test_monitors_need_a_key(kn_fresh):
    # No stored key, no --api-key: refused before any request.
    res = kn_fresh("monitors", "list", key=False)
    assert res.code == 3, (res.out, res.err)
    data = res.yaml()
    assert data["type"] == "authentication_error"
    assert "keenable login" in data["hint"]
    assert res.err.startswith("error: ")


def test_monitors_list(kn):
    res = kn("monitors", "list", "--limit", "5")
    assert res.code == 0, res.err
    data = res.yaml()
    assert isinstance(data["data"], list)
    assert len(data["data"]) <= 5
    assert "has_more" in data
    for m in data["data"]:
        assert m["id"].startswith("mon_"), m
        assert m["object"] == "monitor"


def test_monitors_stats(kn):
    res = kn("monitors", "stats")
    assert res.code == 0, res.err
    data = res.yaml()
    assert isinstance(data["total"], int)
    assert set(data["by_status"]) >= {"active", "paused"}
    assert isinstance(data["by_schedule"], dict)


def test_monitors_get_unknown_id(kn):
    res = kn("monitors", "get", "mon_000000000000000000000000")
    assert res.code == 8, (res.out, res.err)  # invalid request
    data = res.yaml()
    assert data["code"] == "not_found"
    assert data["request_id"]


def _delete_leftovers(kn):
    """Delete monitors an interrupted earlier run left behind."""
    res = kn("monitors", "list", "--name", E2E_PREFIX, "--limit", "100")
    assert res.code == 0, res.err
    for m in res.yaml()["data"]:
        if (m.get("name") or "").startswith(E2E_PREFIX):
            kn("monitors", "delete", m["id"], "--yes")


def test_monitor_lifecycle(kn):
    _delete_leftovers(kn)

    name = E2E_PREFIX + uuid.uuid4().hex[:12]
    res = kn("monitors", "create", "new Rust compiler releases", "--name", name,
             "--max-results", "3")
    if res.code != 0 and "monitor_limit_reached" in res.out:
        pytest.skip("the CI organization is at its active-monitor limit")
    assert res.code == 0, (res.out, res.err)
    created = res.yaml()
    mon_id = created["id"]
    try:
        assert mon_id.startswith("mon_")
        assert created["name"] == name
        assert created["status"] == "active"
        assert created["search"]["query"] == "new Rust compiler releases"
        assert created["search"]["max_results"] == 3
        assert created["schedule"] is None

        got = kn("monitors", "get", mon_id)
        assert got.code == 0, got.err
        assert got.yaml()["id"] == mon_id

        paused = kn("monitors", "pause", mon_id)
        assert paused.code == 0, paused.err
        assert paused.yaml()["status"] == "paused"

        resumed = kn("monitors", "resume", mon_id)
        assert resumed.code == 0, resumed.err
        assert resumed.yaml()["status"] == "active"
    finally:
        deleted = kn("monitors", "delete", mon_id, "--yes")
        assert deleted.code == 0, (deleted.out, deleted.err)

    gone = kn("monitors", "get", mon_id)
    assert gone.code == 8, (gone.out, gone.err)
    assert gone.yaml()["code"] == "not_found"
