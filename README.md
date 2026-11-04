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

`--include-domain` keeps results to an allow-list; `--exclude-domain` drops a
block-list. Both are repeatable and each also takes a comma list, so these are
equivalent:

```bash
keenable search "security council vote" --include-domain press.un.org,undocs.org --include-domain .gov
keenable search "security council vote" --include-domain press.un.org --include-domain undocs.org --include-domain .gov
```

Each entry is one of:

| Entry | Matches |
|---|---|
| `example.com` | the host and its subdomains |
| `example.com/docs` | that path prefix (`/docs/x`, not `/docsx`) |
| `.gov`, `*.gov`, `.gov.uk` | any host under that suffix |

- Spaces around comma-separated entries and stray commas are ignored
  (`"a.com, b.com"` and `a.com,` both work). A blank value such as
  `--include-domain ""` is still sent, so the server rejects it instead of
  running an unfiltered search.
- Both flags together keep results that are included and not excluded:
  an exclude entry wins over a broader include entry (include `a.com`,
  exclude `docs.a.com`). A narrow list can return fewer than `--max-results`.
  When the filter actually removed results, the response carries a `warning`
  that says so (printed in YAML, and shown above the results with `-p`).
- `--site` still works as a single-domain alias, but cannot be combined with
  `--include-domain`.
- The CLI does not validate entries itself. A bad entry (scheme, port, bare
  `*`, the same entry in both lists, ...) comes back from the server as an
  error naming the flag's field, the entry and the reason, and the command
  exits 1.

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
