"""T-30..T-36 — feedback. Valid feedback requires a recent search for the
same query with the same key, so tests depend on the basic_search fixture.

Success-path tests persist real feedback and carry the write_feedback opt-in
gate; the error-path tests are rejected server- or client-side and are safe.
"""

from conftest import SEARCH_QUERY as QUERY  # must match basic_search's query
from conftest import write_feedback


@write_feedback
def test_valid_feedback(kn, basic_search):
    res = kn("feedback", QUERY, "https://tokio.rs=5=synthetic e2e feedback, ignore")
    assert res.code == 0, res.out + res.err
    data = res.yaml()
    assert data["status"] == "ok"


def test_score_without_comment_rejected(kn):
    res = kn("feedback", QUERY, "https://tokio.rs=4")
    assert res.code == 1
    # ui::error word-wraps to terminal width, so assert wrap-safe fragments.
    assert "Invalid format: https://tokio.rs=4" in res.err
    assert "url=score=comment" in res.err


@write_feedback
def test_multiple_scores(kn, basic_search):
    res = kn("feedback", QUERY,
             "https://tokio.rs=5=synthetic e2e feedback, ignore",
             "https://example.com=1=synthetic e2e feedback, ignore")
    assert res.code == 0, res.out + res.err
    assert res.yaml()["status"] == "ok"


def test_feedback_for_unsearched_query(kn):
    res = kn("feedback", "never searched this xyz qqq", "https://x.com=5=test comment")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Bad request"
    assert "does not match any recent search" in data["message"]


def test_out_of_range_score(kn):
    res = kn("feedback", QUERY, "https://tokio.rs=9=too good")
    assert res.code == 1
    assert "Invalid score in 'https://tokio.rs=9=too good'. Must be 0-5." in res.err


def test_no_scores(kn, basic_search):
    res = kn("feedback", QUERY)
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Invalid parameter"
    assert "between 1 and 50 entries" in data["message"]


def test_malformed_score_entry(kn):
    res = kn("feedback", QUERY, "no-equals-sign")
    assert res.code == 1
    assert "Invalid format: no-equals-sign" in res.err
