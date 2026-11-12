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
keenable search "query" --exclude-domain spam.com          # Never these domains
keenable search "query" --published-after 2026-01-01       # Date filter
keenable search "query" --acquired-before 2026-05-01       # Date filter
keenable search "query" --snippet-max-length 2000          # Longer snippets (180-10000)
keenable search "query" --query-time 2026-01-01T00:00:00Z  # Point-in-time search
keenable search "query" --session-id agent-run-42          # Group related calls in a session
keenable search "query" --api-key KEY                      # Use a specific API key
```

Works without login (free tier). Log in for higher rate limits.

#### Excluding domains

`--exclude-domain` drops results from a block-list of hosts. It is repeatable
and also takes a comma list, so these are equivalent:

```bash
keenable search "security council vote" --exclude-domain blogs.un.org,reddit.com
keenable search "security council vote" --exclude-domain blogs.un.org --exclude-domain reddit.com
```

- Each entry is a host and covers that host and every subdomain
  (`yahoo.com` also excludes `finance.yahoo.com`). At most 1,000 distinct
  hosts per request, 2,000 entries as sent, and 2,048 bytes per entry.
- The server cleans up harmless noise: scheme, a leading `www.`, case, port,
  a trailing `/` or dot, a query string or `#fragment` and surrounding
  whitespace (`https://WWW.UN.org/` becomes `un.org`); international names
  become punycode.
- Anything else is rejected with an `invalid_filter` error naming the field,
  the entry and the reason, and the command exits 8: paths (`un.org/press`),
  suffixes (`.com`), wildcards (`*.gov`), IP addresses, `user@host` and
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
keenable fetch https://example.com --live   # Fetch the live page (skip cache)
keenable fetch https://example.com --prompt "List all pricing tiers"   # LLM extraction instead of the full page
keenable fetch https://example.com --max-chars 200000   # Raise the 50000-char content cap
keenable fetch https://example.com --session-id agent-run-42   # Group with related calls in a session
```

When content is cut at the character cap, it ends with the API's truncation note and the YAML output carries `truncated: true`.

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

A stored API key is only sent to the API it was saved for. `keenable login` saves the key together with the current `KEENABLE_API_URL` (production when unset), and a command run against any other API stops with exit code 1 instead of sending it there. Under an override, pass a key with `KEENABLE_API_KEY` or `--api-key`, or run `keenable login` with the override set. This keeps a `KEENABLE_API_URL` set by a project's environment from redirecting your key to another host.

Redirects are followed only within the same scheme, host and port, so the API key never follows a redirect to another host or to plain http.

### Authentication

```bash
keenable login                          # Device-code login (opens browser)
printenv KEENABLE_API_KEY | keenable login --api-key -
                                        # Save a key from stdin (CI, servers)
keenable login --api-key <KEY>          # Save API key directly
keenable logout                         # Clear stored credentials
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

### Behind a proxy

The CLI honors `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY`. It trusts its bundled Mozilla roots plus the OS certificate store; when `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, those replace the OS store. A TLS-intercepting proxy works once its CA is in that bundle:

```bash
export HTTPS_PROXY=http://proxy.internal:3128
export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle-with-proxy-ca.pem
keenable search "query"
```

The background daemon (macOS/Linux) exits after 5 minutes idle. It restarts on its own when the proxy or certificate variables change, so there is nothing to do after changing them. Commands run with `--api-key`, `KEENABLE_API_KEY` or `-v` skip the daemon.

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
