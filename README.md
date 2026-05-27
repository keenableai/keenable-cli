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
keenable search "rust async patterns" --mode pro -p

# Login for higher rate limits
keenable login

# Configure MCP for your AI clients (Claude Code, Cursor, etc.)
keenable configure-mcp --all
```

## Usage

### Search

```bash
keenable search "query" --mode pro                         # YAML output (for agents)
keenable search "query" --mode pro -p                      # Pretty output (for humans)
keenable search "AI news" --site techcrunch.com            # Restrict to site
keenable search "query" --published-after 2026-01-01       # Date filter
keenable search "query" --acquired-before 2026-05-01       # Date filter
keenable search "query" --api-key KEY                      # Use a specific API key
```

Search modes: `--mode standard` (fast, default) or `--mode pro` (higher quality).

Works without login (free tier). Log in for higher rate limits.

### Fetch

```bash
keenable fetch https://example.com      # Fetch page content
keenable fetch url1 url2 -p             # Fetch multiple URLs, pretty output
```

### Configuration

```bash
keenable config                                        # View all settings
keenable config set default_search_mode pro            # Default to pro mode
keenable config set forced_search_mode standard        # Always use standard, ignore --mode
keenable config get default_search_mode                # Get a single value
keenable config unset forced_search_mode               # Remove a setting
```

Supported keys:
- `default_search_mode` — search mode when `--mode` is not specified (`standard`, `pro`)
- `forced_search_mode` — always use this mode, ignoring `--mode` (`standard`, `pro`)

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

Supported clients: Claude Code, Claude Desktop, Cursor, Windsurf, Codex, OpenCode.

### WebQL MCP setup

```bash
keenable configure-webql                # Show client status
keenable configure-webql --all          # Configure all detected clients
keenable reset-webql --all              # Remove WebQL from all clients
```

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

## Contributing

See [CLAUDE.md](CLAUDE.md) for project conventions and architecture.
