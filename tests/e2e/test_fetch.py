"""T-23..T-29 — fetch. With a key (as here), every call is one POST /v1/fetch of 1 to 20 URLs."""

import uuid

import pytest
from urllib.parse import urlparse


# Body text of example.com. The page is not frozen: in Sep 2026 it dropped its
# <h1> and reworded this paragraph (was "...for use in illustrative examples in
# documents"). When it changes again, update this one line.
EXAMPLE_BODY = "This domain is for use in documentation examples"


def test_fetch_single_url(kn):
    res = kn("fetch", "https://example.com")
    assert res.code == 0
    data = res.yaml()
    assert data["title"] == "Example Domain"
    parsed_url = urlparse(data["url"])
    assert parsed_url.scheme == "https"
    assert parsed_url.hostname == "example.com"
    assert EXAMPLE_BODY in data["content"]
    assert "description" not in data


def test_fetch_live(kn):
    res = kn("fetch", "https://example.com", "--max-age-hours", "0")
    assert res.code == 0
    data = res.yaml()
    assert data["title"] == "Example Domain"
    assert EXAMPLE_BODY in data["content"]


def test_fetch_stored_copy_only(kn):
    # -1 reads only Keenable's stored copy. Whether example.com has one depends
    # on the index, so either the stored page or the not_stored error is right.
    res = kn("fetch", "https://example.com", "--max-age-hours", "-1")
    if res.code == 0:
        assert res.yaml()["title"] == "Example Domain"
    else:
        assert res.code == 8, res.out
        assert res.yaml()["code"] == "not_stored", res.out


def test_fetch_stored_copy_only_never_reads_live(kn):
    # A path nobody has crawled has no stored copy, and -1 never reads it live.
    url = f"https://example.com/keenable-cli-e2e-{uuid.uuid4().hex[:12]}"
    res = kn("fetch", url, "--max-age-hours", "-1")
    assert res.code == 8, res.out
    data = res.yaml()
    assert data["code"] == "not_stored", data
    assert "max_age_hours 0" in data["hint"], data


@pytest.mark.semantic  # asserts live LLM extraction output
def test_fetch_prompt(kn):
    # Ask for something the page text literally states. The original prompt asked
    # for the domain name, which appears nowhere in the markdown — the model has
    # to infer it from the URL, and once answered "The content does not contain
    # the page's domain name." That reply was still valid extraction output, so
    # the test failed on a detail it was never meant to pin down. The page title
    # went the same way once example.com dropped its <h1>: the model no longer
    # sees it and answered "The page content does not contain a title."
    res = kn("fetch", "https://example.com", "--prompt", "What does the page say not to rely on it for? Answer in a few words.")
    assert res.code == 0
    data = res.yaml()
    parsed_url = urlparse(data["url"])
    assert parsed_url.scheme == "https"
    assert parsed_url.hostname == "example.com"
    # LLM extraction replaces the full page with the instruction's output. The
    # last assert is the load-bearing one: a stale daemon that drops `prompt`
    # returns the full page with exit code 0.
    assert "testing" in data["content"].lower()
    assert "monitoring" in data["content"].lower()
    assert EXAMPLE_BODY not in data["content"]


def test_fetch_max_chars(kn):
    res = kn("fetch", "https://example.com", "--max-chars", "50")
    assert res.code == 0
    data = res.yaml()
    # The cap is the load-bearing assert: a stale daemon that drops
    # `max_chars` returns the full page with exit code 0. The API used to
    # append a ~70-character notice; it now cuts to at most `max_chars`
    # characters ending in "..." and sets `truncated` (keenable-backend-ts#691).
    # Accept both until that change is deployed; remove the notice branch
    # then (keenable-cli#95). Branch on the notice, not on `truncated`: since
    # 0.3.0 the CLI adds `truncated: true` itself when it sees the old notice.
    content = data["content"]
    assert data["truncated"] is True
    if "truncated to stay below 50 characters" in content:
        return
    assert len(content) <= 50
    assert content.endswith("...")
    assert data["total_chars"] > 50


def test_fetch_max_chars_rejects_zero(kn):
    res = kn("fetch", "https://example.com", "--max-chars", "0")
    assert res.code == 2
    assert "invalid value" in res.err


def test_pretty_fetch(kn):
    res = kn("fetch", "https://example.com", "-p")
    assert res.code == 0
    assert res.out == ""
    assert "Example Domain" in res.err


def test_live_flag_is_gone(kn):
    res = kn("fetch", "https://example.com", "--live")
    assert res.code == 2
    assert "unexpected argument" in res.err


def test_multiple_urls(kn):
    res = kn("fetch", "https://example.com", "https://www.iana.org/nonexistent-xyz-12345")
    # One page read, one failed: the call succeeds and lists both, in order.
    assert res.code == 0
    results = res.yaml()["results"]
    assert len(results) == 2
    assert results[0]["title"] == "Example Domain"
    assert EXAMPLE_BODY in results[0]["content"]
    assert results[1]["url"] == "https://www.iana.org/nonexistent-xyz-12345"
    assert results[1]["error"]["status"] == 404


def test_multiple_urls_pretty(kn):
    res = kn("fetch", "https://example.com", "https://example.org", "-p")
    assert res.code == 0
    assert res.out == ""
    assert "[1/2]" in res.err and "[2/2]" in res.err


def test_every_url_failed(kn):
    res = kn("fetch", "https://www.iana.org/nonexistent-xyz-1", "https://www.iana.org/nonexistent-xyz-2")
    assert res.code == 6  # the most common page error: not found
    results = res.yaml()["results"]
    assert [r["error"]["status"] for r in results] == [404, 404]
    assert "all 2 URLs failed" in res.err


def test_more_than_20_urls_rejected(kn):
    urls = [f"https://example.com/{i}" for i in range(21)]
    res = kn("fetch", *urls)
    assert res.code == 2


def test_duplicate_urls_rejected(kn):
    res = kn("fetch", "https://example.com", "https://example.com")
    assert res.code == 8  # invalid request
    assert "send each URL once" in res.yaml()["message"]


def test_fetch_links_and_max_age(kn):
    res = kn("fetch", "https://www.iana.org/", "--links", "5", "--max-age-hours", "24")
    assert res.code == 0
    data = res.yaml()
    assert 0 < len(data["links"]) <= 5
    assert all("url" in link for link in data["links"])
    assert isinstance(data["stale"], bool)


@pytest.mark.semantic  # asserts live LLM structured output
def test_fetch_schema(kn, tmp_path):
    schema = tmp_path / "schema.json"
    schema.write_text('{"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}')
    res = kn("fetch", "https://example.com", "--schema", f"@{schema}")
    assert res.code == 0
    data = res.yaml()
    assert "example" in data["structured"]["title"].lower(), data


def test_fetch_schema_must_be_an_object(kn):
    res = kn("fetch", "https://example.com", "--schema", "[1]")
    assert res.code == 2
    assert "JSON object" in res.err


def test_unparseable_url(kn):
    res = kn("fetch", "not-a-url")
    assert res.code == 8  # invalid request
    data = res.yaml()
    assert data["error"] == "Bad request"
    assert "Could not parse the provided URL" in data["message"]


def test_dead_page(kn):
    # Not example.com: since Oct 2026 it serves its page on every path,
    # so a made-up path there is no longer a dead page.
    res = kn("fetch", "https://www.iana.org/nonexistent-xyz-12345")
    assert res.code == 6  # the page failed, not the request
    data = res.yaml()
    assert data["error"] == "Not found"


def test_invalid_key_on_fetch(kn):
    res = kn("fetch", "https://example.com", "--api-key", "keen_bad", key=False)
    assert res.code == 3  # authentication
    data = res.yaml()
    assert data["error"] == "Authentication failed"
    # Server distinguishes malformed keys from well-formed-but-unknown ones.
    assert data["message"] in ("Invalid API key", "Malformed API key")


def test_no_url_arg(kn):
    res = kn("fetch")
    assert res.code == 2
    assert "the following required arguments were not provided" in res.err
    assert "<URL>" in res.err
