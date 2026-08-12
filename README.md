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
keenable search "query" --published-after 2026-01-01       # Date filter
keenable search "query" --acquired-before 2026-05-01       # Date filter
keenable search "query" --snippet-max-length 2000          # Longer snippets (180-10000)
keenable search "query" --api-key KEY                      # Use a specific API key
```

Works without login (free tier). Log in for higher rate limits.

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
brew update && brew upgrade keenable-cli               # Homebrew
# or re-run the installer script
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
