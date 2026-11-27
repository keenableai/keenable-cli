# Changelog

User-facing changes to the Keenable CLI. Each release's section is also its
[GitHub release notes](https://github.com/keenableai/keenable-cli/releases).
**Breaking** marks a change that can break a script written for the previous
version.

Add your change under "Unreleased" in the same PR. The release workflow turns
that heading into the version number.

## Unreleased

- `keenable search --moderation <moderate|strict>` removes results a safety
  model judges unsafe at that level (#144).
- Search responses can carry `warnings` (deprecated request parameters). YAML
  output passes them through, and each also goes to stderr as a
  `warning: …` line (#144).
- `keenable fetch --max-age-hours -1` reads only Keenable's stored copy, of
  any age, and never the live page. Without a key, fetch now takes any
  `--max-age-hours`, not just `0` (#145).
- **Breaking:** `keenable config` exits 2 (a usage error) instead of 1 for an
  unknown key or a value that isn't allowed.
- The CLI is now MIT licensed. `--help` links to the docs and the issue
  tracker, and the top-level help shows `fetch` and `monitors`.

## 0.6.1 - 2026-11-23

- **Breaking:** `keenable fetch --live` is removed. Use `--max-age-hours 0`
  (#132).
- `keenable fetch` takes 1 to 20 URLs in one call, plus `--schema`,
  `--links`, `--images` and `--max-age-hours` (0 to 720). With several URLs,
  YAML prints `results` with one page or `{url, error}` per URL (#132).
- **Breaking:** `keenable monitors pages` is now `keenable monitors results`
  (#131).
- Keyless fetch uses `POST /v1/fetch/public` (#138).
- `--exclude-domain` accepts public suffixes such as `.gov` (#135).

## 0.6.0 - 2026-11-18

- `keenable monitors`: create, list, update, pause, resume, delete and trigger
  monitors, and read their runs, results and webhook deliveries (#124).
- `keenable search --objective` says what a search is for; it is used in
  ranking (#122).
- An empty `--api-key` value is an error (exit 1) instead of a silent fallback
  to the saved key or an anonymous request. An empty `KEENABLE_API_KEY` is
  ignored with a warning (#121).

## 0.5.1 - 2026-11-13

- **Breaking:** `search` and `fetch` exit with a code for each kind of failure
  (3 auth, 4 rate limited, 5 out of credits, 6 page failed, 7 network,
  8 invalid request, 9 server error) instead of always 1. Scripts that check
  for any non-zero code keep working (#107).
- In YAML mode, errors also print a one-line `error: …` summary on stderr
  (#107).
- `-p` shows `Credits left: N` when the API reports it (#107).
- Security and crash fixes (#111).

## 0.4.0 - 2026-11-11

- `keenable search --exclude-domain` (#89).
- `configure-mcp --devin` and `reset --devin` for Devin Desktop (#104).

## 0.3.0 - 2026-11-09

- The CLI trusts the OS certificate store and `SSL_CERT_FILE` /
  `SSL_CERT_DIR`, and the new `-v` / `--verbose` flag shows the proxy,
  trusted CAs and the full error chain (#98).
- Errors show the API's `type`, `code`, `fix` and `docs_url` (#93).
- Device login polls with the RFC 8628 grant and backs off on `slow_down`
  (#92).

Older releases: see the
[GitHub releases](https://github.com/keenableai/keenable-cli/releases).
