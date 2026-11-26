"""monitors — the Monitors API (/v1/monitors) through the CLI, against the live API.

Read-only calls (list, stats, an unknown id), the keyless refusal, and one
full lifecycle: create → get → results → pause → resume → delete. The lifecycle monitor
has no schedule and no webhook, so it never runs or sends anything, and it is
deleted at the end (monitors are free; an organization holds up to 100 active
ones). Monitors left behind by an interrupted run carry the same name prefix
and are deleted before the next create.
"""

from conftest import E2E_PREFIX, e2e_name


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

    name = e2e_name()
    res = kn("monitors", "create", "new Rust compiler releases", "--name", name,
             "--max-results", "3")
    # A full organization fails loudly instead of skipping: a silent skip would
    # hide that the lifecycle stopped being tested. Free some monitors (pause or
    # delete leftovers) in the organization that owns KEENABLE_API_KEY.
    assert "monitor_limit_reached" not in res.out, (
        "the organization behind KEENABLE_API_KEY is at its active-monitor limit "
        "(100); pause or delete unused monitors there")
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
        for gone_field in ("agent_run_id", "backfill", "memory_scope",
                           "estimated_cost_usd", "cost_this_month_usd"):
            assert gone_field not in got.yaml(), gone_field

        # A new monitor has no results yet; the list shape is still there.
        results = kn("monitors", "results", mon_id)
        assert results.code == 0, (results.out, results.err)
        assert results.yaml()["data"] == []
        assert results.yaml()["has_more"] is False

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
