"""T-01..T-22 — search: core, modes, filters, error/edge cases."""

import re
from datetime import datetime, timedelta, timezone

import pytest

from conftest import SEARCH_QUERY, host_of, host_under, parse_ts, results_of, search_results, utc

# published_at is omitted (not nulled) for pages with no known publish date
RESULT_FIELDS = ("url", "title", "snippet", "acquired_at")


# --- 2.1 core ---

def test_basic_search(basic_search):
    assert basic_search["query"] == SEARCH_QUERY
    assert basic_search["mode"] in ("realtime", "pro")
    results = results_of(basic_search)
    assert results
    for r in results:
        for field in RESULT_FIELDS:
            assert field in r, f"result missing {field}: {list(r)}"
        assert "description" not in r


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
        if r.get("published_at") is not None:
            parse_ts(r["published_at"])


def test_result_count(basic_search):
    count = len(results_of(basic_search))
    assert count >= 1
    print(f"\nResult count for 'rust async patterns': {count}")


def test_snippet_max_length(kn):
    def avg_snippet(data):
        lens = [len(r.get("snippet") or "") for r in results_of(data)]
        return sum(lens) / len(lens)

    short = kn("search", SEARCH_QUERY, "--snippet-max-length", "180").yaml()
    long = kn("search", SEARCH_QUERY, "--snippet-max-length", "5000").yaml()
    # Caps overshoot by ~10% (applied per-fragment upstream), so assert loose
    # bounds far apart instead of exact limits. A dropped param would give
    # both runs the same default length and fail one of the two.
    assert avg_snippet(short) < 500
    assert avg_snippet(long) > 1000


def test_snippet_max_length_out_of_range(kn):
    res = kn("search", SEARCH_QUERY, "--snippet-max-length", "50")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid parameter"
    assert "snippet_max_length" in data["message"]


def test_max_results(kn):
    few = kn("search", SEARCH_QUERY, "--max-results", "3").yaml()
    many = kn("search", SEARCH_QUERY, "--max-results", "30").yaml()
    assert len(results_of(few)) <= 3
    # A dropped param would give both runs the default count (10).
    assert len(results_of(many)) > 10


def test_max_results_out_of_range(kn):
    res = kn("search", SEARCH_QUERY, "--max-results", "51")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid parameter"
    assert "max_results" in data["message"]


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
    # A date-only cutoff is inclusive of that whole day (a bare 2026-05-01 keeps
    # 2026-05-01T14:31Z), so the bound is the start of the next day. Pass a full
    # timestamp to cut at an exact instant.
    for r in _non_empty_results(kn, "AI news", "--acquired-before", "2026-05-01"):
        assert parse_ts(r["acquired_at"]) < utc(2026, 5, 2), r["url"]


def test_published_after(kn):
    for r in _non_empty_results(kn, "AI news", "--published-after", "2026-01-01"):
        if r.get("published_at") is not None:
            assert parse_ts(r["published_at"]) >= utc(2026, 1, 1), r["url"]


def test_published_before(kn):
    # Same whole-day-inclusive cutoff as test_acquired_before.
    for r in _non_empty_results(kn, "rust async", "--published-before", "2024-01-01"):
        if r.get("published_at") is not None:
            assert parse_ts(r["published_at"]) < utc(2024, 1, 2), r["url"]


def test_relative_date(kn):
    cutoff = datetime.now(timezone.utc) - timedelta(days=7, hours=1)  # 1h skew allowance
    for r in _non_empty_results(kn, "AI news", "--acquired-after", "7d"):
        assert parse_ts(r["acquired_at"]) >= cutoff, r["url"]


def test_iso8601_datetime(kn):
    instant = utc(2026, 1, 15).replace(hour=10, minute=30)
    for r in _non_empty_results(kn, "AI news", "--acquired-after", "2026-01-15T10:30:00Z"):
        assert parse_ts(r["acquired_at"]) >= instant, r["url"]


def test_query_time(kn):
    for r in _non_empty_results(kn, "AI news", "--query-time", "2026-05-01T00:00:00Z"):
        assert parse_ts(r["acquired_at"]) <= utc(2026, 5, 1), r["url"]


def test_combined_filters(kn):
    for r in _non_empty_results(kn, "async", "--site", "docs.rs", "--acquired-after", "2025-01-01"):
        assert host_under(host_of(r["url"]), "docs.rs"), r["url"]
        assert parse_ts(r["acquired_at"]) >= utc(2025, 1, 1), r["url"]


# --- 2.3b domain allow-list / block-list (keenableai/keenable-sdk#10) ---

ALLOWLIST = ("docs.rs", "doc.rust-lang.org", "rust-lang.github.io")


@pytest.fixture(scope="module")
def domain_flags(kn):
    """Skip on released binaries that predate the domain flags."""
    res = kn("search", "--help", key=False)
    if "--include-domain" not in res.out + res.err:
        pytest.skip("binary predates --include-domain/--exclude-domain")


def _assert_invalid_filter(message, field, entry):
    """Assert the API's invalid_filter wording, not an unknown-parameter 400."""
    assert "Unknown parameter" not in message, f"server lacks domain filters: {message}"
    assert message.startswith(f'Invalid {field} value "{entry}"'), message


def test_include_domains_allowlist(kn, domain_flags):
    # Repeated flag + comma list together: all three must reach the server.
    results = _non_empty_results(
        kn, "async", "--include-domain", ",".join(ALLOWLIST[:2]),
        "--include-domain", ALLOWLIST[2],
    )
    for r in results:
        host = host_of(r["url"])
        assert any(host_under(host, d) for d in ALLOWLIST), r["url"]


def test_exclude_domains_blocklist(kn, domain_flags):
    for r in _non_empty_results(kn, "rust async", "--exclude-domain", "docs.rs"):
        assert not host_under(host_of(r["url"]), "docs.rs"), r["url"]


def test_include_and_exclude_combined(kn, domain_flags):
    # Exclude wins: a subdomain carved out of an allowed parent stays out.
    results = _non_empty_results(
        kn, "rust", "--include-domain", "rust-lang.org",
        "--exclude-domain", "doc.rust-lang.org",
    )
    for r in results:
        host = host_of(r["url"])
        assert host_under(host, "rust-lang.org"), r["url"]
        assert not host_under(host, "doc.rust-lang.org"), r["url"]


def test_domain_conflict_errors(kn, domain_flags):
    # The same entry in both lists is rejected by the server.
    res = kn("search", "x", "--include-domain", "docs.rs", "--exclude-domain", "docs.rs")
    assert res.code == 1
    data = res.yaml()
    assert "error" in data
    _assert_invalid_filter(data["message"], "exclude_domains", "docs.rs")


def test_site_with_include_domain_errors(kn, domain_flags):
    res = kn("search", "x", "--site", "docs.rs", "--include-domain", "crates.io")
    assert res.code == 1
    data = res.yaml()
    assert "error" in data
    _assert_invalid_filter(data["message"], "site", "docs.rs")


# Inline positive site: plus --include-domain always drops the site: token with a warning.
INLINE_SITE_ARGS = ("search", "site:docs.rs async", "--include-domain", "crates.io")


def test_inline_site_with_include_domain_warns_in_yaml(kn, domain_flags):
    res = kn(*INLINE_SITE_ARGS)
    assert res.code == 0, res.err
    data = res.yaml()
    assert isinstance(data.get("warning"), str) and data["warning"].strip(), data


def test_inline_site_with_include_domain_warns_in_pretty(kn, domain_flags):
    warning = kn(*INLINE_SITE_ARGS).yaml()["warning"]
    res = kn(*INLINE_SITE_ARGS, "-p")
    assert res.code == 0
    assert res.out == ""
    # ui::warning word-wraps and may color each line, so compare plain, joined text.
    err = re.sub(r"\x1b\[[0-9;]*m", "", res.err)
    assert " ".join(warning.split()) in " ".join(err.split())


def test_malformed_date(kn):
    res = kn("search", "x", "--acquired-after", "notadate")
    assert res.code == 1
    data = res.yaml()
    # server wording drifted from "Invalid parameter" to "Bad request" (2026-06)
    assert data["error"] in ("Invalid parameter", "Bad request")
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
