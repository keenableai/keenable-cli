"""S-01..S-09 — semantic/relevance checks on real recent dev queries.

Structure tests prove the pipe works; these prove results are *relevant*.
Assertions are presence-in-top-k, never exact rank (rank belongs to the
quality dashboards, not a smoke suite).
"""

import os
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from conftest import host_of, host_under, results_of, search_results

pytestmark = pytest.mark.semantic


def blob(result) -> str:
    return " ".join(str(result.get(f) or "") for f in ("title", "description", "snippet"))


def test_gold_fact_mozart(kn):
    results = search_results(kn, "Wolfgang Amadeus Mozart birth year")
    assert any("1756" in blob(r) for r in results[:5]), \
        "no top-5 result contains the answer 1756"


def test_gold_fact_sargeson(kn):
    results = search_results(kn, "Frank Sargeson death year")
    assert any("1982" in blob(r) for r in results[:5]), \
        "no top-5 result contains the answer 1982"


def test_authority_domain_langchain(kn):
    # Official source = langchain.com/.dev or the github.com/langchain-ai org
    # (observed live 2026-06-06: official repo at rank 3, langchain.com at 6).
    top5 = search_results(kn, "langchain")[:5]
    assert any(
        host_under(host_of(r["url"]), "langchain.com")
        or host_under(host_of(r["url"]), "langchain.dev")
        or (host_under(host_of(r["url"]), "github.com") and "/langchain-ai/" in r["url"])
        for r in top5
    ), f"no official langchain source in top-5: {[r['url'] for r in top5]}"


def test_repo_seeking_query(kn):
    results = search_results(kn, "agno agent framework github")
    assert any(host_under(host_of(r["url"]), "github.com") and "agno" in r["url"].lower()
               for r in results[:5]), \
        f"no top-5 github.com/agno result: {[r['url'] for r in results[:5]]}"


def test_inline_site_operator(kn):
    results = search_results(kn, "site:github.com OpenHands open source AI agent")
    assert results
    for r in results:
        assert host_under(host_of(r["url"]), "github.com"), r["url"]


def test_site_flag_vs_inline_operator_parity(kn):
    query = "OpenHands open source AI agent"
    flag_results = search_results(kn, query, "--site", "github.com")
    inline_results = search_results(kn, f"site:github.com {query}")
    for r in flag_results + inline_results:
        assert host_under(host_of(r["url"]), "github.com"), r["url"]

    flag_urls, inline_urls = {r["url"] for r in flag_results}, {r["url"] for r in inline_results}
    jaccard = len(flag_urls & inline_urls) / len(flag_urls | inline_urls)
    print(f"\nFlag/inline Jaccard: {jaccard:.2f}")
    # Live overlap fluctuates (0.43 observed 2026-06-06); the host assertions
    # above are the hard gate — this bound only catches total path divergence.
    assert jaccard >= 0.3, f"flag and inline site: paths diverged (Jaccard {jaccard:.2f})"


def test_entity_navigational(kn):
    results = search_results(kn, "Keenable AI search")
    top5 = [host_of(r["url"]) for r in results[:5]]
    assert any(host_under(h, "keenable.ai") for h in top5), \
        f"keenable.ai not in top-5 hosts: {top5}"


def _recall_search(kn, query, attempts=4):
    """Run one recall query, retrying on rate-limit throttling.

    Recall measures whether real queries return results, not whether the
    pooled fan-out below stays under the API's per-second budget. A throttled
    query is a transient 429, not a recall miss, so back off and retry a few
    times before letting `search_results` assert on a non-zero exit.
    """
    for i in range(attempts - 1):
        res = kn("search", query)
        if res.code == 0:
            return results_of(res.yaml())
        if "rate limit" not in (res.out + res.err).lower():
            break
        time.sleep(0.5 * (i + 1))
    return search_results(kn, query)


def test_non_empty_recall_rate(kn):
    seed_file = Path(os.environ.get("KEENABLE_SEED_QUERIES",
                                    Path(__file__).parent / "seed_queries.txt"))
    queries = [q.strip() for q in seed_file.read_text().splitlines()
               if q.strip() and not q.startswith("#")]
    assert len(queries) >= 10, "seed corpus too small to be meaningful"

    # Modest fan-out: a wide burst trips the API's per-second rate limit, and
    # _recall_search backs off on the throttling that slips through.
    with ThreadPoolExecutor(max_workers=4) as pool:
        non_empty = sum(1 for results in pool.map(lambda q: _recall_search(kn, q), queries) if results)
    rate = non_empty / len(queries)
    print(f"\nNon-empty recall: {non_empty}/{len(queries)} = {rate:.0%}")
    assert rate >= 0.9, f"recall rate {rate:.0%} below 90% (n={len(queries)})"


def test_cross_mode_self_consistency(kn):
    # Navigational query: the obvious top answer shouldn't flip between modes.
    top_hosts = {}
    for mode in ("realtime", "pro"):
        results = search_results(kn, "langchain", "--mode", mode)
        assert results, f"no results in {mode} mode"
        top_hosts[mode] = host_of(results[0]["url"])
    assert top_hosts["realtime"] == top_hosts["pro"], f"top-1 host flipped: {top_hosts}"
