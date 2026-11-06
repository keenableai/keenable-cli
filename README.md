# Keenable CLI

CLI for [Keenable](https://keenable.ai) — authenticate, manage API keys, configure MCP, and search the web.

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

## Usage

### Search

```bash
keenable search "query"                                    # YAML output (for agents)
keenable search "query" -p                                 # Pretty output (for humans)
keenable search "AI news" --site techcrunch.com            # Restrict to site
keenable search "query" --include-domain a.com,b.com       # Only these domains
keenable search "query" --exclude-domain spam.com          # Never these domains
keenable search "query" --published-after 2026-01-01       # Date filter
keenable search "query" --acquired-before 2026-05-01       # Date filter
keenable search "query" --snippet-max-length 2000          # Longer snippets (180-10000)
keenable search "query" --query-time 2026-01-01T00:00:00Z  # Point-in-time search
keenable search "query" --api-key KEY                      # Use a specific API key
```

Works without login (free tier). Log in for higher rate limits.

#### Domain filters

`--include-domain` keeps results to an allow-list of hosts; `--exclude-domain`
drops a block-list. Both are repeatable and each also takes a comma list, so
these are equivalent:

```bash
keenable search "security council vote" --include-domain press.un.org,undocs.org --include-domain who.int
keenable search "security council vote" --include-domain press.un.org --include-domain undocs.org --include-domain who.int
```

Each entry is a host, and it covers that host and every subdomain
(`yahoo.com` also matches `finance.yahoo.com`). Paths (`un.org/press`) and
suffixes or wildcards (`.gov`, `*.gov`, `*`) are not supported.
Hosts that are public suffixes only in the private section of the Public
Suffix List (`github.io`, `blogspot.com`, `s3.amazonaws.com`) are accepted,
while ICANN suffixes and non-registrable names (`com`, `co.uk`, `localhost`)
are rejected; a leading `www.` is kept when the rest would be a public suffix
(`www.github.io` stays as is).

- Limits per request, counted on distinct hosts: at most 50 in
  `--include-domain` and at most 200 in `--exclude-domain`.
- The server tidies harmless noise for you and says so: case, an `http://` or
  `https://` scheme, a port, a trailing `/`, a trailing dot, a leading `www.`
  and duplicates are cleaned up (`https://WWW.UN.org/` becomes `un.org`), and
  the response `warning` lists what was changed.
- Spaces around comma-separated entries and stray commas are ignored
  (`"a.com, b.com"` and `a.com,` both work). A blank value such as
  `--include-domain ""` is still sent, so the server rejects it instead of
  running an unfiltered search.
- Both flags together keep results that are included and not excluded:
  an exclude host wins over a broader include host (include `a.com`,
  exclude `docs.a.com`). A narrow list can return fewer than `--max-results`.
  When the filter actually removed results, the response `warning` says so.
  Warnings are printed in YAML and shown above the results with `-p`.
- `--site` still works as a single-domain alias, but cannot be combined with
  `--include-domain`.
- The CLI does not validate entries itself. An entry the server cannot fix (a
  path, a suffix or wildcard, another scheme such as `ftp://`, `user@host`, an
  IP address, `localhost` or an unknown top-level domain, the same host in both
  lists, a list over its limit, ...) comes back as an `invalid_filter` error
  naming the flag's field, the entry and how to fix it, and the command exits 1.

### Fetch

```bash
keenable fetch https://example.com      # Fetch page content
keenable fetch https://example.com -p   # Pretty output
keenable fetch https://example.com --live   # Fetch the live page (skip cache)
keenable fetch https://example.com --prompt "List all pricing tiers"   # LLM extraction instead of the full page
keenable fetch https://example.com --max-chars 200000   # Raise the 50000-char content cap
```

### Authentication

```bash
keenable login                          # Device-code login (opens browser)
keenable login --api-key <KEY>          # Save API key directly (CI, servers)
keenable logout                         # Clear stored credentials
```

### MCP setup

```bash
keenable configure-mcp                  # Show client status
keenable configure-mcp --all            # Configure all detected clients
keenable configure-mcp --cursor         # Configure a specific client
keenable reset --all                    # Remove Keenable from all clients
```

Supported clients: Claude Code, Cursor, Windsurf, Codex, OpenCode.

## Updating

The CLI checks for updates automatically (once per hour). To update manually:

```bash
keenable update                                        # Installer-script installs
brew update && brew upgrade keenable-cli               # Homebrew
```

## Building from source

```bash
cargo build --release
```

## Testing

End-to-end suite (real binary, live API — see `tests/e2e/`):

```bash
export KEENABLE_API_KEY=keen_...
uv run --project tests/e2e pytest tests/e2e -v
```

## Contributing

See [CLAUDE.md](CLAUDE.md) for project conventions and architecture.
