"""T-30..T-36 — feedback. Valid feedback requires a recent search for the
same query with the same key, so tests depend on the basic_search fixture."""

import pytest

QUERY = "rust async patterns"  # matches basic_search


def test_t30_valid_feedback(kn, basic_search):
    res = kn("feedback", QUERY, "https://tokio.rs=5=great overview")
    assert res.code == 0, res.out + res.err
    data = res.yaml()
    assert data["status"] == "ok"


@pytest.mark.xfail(
    strict=True,
    reason="BUG: --help says comment is optional, but the CLI always sends "
    'comment:"" and the server rejects empty comments '
    '(\'"relevance": each entry must have a non-empty "comment" string\'). '
    "Fix: CLI should omit the comment field when empty.",
)
def test_t31_score_without_comment(kn, basic_search):
    res = kn("feedback", QUERY, "https://tokio.rs=4")
    assert res.code == 0, res.out + res.err


def test_t32_multiple_scores(kn, basic_search):
    res = kn("feedback", QUERY, "https://tokio.rs=5=good", "https://example.com=1=off topic")
    assert res.code == 0, res.out + res.err
    assert res.yaml()["status"] == "ok"


def test_t33_feedback_for_unsearched_query(kn):
    res = kn("feedback", "never searched this xyz qqq", "https://x.com=5=test comment")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Bad request"
    assert "does not match any recent search" in data["message"]


def test_t34_out_of_range_score(kn):
    res = kn("feedback", QUERY, "https://tokio.rs=9")
    assert res.code == 1
    assert "Invalid score in 'https://tokio.rs=9'. Must be 0-5." in res.err


def test_t35_no_scores(kn, basic_search):
    res = kn("feedback", QUERY)
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid parameter"
    assert "between 1 and 50 entries" in data["message"]


def test_t36_malformed_score_entry(kn):
    res = kn("feedback", QUERY, "no-equals-sign")
    assert res.code == 1
    assert "Invalid format: no-equals-sign" in res.err
