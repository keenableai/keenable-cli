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


@pytest.fixture(scope="module")
def domain_flags(kn):
    res = kn("search", "--help", key=False)
    if "--exclude-domain" not in res.out + res.err:
        pytest.skip("binary predates --exclude-domain")


def _assert_invalid_filter(message, entry):
    assert "Unknown parameter" not in message, f"server lacks domain filters: {message}"
    assert message.startswith(f'Invalid exclude_domains value "{entry}"'), message


def test_exclude_domains_blocklist(kn, domain_flags):
    for r in _non_empty_results(kn, "rust async", "--exclude-domain", "docs.rs"):
        assert not host_under(host_of(r["url"]), "docs.rs"), r["url"]


def test_exclude_domains_covers_subdomains(kn, domain_flags):
    for r in _non_empty_results(kn, "rust std library", "--exclude-domain", "rust-lang.org"):
        assert not host_under(host_of(r["url"]), "rust-lang.org"), r["url"]


def _assert_filter_rejected(res, entry):
    assert res.code == 1, res.out
    data = res.yaml()
    assert data["error"] == "invalid_filter", data
    assert "keenable login" not in data.get("hint", ""), data
    _assert_invalid_filter(data["message"], entry)


def test_bad_entry_errors_in_pretty(kn, domain_flags):
    res = kn("search", "x", "--exclude-domain", "docs.rs/releases", "-p")
    assert res.code == 1
    assert res.out == ""
    err = " ".join(re.sub(r"\x1b\[[0-9;]*m", "", res.err).split())
    assert 'Invalid exclude_domains value "docs.rs/releases"' in err, res.err
    assert "keenable login" not in err, res.err


@pytest.mark.parametrize("blank", ["", " "])
def test_blank_exclude_flag_is_rejected(kn, domain_flags, blank):
    _assert_filter_rejected(kn("search", "x", f"--exclude-domain={blank}"), "")


REJECTED_ENTRIES = [
    "*", ".com", ".gov", "*.gov", "*.docs.rs", "docs.rs/releases",
    "user@docs.rs", "127.0.0.1", "doc s.rs", "localhost", "co.uk", "docs.notarealtld",
]


@pytest.mark.parametrize("entry", REJECTED_ENTRIES)
def test_bad_exclude_entry_is_rejected_as_sent(kn, domain_flags, entry):
    _assert_filter_rejected(kn("search", "x", "--exclude-domain", entry), entry)


@pytest.mark.parametrize("host", ["github.io", "blogspot.com"])
def test_private_suffix_host_is_accepted(kn, domain_flags, host):
    res = kn("search", "async", "--exclude-domain", host)
    assert res.code == 0, res.out + res.err
    for r in res.yaml().get("results") or []:
        assert not host_under(host_of(r["url"]), host), r["url"]


def test_fixable_entries_are_cleaned_up(kn, domain_flags):
    res = kn("search", "rust async", "--exclude-domain", " HTTPS://WWW.Docs.RS:443/?a=1 ")
    assert res.code == 0, res.out + res.err
    for r in res.yaml().get("results") or []:
        assert not host_under(host_of(r["url"]), "docs.rs"), r["url"]


def _hosts(start, stop):
    return ",".join(f"d{i}.example.com" for i in range(start, stop))


def test_1000_exclude_hosts_are_accepted(kn, domain_flags):
    res = kn("search", "x", "--exclude-domain", _hosts(0, 1000))
    assert res.code == 0, res.out + res.err


def test_more_than_1000_exclude_hosts_is_rejected(kn, domain_flags):
    res = kn("search", "x", "--exclude-domain", _hosts(0, 1001))
    assert res.code == 1, res.out
    data = res.yaml()
    assert data["error"] == "invalid_filter", data
    assert data["message"].startswith("Invalid exclude_domains value"), data
    assert "1000" in data["message"].replace(",", ""), data


def test_limit_counts_hosts_after_cleanup(kn, domain_flags):
    hosts = _hosts(0, 1000) + ",WWW.D0.example.com,https://d1.example.com/"
    res = kn("search", "x", "--exclude-domain", hosts)
    assert res.code == 0, res.out + res.err


def test_malformed_date(kn):
    res = kn("search", "x", "--acquired-after", "notadate")
    assert res.code == 1
    data = res.yaml()
    # server wording drifted from "Invalid parameter" to "Bad request" (2026-06)
    assert data["error"] in ("Invalid parameter", "Bad request")
    assert "acquired_after" in data["message"]
    # A rejected parameter says to fix the request (not to log in) and carries the
    # API's request id for support.
    assert "retrying unchanged fails" in data["hint"]
    assert "keenable login" not in data["hint"]
    assert data["request_id"]


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
