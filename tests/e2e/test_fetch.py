"""T-23..T-29 — fetch. Single-URL by design since 0.1.16 (clap rejects extras)."""

import pytest


def test_fetch_single_url(kn):
    res = kn("fetch", "https://example.com")
    assert res.code == 0
    data = res.yaml()
    assert data["title"] == "Example Domain"
    assert data["url"].startswith("https://example.com")
    assert "# Example Domain" in data["content"]
    assert "description" not in data


def test_fetch_live(kn):
    res = kn("fetch", "https://example.com", "--live")
    assert res.code == 0
    data = res.yaml()
    assert data["title"] == "Example Domain"
    assert "# Example Domain" in data["content"]


@pytest.mark.semantic  # asserts live LLM extraction output
def test_fetch_prompt(kn):
    # Ask for something the page text literally states. The original prompt asked
    # for the domain name, which appears nowhere in the markdown — the model has
    # to infer it from the URL, and once answered "The content does not contain
    # the page's domain name." That reply was still valid extraction output, so
    # the test failed on a detail it was never meant to pin down.
    res = kn("fetch", "https://example.com", "--prompt", "Answer with the exact page title and nothing else.")
    assert res.code == 0
    data = res.yaml()
    assert data["url"].startswith("https://example.com")
    # LLM extraction replaces the full page with the instruction's output. The
    # second assert is the load-bearing one: a stale daemon that drops `prompt`
    # returns the full page with exit code 0.
    assert "example domain" in data["content"].lower()
    assert "This domain is for use in illustrative examples" not in data["content"]


def test_fetch_max_chars(kn):
    res = kn("fetch", "https://example.com", "--max-chars", "50")
    assert res.code == 0
    data = res.yaml()
    # The truncation disclaimer is the load-bearing assert: a stale daemon
    # that drops `max_chars` returns the full page with exit code 0.
    assert "truncated to stay below 50 characters" in data["content"]


def test_fetch_max_chars_rejects_zero(kn):
    res = kn("fetch", "https://example.com", "--max-chars", "0")
    assert res.code == 2
    assert "invalid value" in res.err


def test_pretty_fetch(kn):
    res = kn("fetch", "https://example.com", "-p")
    assert res.code == 0
    assert res.out == ""
    assert "Example Domain" in res.err


def test_multiple_urls_rejected(kn):
    # 0.1.15 documented multi-URL but broke at the API; 0.1.16 made fetch
    # single-URL by design. Extra URLs are a clap parse error.
    res = kn("fetch", "https://example.com", "https://example.org")
    assert res.code == 2
    assert "unexpected argument" in res.err


def test_unparseable_url(kn):
    res = kn("fetch", "not-a-url")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Bad request"
    assert "Could not parse the provided URL" in data["message"]


def test_dead_page(kn):
    res = kn("fetch", "https://example.com/nonexistent-xyz-12345")
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Not found"


def test_invalid_key_on_fetch(kn):
    res = kn("fetch", "https://example.com", "--api-key", "keen_bad", key=False)
    assert res.code == 1
    data = res.yaml()
    assert data["error"] == "Authentication failed"
    # Server distinguishes malformed keys from well-formed-but-unknown ones.
    assert data["message"] in ("Invalid API key", "Malformed API key")


def test_no_url_arg(kn):
    res = kn("fetch")
    assert res.code == 2
    assert "the following required arguments were not provided" in res.err
    assert "<URL>" in res.err
