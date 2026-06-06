"""T-01..T-22 — search: core, modes, filters, error/edge cases."""

from datetime import datetime, timedelta, timezone

import pytest

from conftest import SEARCH_QUERY, host_of, host_under, parse_ts, results_of, search_results, utc

RESULT_FIELDS = ("url", "title", "description", "snippet", "acquired_at", "published_at")


# --- 2.1 core ---

def test_basic_search(basic_search):
    assert basic_search["query"] == SEARCH_QUERY
    assert basic_search["mode"] in ("realtime", "pro")
    results = results_of(basic_search)
    assert results
    for r in results:
        for field in RESULT_FIELDS:
            assert field in r, f"result missing {field}: {list(r)}"


def test_pretty_output(kn):
    res = kn("search", "rust async", "-p")
    assert res.code == 0
    # Human output goes to stderr; stdout stays clean for machine output.
    assert res.out == ""
    assert "keenable search" in res.err
    assert "http" in res.err


def test_result_schema(basic_search):
    for r in results_of(basic_search):
        assert r["url"].startswith(("http://", "https://"))
        assert isinstance(r["title"], str) and r["title"]
        parse_ts(r["acquired_at"])
        if r["published_at"] is not None:
            parse_ts(r["published_at"])


def test_result_count(basic_search):
    count = len(results_of(basic_search))
    assert count >= 1
    print(f"\nResult count for 'rust async patterns': {count}")


# --- 2.2 modes ---

def test_mode_realtime(kn):
    data = kn("search", "test query", "--mode", "realtime").yaml()
    assert data["mode"] == "realtime"
    assert "results" in data


def test_mode_pro(kn):
    data = kn("search", "test query", "--mode", "pro").yaml()
    assert data["mode"] == "pro"
    assert "results" in data


def test_no_mode_flag_uses_server_default(kn):
    # Since 0.1.16 the CLI sends no mode and the server default applies
    # (including org-level overrides) — so assert a valid mode, don't pin one.
    data = kn("search", "test query").yaml()
    assert data["mode"] in ("realtime", "pro")
    print(f"\nServer default mode: {data['mode']}")


def test_invalid_mode(kn):
    res = kn("search", "x", "--mode", "bogus")
    assert res.code == 1
    assert 'Invalid mode "bogus". Must be "realtime" or "pro".' in res.err


def test_mode_standard_legacy_alias(kn):
    # "standard" is a documented legacy alias that maps to realtime.
    data = kn("search", "x", "--mode", "standard").yaml()
    assert data["mode"] == "realtime"


# --- 2.3 filters ---

def _non_empty_results(kn, *args):
    results = search_results(kn, *args)
    if not results:
        pytest.skip(f"no results for {args} — filter assertion would be vacuous")
    return results


def test_site_filter(kn):
    for r in _non_empty_results(kn, "async", "--site", "docs.rs"):
        assert host_under(host_of(r["url"]), "docs.rs"), r["url"]


def test_acquired_after(kn):
    for r in _non_empty_results(kn, "AI news", "--acquired-after", "2026-01-01"):
        assert parse_ts(r["acquired_at"]) >= utc(2026, 1, 1), r["url"]


def test_acquired_before(kn):
    for r in _non_empty_results(kn, "AI news", "--acquired-before", "2026-05-01"):
        assert parse_ts(r["acquired_at"]) <= utc(2026, 5, 1), r["url"]


def test_published_after(kn):
    for r in _non_empty_results(kn, "AI news", "--published-after", "2026-01-01"):
        if r["published_at"] is not None:
            assert parse_ts(r["published_at"]) >= utc(2026, 1, 1), r["url"]


def test_published_before(kn):
    for r in _non_empty_results(kn, "rust async", "--published-before", "2024-01-01"):
        if r["published_at"] is not None:
            assert parse_ts(r["published_at"]) <= utc(2024, 1, 1), r["url"]


def test_relative_date(kn):
    cutoff = datetime.now(timezone.utc) - timedelta(days=7, hours=1)  # 1h skew allowance
    for r in _non_empty_results(kn, "AI news", "--acquired-after", "7d"):
        assert parse_ts(r["acquired_at"]) >= cutoff, r["url"]


def test_iso8601_datetime(kn):
    instant = utc(2026, 1, 15).replace(hour=10, minute=30)
    for r in _non_empty_results(kn, "AI news", "--acquired-after", "2026-01-15T10:30:00Z"):
        assert parse_ts(r["acquired_at"]) >= instant, r["url"]


def test_combined_filters(kn):
    for r in _non_empty_results(kn, "async", "--site", "docs.rs", "--acquired-after", "2025-01-01"):
        assert host_under(host_of(r["url"]), "docs.rs"), r["url"]
        assert parse_ts(r["acquired_at"]) >= utc(2025, 1, 1), r["url"]


def test_malformed_date(kn):
    res = kn("search", "x", "--acquired-after", "notadate")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid parameter"
    assert "acquired_after" in data["message"]


# --- 2.4 error/edge cases ---

def test_missing_query_arg(kn):
    res = kn("search")
    assert res.code == 2
    assert "the following required arguments were not provided" in res.err
    assert "<QUERY>" in res.err


def test_empty_query(kn):
    res = kn("search", "")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Missing required parameter"
    assert '"query" is required' in data["message"]


def test_invalid_api_key(kn):
    res = kn("search", "x", "--api-key", "keen_bad_key", key=False)
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Authentication failed"
    assert data["message"] == "Invalid API key"
    assert "keenable login" in data["hint"]


def test_unicode_query(kn):
    query = "café ☕ münchen"
    res = kn("search", query)
    assert res.code == 0
    assert res.yaml()["query"] == query


def test_inline_date_operator(kn):
    for r in _non_empty_results(kn, "python asyncio acquired_after:2026-01-01"):
        assert parse_ts(r["acquired_at"]) >= utc(2026, 1, 1), r["url"]
