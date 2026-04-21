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
# Login with your Keenable account
keenable login

# Configure MCP for your AI clients (Claude Code, Cursor, etc.)
keenable setup --all

# Search the web
keenable search "rust async patterns" -p
```

## Usage

### Authentication

```bash
keenable login                          # OAuth login (opens browser)
keenable logout                         # Clear stored credentials
keenable configure --api-key <KEY>      # Headless setup (CI, servers)
```

### MCP setup

```bash
keenable setup                          # Show client status
keenable setup --all                    # Configure all detected clients
keenable setup --cursor                 # Configure a specific client
keenable reset --all                    # Remove Keenable from all clients
```

Supported clients: Claude Code, Claude Desktop, Cursor, VS Code, Windsurf, Codex.

### Search

```bash
keenable search "query"                 # YAML output (for agents)
keenable search "query" -p              # Pretty output (for humans)
keenable search "query" --api-key KEY   # Use a specific API key
```

### Fetch

```bash
keenable fetch https://example.com      # Fetch page content
keenable fetch url1 url2 -p             # Fetch multiple URLs, pretty output
```

### API keys

```bash
keenable keys-create --label "my-key"   # Create a new API key
```

## Updating

The CLI checks for updates automatically (once per day). To update manually:

```bash
brew upgrade keenable-cli               # Homebrew
# or re-run the installer script
```

## Building from source

```bash
cargo build --release
```

## Contributing

See [CLAUDE.md](CLAUDE.md) for project conventions and architecture.
