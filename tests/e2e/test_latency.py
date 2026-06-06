"""L-01..L-06 — wall-clock latency by mode.

The CLI exposes no timing field, so latency is wall-clock around the process —
including startup, DNS, TLS, and network. Bounds are therefore loose and
tail-focused (the pro gate-miss tail can legitimately hit 5-10s; we bound how
OFTEN, not whether). Override bounds per environment via KEENABLE_LAT_* env
vars. Never assert on a single sample — always median/p90 with n.
"""

import os
import time
from statistics import median

import pytest

pytestmark = pytest.mark.latency

QUERIES = [
    "langchain",
    "Wolfgang Amadeus Mozart birth year",
    "rust async patterns",
    "kubernetes pod restart policy",
    "Into the Woods cast",
]
REPS = 4  # 4 reps x 5 queries = n=20 per mode

REALTIME_MEDIAN_MS = int(os.environ.get("KEENABLE_LAT_REALTIME_MEDIAN_MS", 1500))
PRO_MEDIAN_MS = int(os.environ.get("KEENABLE_LAT_PRO_MEDIAN_MS", 2000))
PRO_P90_MS = int(os.environ.get("KEENABLE_LAT_PRO_P90_MS", 8000))


def timed_ms(kn, *args) -> float:
    start = time.perf_counter()
    res = kn("search", *args)
    elapsed = (time.perf_counter() - start) * 1000
    assert res.code == 0, res.out + res.err
    return elapsed


def p90(samples) -> float:
    return sorted(samples)[int(len(samples) * 0.9) - 1]


def report(name, samples) -> str:
    return (f"{name}: median={median(samples):.0f}ms p90={p90(samples):.0f}ms "
            f"max={max(samples):.0f}ms n={len(samples)}")


@pytest.fixture(scope="module")
def samples(kn):
    # Warm up process/connection path; discard.
    for _ in range(2):
        timed_ms(kn, "warmup query", "--mode", "realtime")
    data = {mode: [timed_ms(kn, q, "--mode", mode) for _ in range(REPS) for q in QUERIES]
            for mode in ("realtime", "pro")}
    print("\n" + "\n".join(report(m, s) for m, s in data.items()))
    return data


def test_l01_realtime_median(samples):
    assert median(samples["realtime"]) <= REALTIME_MEDIAN_MS, report("realtime", samples["realtime"])


def test_l02_pro_median(samples):
    assert median(samples["pro"]) <= PRO_MEDIAN_MS, report("pro", samples["pro"])


def test_l03_pro_tail(samples):
    pro = samples["pro"]
    over_5s = sum(1 for s in pro if s > 5000) / len(pro)
    print(f"\nL-03 pro calls >5s: {over_5s:.0%}")
    assert p90(pro) <= PRO_P90_MS, report("pro", pro)
    # The gate-miss tail exists by design; bound its frequency, not its existence.
    assert over_5s <= 0.2, f"{over_5s:.0%} of pro calls exceeded 5s (gate firing less often?)"


def test_l04_relative_mode_behavior(samples):
    # Relative bound survives environment changes better than absolute ms;
    # the 1s floor avoids false alarms when realtime medians are tiny.
    bound = max(3 * median(samples["realtime"]), 1000)
    assert median(samples["pro"]) <= bound, \
        f"median(pro)={median(samples['pro']):.0f}ms > {bound:.0f}ms"


def test_l05_cache_warm_path(kn):
    # Soft check: log the cold/warm delta, never fail — the public CLI may not
    # hit the same cache layer as the internal orchestrator.
    cold = timed_ms(kn, "redis cluster failover details", "--mode", "realtime")
    warm = timed_ms(kn, "redis cluster failover details", "--mode", "realtime")
    print(f"\nL-05 cold={cold:.0f}ms warm={warm:.0f}ms delta={warm - cold:+.0f}ms")
    if warm > cold:
        pytest.skip(f"warm call slower than cold ({warm:.0f}ms > {cold:.0f}ms) — logged, not failed")


def test_l06_no_result_query_latency(kn):
    # Empty results shouldn't hang; best-of-3 against the realtime bound.
    best = min(timed_ms(kn, "zxqwvbnm qwerty asdfgh nonsense") for _ in range(3))
    assert best <= REALTIME_MEDIAN_MS, f"gibberish query best-of-3 {best:.0f}ms"
