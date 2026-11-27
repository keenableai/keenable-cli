# Keenable CLI

[![Latest release](https://img.shields.io/github/v/release/keenableai/keenable-cli)](https://github.com/keenableai/keenable-cli/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Web search, page fetch and monitors for AI agents, from your terminal or over MCP.

[Keenable](https://keenable.ai) is a web search engine built for AI agents: fresh results with URLs to cite, and any page as clean markdown. The CLI lets you and your agents:

- **search** the web and **fetch** pages, with no account needed to start,
- add Keenable search to Claude Code, Cursor, Codex and other clients with one command (`configure-mcp`),
- run **monitors** that watch the web for news about a topic.

Output is YAML by default, so agents and scripts can read it, and readable with `-p`. Full reference: **[docs.keenable.ai/cli](https://docs.keenable.ai/cli)**.

**Contributions welcome.** Found a bug or missing a flag? [Open an issue](https://github.com/keenableai/keenable-cli/issues/new/choose). Want to fix it yourself? Pull requests are welcome; [good first issues](https://github.com/keenableai/keenable-cli/issues?q=is%3Aopen+label%3A%22good+first+issue%22) are a place to start, and [CONTRIBUTING.md](CONTRIBUTING.md) has the setup.

## Installation

**Homebrew (macOS + Linux):**

```bash
brew install keenableai/tap/keenable-cli
```

**Shell (macOS + Linux):**

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.sh | sh
```

**PowerShell (Windows):**

```powershell
irm https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.ps1 | iex
```

**From source:**

```bash
cargo install --git https://github.com/keenableai/keenable-cli
```

## Quick start

```bash
# Search the web (works without login)
keenable search "rust async patterns" -p

# Login for higher rate limits
keenable login

# Configure MCP for your AI clients (Claude Code, Cursor, etc.)
keenable configure-mcp --all
```

What a search returns (YAML, the default):

```console
$ keenable search "rust async patterns" --max-results 2
query: rust async patterns
mode: pro
results:
- title: Applying Concurrency with Async - The Rust Programming Language
  url: https://doc.rust-lang.org/book/ch17-02-concurrency-with-async.html
  acquired_at: 2026-11-25T00:07:20Z
  snippet: |-
    The first operation we tackled in the "Creating a New Thread with spawn" section in Chapter 16 was counting up …
- title: async/await - Asynchronous Programming in Rust
  url: https://rust-lang.github.io/async-book/03_async_await/01_chapter.html
  …
```

> This README follows `main`. Flags marked *(next release)* are merged but not released yet; `keenable <command> --help` always shows what your installed version takes.

## Usage

### Search

```bash
keenable search "query"                                    # YAML output (for agents)
keenable search "query" -p                                 # Pretty output (for humans)
keenable search "query" --mode realtime                    # Fastest response (default: pro, deeper retrieval)
keenable search "query" --max-results 25                   # More results (1-50, default 10)
keenable search "AI news" --site techcrunch.com            # Restrict to site
keenable search "query" --exclude-domain spam.com          # Never these domains
keenable search "query" --published-after 2026-01-01       # Date filter
keenable search "query" --acquired-before 2026-05-01       # Date filter
keenable search "query" --snippet-max-length 2000          # Longer snippets (180-10000)
keenable search "query" --query-time 2026-01-01T00:00:00Z  # Point-in-time search
keenable search "query" --session-id agent-run-42          # Group related calls in a session
keenable search "query" --objective "Find the tokio docs page on spawn_blocking"   # Say what the search is for (used in ranking)
keenable search "query" --moderation strict                # Remove unsafe results, moderate or strict (next release)
KEENABLE_API_KEY="$KEY" keenable search "query"            # Use a specific API key (or --api-key)
```

Works without login (free tier). Log in for higher rate limits.

*(next release)* `--moderation moderate` or `--moderation strict` removes results a safety model
judges unsafe at that level; results are removed, never replaced, so a search
can return fewer results or none. An empty value (`--moderation ""`) is the same
as leaving the flag out. It works in the `pro` and `realtime` modes; the API
rejects it (exit 8) for a URL query fetched as-is. When the API reports a deprecated request parameter, YAML output
carries its `warnings` list and each warning also goes to stderr as a
`warning: …` line; `-p` prints each warning on stderr.

#### Excluding domains

`--exclude-domain` drops results from a block-list of hosts. It is repeatable
and also takes a comma list, so these are equivalent:

```bash
keenable search "security council vote" --exclude-domain blogs.un.org,reddit.com
keenable search "security council vote" --exclude-domain blogs.un.org --exclude-domain reddit.com
```

- Each entry is a host and covers that host and every subdomain
  (`yahoo.com` also excludes `finance.yahoo.com`). At most 1,000 distinct
  entries per request, 2,000 entries as sent, and 2,048 bytes per entry.
- A public suffix with a leading dot excludes every host under it:
  `--exclude-domain .gov` drops `irs.gov`, `www.nasa.gov` and the rest, and
  `*.gov` means the same. The suffix must be on the
  [Public Suffix List](https://publicsuffix.org/) (`.co.uk` works,
  `.example` doesn't), and `.gov` doesn't cover `gov.uk`: write `.gov.uk` for
  that. A bare `gov` or `com` is rejected, so a typo can't drop a whole TLD.
- The server cleans up harmless noise: scheme, a leading `www.`, case, port,
  a trailing `/` or dot, a query string or `#fragment` and surrounding
  whitespace (`https://WWW.UN.org/` becomes `un.org`); international names
  become punycode.
- Anything else is rejected with an `invalid_filter` error naming the field,
  the entry and the reason, and the command exits 8: paths (`un.org/press`),
  a dot or wildcard on a host that isn't a public suffix (`.un.org`,
  `*.un.org`), a bare suffix (`com`, `co.uk`), IP addresses, `user@host` and
  invalid hosts. Private-suffix hosts such as `github.io` or `blogspot.com`
  are accepted.
- Spaces around comma-separated entries are trimmed. A blank entry, such as
  `--exclude-domain ""` or the trailing piece of `a.com,`, is sent as `""` and
  the server rejects it with `invalid_filter`. When a script builds the flag
  from a variable that may be empty, omit the flag instead.
- If every result is excluded, the response can be empty. Response warnings
  are printed in YAML and shown above the results with `-p`.

### Fetch

```bash
keenable fetch https://example.com      # Fetch page content
keenable fetch https://example.com -p   # Pretty output
keenable fetch https://a.com https://b.com   # Several pages in one call (1 to 20 URLs, each once)
keenable fetch https://example.com --max-age-hours 0    # Read the live page
keenable fetch https://example.com --max-age-hours 24   # Re-read the page if the stored copy is older (0 to 720, fractions allowed)
keenable fetch https://example.com --max-age-hours -1   # Stored copy only, of any age, never the live page (next release)
keenable fetch https://example.com --prompt "List all pricing tiers"   # LLM extraction instead of the full page
keenable fetch https://example.com --schema @plans.json   # Typed answer in `structured` (JSON Schema inline or @file, root type object)
keenable fetch https://example.com --links 50 --images 5  # Links (1 to 1000) and images (0 to 25) found on the page
keenable fetch https://example.com --max-chars 200000   # Raise the 50000-char content cap
keenable fetch https://example.com --session-id agent-run-42   # Group with related calls in a session
```

With a key (`keenable login`, `--api-key` or `KEENABLE_API_KEY`), fetch sends one `POST /v1/fetch` for all URLs. The call counts as one request toward the rate limit, and each page is billed at the fetch price. Without a key, fetch sends one `POST /v1/fetch/public` for one URL through the free tier and takes only `--max-chars`, `--prompt` and `--max-age-hours` (0.6.1 and older: only `--max-age-hours 0`); several URLs, `--schema`, `--links` or `--images` stop before any request with `API key required` and exit code 2.

One URL prints the page as before. Several URLs print `results`: one page or `{url, error}` per URL, in the order given; `-p` shows each in turn. A page can also carry `image`, `favicon`, `images`, `links`, `structured`, `answer_error` (why `structured` is missing) and `stale` (an older stored copy, because a fresh read failed). The command fails only when the call fails as a whole or every URL failed; then it exits with the code of the most common page error.

When content is cut at the character cap, it ends with the API's truncation note and the YAML output carries `truncated: true`.

### Monitors

A monitor watches the web for news about a topic: every newly crawled page that is news about it is kept as a result of that monitor, and sent to your webhook or returned in scheduled runs. Monitors need an API key (`keenable login`); they are free for now, with up to 100 active monitors per organization. See [Monitors](https://docs.keenable.ai/products/monitors).

```bash
keenable monitors create "new open-weights LLM releases" --every 1d     # Daily runs
keenable monitors create "Series A rounds in robotics" --cron "0 9 * * 1-5" --timezone Europe/Paris
keenable monitors create "CVE in OpenSSL" --webhook https://example.com/hook   # Each result to a webhook as it is found
keenable monitors list -p                                               # Your organization's monitors
keenable monitors results mon_123 --min-importance 4 -p                 # The important results it found
keenable monitors update mon_123 --every 6h                             # Change only what you pass
keenable monitors trigger mon_123                                       # Run now
keenable monitors runs mon_123 --limit 1                                # The latest run
keenable monitors pause mon_123                                         # Stop it for a while (resume to restart)
keenable monitors delete mon_123 --yes                                  # Delete it and its results
```

`create` prints `webhook.secret` (`whsec_...`) once when you pass `--webhook`: store it to check signatures. By default a webhook gets the lifecycle and run events plus `monitor.result.found` and `monitor.result.changed`, and `monitor.page.changed` once a page is attached; `--event` sends only the events you list. `--idempotency-key` makes a retried create return the same monitor. Output is YAML by default and readable with `-p`; errors and exit codes are the same as for search and fetch. `delete` asks first, and without a terminal it needs `--yes`.

### Errors and exit codes in scripts

On a failed `search` or `fetch`, YAML mode prints the structured error on
stdout (`error`, `type`, `code`, `message`, `fix`, `retry_after`, `hint`, ...)
and a one-line summary on stderr. `-p` prints only to stderr. The exit code
says what failed, so a script does not need to parse the output:

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Any other error: a local setup problem such as a bad `SSL_CERT_FILE`, an invalid `KEENABLE_API_URL` or an invalid `--mode` |
| 2 | Invalid arguments (the CLI's own usage error) |
| 3 | Authentication or permission: the key is invalid, revoked or not allowed this |
| 4 | Rate limited: wait `retry_after` seconds (or until `rate_limit_reset`), then retry |
| 5 | Out of credits: add credits at https://keenable.ai/console |
| 6 | The page or site failed (fetch): not found, blocked, timed out, unreachable |
| 7 | Network: no response from the API (proxy, DNS, TLS) |
| 8 | Invalid request: fix the arguments; retrying unchanged fails the same way |
| 9 | Server error on Keenable's side: retry later |

```bash
keenable search "query" > out.yaml
case $? in
  0) ;;                       # results in out.yaml
  4) sleep 2 ;;               # rate limited: retry later
  5) echo "out of credits" ;; # stop the run
  9) sleep 30 ;;              # Keenable server error: retry later
esac
```

With an API key, each response's `usage.credits_remaining` is the
organization's credits left after the request (an estimate that can lag a few
seconds); `-p` shows it as `Credits left: N`.

### API endpoint

Set `KEENABLE_API_URL` to send API calls somewhere other than `https://api.keenable.ai`, e.g. a local gateway that adds credentials. It must be `https://`, except `http://` to `localhost`, `127.0.0.1` or `::1`, and must not contain a user or password. `configure-mcp` still writes the production MCP URL.

```bash
KEENABLE_API_URL=http://127.0.0.1:8080 keenable search "query"
```

A stored API key is only sent to the API it was saved for. `keenable login` saves the key for the current `KEENABLE_API_URL` (production when unset), next to any key you saved for another API, so logging in under an override keeps your production key. A command run against an API with no key of its own stops with exit code 1 instead of sending the production key there. Under an override, pass a key with `KEENABLE_API_KEY` or `--api-key`, or run `keenable login` with the override set. This keeps a `KEENABLE_API_URL` set by a project's environment from redirecting your key to another host.

Redirects are followed only within the same scheme, host and port, so the API key never follows a redirect to another host or to plain http.

### Authentication

```bash
keenable login                          # Device-code login (opens browser)
printenv KEENABLE_API_KEY | keenable login --api-key -
                                        # Save a key from stdin (CI, servers)
keenable login --api-key <KEY>          # Save API key directly
keenable logout                         # Remove the key for the current API address
```

Prefer `--api-key -`: a key passed as an argument is visible to other processes on the machine and lands in shell history. The key is stored in `~/.keenable/config.json` (mode 0600, in a 0700 directory). `configure-mcp` also writes it into each configured client's MCP config; `logout` lists those clients, and `keenable reset --all` removes it from them.

### MCP setup

```bash
keenable configure-mcp                  # Show client status
keenable configure-mcp --all            # Configure all detected clients
keenable configure-mcp --cursor         # Configure a specific client
keenable reset --all                    # Remove Keenable from all clients
```

Supported clients: Claude Code, Cursor, Devin Desktop (`--devin`, writes `~/.config/devin/mcp_config.json`), Windsurf (`--windsurf`, writes the legacy `~/.codeium/windsurf/mcp_config.json`), Codex, OpenCode.

### Settings

```bash
keenable config                                   # Show all settings
keenable config set default_search_mode realtime  # Mode when --mode isn't given
keenable config set forced_search_mode pro        # Mode for every search, even with --mode
keenable config get default_search_mode
keenable config unset default_search_mode
```

Settings live in `~/.keenable/config.json`. An unknown key or a value that isn't allowed is a usage error, exit code 2 (next release; 0.6.1 and older exit 1).

### Behind a proxy

The CLI honors `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY`. It trusts its bundled Mozilla roots plus the OS certificate store; when `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, those replace the OS store. A TLS-intercepting proxy works once its CA is in that bundle:

```bash
export HTTPS_PROXY=http://proxy.internal:3128
export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle-with-proxy-ca.pem
keenable search "query"
```

The background daemon (macOS/Linux) exits after 5 minutes idle. It restarts on its own when the proxy or certificate variables change, so there is nothing to do after changing them. Commands run with `--api-key`, `KEENABLE_API_KEY` or `-v` skip the daemon.

An empty `--api-key` value (for example `--api-key "$UNSET_VAR"`) is an error with exit code 1 on `search`, `fetch` and `login`, so a missing key never silently turns into an anonymous request or replaces your saved key. An empty `KEENABLE_API_KEY` is ignored with a warning on stderr.

## Updating

The CLI checks for updates automatically (once per hour). To update manually:

```bash
keenable update                                        # Installer-script installs
brew update && brew upgrade keenable-cli               # Homebrew
```

What changed in each version: [CHANGELOG.md](CHANGELOG.md).

## Help and feedback

- Docs: [docs.keenable.ai/cli](https://docs.keenable.ai/cli), and `keenable <command> --help`
- Bugs and feature requests: [GitHub issues](https://github.com/keenableai/keenable-cli/issues/new/choose)
- Account, billing or API questions: support@keenable.ai
- Security: see [SECURITY.md](SECURITY.md)

## Contributing

Issues and pull requests from anyone are welcome, from a typo fix to a new flag.

1. Look for an open issue, or [open one](https://github.com/keenableai/keenable-cli/issues/new/choose) to talk through a bigger change first.
2. Fork the repo, make the change on a branch, and run `cargo test`.
3. Open a pull request. A maintainer reviews it and runs the live-API tests.

Issues labelled [good first issue](https://github.com/keenableai/keenable-cli/issues?q=is%3Aopen+label%3A%22good+first+issue%22) or [help wanted](https://github.com/keenableai/keenable-cli/issues?q=is%3Aopen+label%3A%22help+wanted%22) are ready to pick up. Setup and conventions: [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
