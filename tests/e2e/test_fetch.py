"""T-23..T-29 — fetch. Single-URL by design since 0.1.16 (clap rejects extras)."""


def test_fetch_single_url(kn):
    res = kn("fetch", "https://example.com")
    assert res.code == 0
    data = res.yaml()
    assert data["title"] == "Example Domain"
    assert data["url"].startswith("https://example.com")
    assert "# Example Domain" in data["content"]


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
    # The backend rejects an unfetchable page with either a 404 ("Not found")
    # or a 422 ("Unprocessable entity") depending on how far upstream got —
    # both are valid "this page can't be fetched" outcomes.
    assert data["error"] in ("Not found", "Unprocessable entity")


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
