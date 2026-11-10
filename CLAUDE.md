# Keenable CLI

Rust CLI for Keenable — authenticate, manage API keys, configure MCP, and search the web.

## Build

```bash
cargo build --release
```

## Tests

End-to-end suite in `tests/e2e/` (pytest, run with UV) exercises the real binary against the live API: every subcommand/flag/error path, the stored-login + daemon path (`login --api-key` → commands without the flag → `logout`), `configure-mcp`/`reset` against fake client configs (entry shapes per IDE, dedup, Claude Code deny-list, TOML for Codex, reset round-trips — local file work, no API), installer download + smoke test, semantic-relevance checks (gold facts, authority domains, `site:` parity), and wall-clock latency checks.

```bash
export KEENABLE_API_KEY=keen_...                 # required
export KEENABLE_BIN=./target/release/keenable    # default: `keenable` on PATH
uv run --project tests/e2e pytest tests/e2e -v
```

Markers: `-m "not latency"` / `-m "not semantic"` / `-m "not install"` to skip the slow/live-index/download groups. CI (`.github/workflows/e2e.yml`) installs the latest released binary via the installer script and runs the full suite nightly and on manual dispatch (with an optional version input). Requires the `KEENABLE_API_KEY` repo secret.

The suite isolates each run by pointing `KEENABLE_HOME` at a temp dir (see "Config & path resolution" below). CI runs the matrix across **Linux, macOS, and Windows**. Unix installs the binary via the shell installer; Windows via the PowerShell installer (`keenable-cli-installer.ps1`). The Unix-only daemon tests and the shell-installer test self-skip on Windows (`os.name != "posix"`), so the same `pytest tests/e2e` invocation works on every platform.

## Project Structure

```
src/
  main.rs              # CLI entry point, clap definitions
  ui.rs                # Shared terminal UI helpers (header, step_done, success, error, hint)
  constants.rs         # Default API URL, KEENABLE_API_URL name, OAuth config
  config.rs            # ~/.keenable/ config and credentials read/write
  api.rs               # HTTP client factories (api_key_client, bare_client), ApiError, response handling
  update.rs            # GitHub Releases version check (cached, non-blocking)
  daemon.rs            # Background daemon for connection reuse (auth + unauth)
  net.rs               # Shared HTTP client builder: trust roots, proxy reporting, error chains + hints, --verbose
  commands/
    login.rs           # Device code login + --api-key direct save; also logout
    config_cmd.rs      # CLI configuration settings
    ide.rs             # Shared IDE definitions and config helpers
    configure_mcp.rs   # Client detection, MCP configuration, interactive setup
    reset.rs           # Remove Keenable MCP and restore defaults
    search.rs          # search, fetch commands
    update_cmd.rs      # Self-update via axoupdater (needs the installer's receipt)
assets/
  login_success.html   # Styled OAuth callback success page
  login_failure.html   # Styled OAuth callback failure page
```

## Conventions

### Login Modes

- **`keenable login`** — interactive device-code flow. Opens browser, provisions API key, enables MCP configuration via `configure-mcp`.
- **`keenable login --api-key <KEY>`** — for CI, servers, agent machines. Saves API key directly, no browser needed.

### Output Format

All tool commands (`search`, `fetch`) output **YAML by default** (token-efficient for agents).
Use `-p` / `--pretty` flag for pretty-printed human-readable output.
Use `--api-key <KEY>` (or the `KEENABLE_API_KEY` env var; flag wins) to override the stored key for one-off calls. Either override bypasses the daemon and goes direct.
Use the global `-v` / `--verbose` flag to print network details to stderr: the proxy in use and its source, the trusted CA sources, and the full error chain on failure. It also bypasses the daemon (see Daemon).

```bash
keenable search "query"                        # YAML output (for agents)
keenable search "query" -p                     # Pretty output (for humans)
keenable search "query" --api-key keen_***_*****    # Use specific API key
```

Management commands (`login`, `logout`, `configure-mcp`, `config`) always output human-readable text.

### Config Command (`src/commands/config_cmd.rs`)

`keenable config` manages CLI settings stored in `~/.keenable/config.json`.

```bash
keenable config                          # View all settings
keenable config set <key> <value>        # Set a value
keenable config get <key>                # Get a single value
keenable config unset <key>              # Remove a setting
```

Supported keys and allowed values are defined in `KNOWN_KEYS` in `config_cmd.rs`. Adding a new config key only requires adding an entry there.

### Unauthenticated (Free Tier) Flow

All tool commands (`search`, `fetch`) work without login. When no API key is configured:
- Requests go to `/v1/{search,fetch}/public` endpoints (IP-based rate limits)
- The daemon starts with a bare HTTP client (no `X-API-Key` header)
- Rate limit errors include a hint to run `keenable login` for higher limits

### Config & Path Resolution

Every path the CLI touches (config, credentials, daemon socket, MCP client configs) is resolved against `config::home_dir()`, which returns the `KEENABLE_HOME` env var when set and falls back to `dirs::home_dir()` otherwise. Set `KEENABLE_HOME` to relocate the entire `.keenable` tree — this is how the e2e suite isolates runs per-test (and the only reliable way on Windows, where `dirs::home_dir()` queries the real user profile and ignores `HOME`). Always go through `config::home_dir()`, never `dirs::home_dir()` directly.

### Daemon

The CLI uses a background daemon (`~/.keenable/daemon.sock`) to reuse HTTP connections.
Commands auto-start the daemon on first call. It auto-exits after 5 minutes of idle.

The daemon works with or without an API key (authenticated vs public endpoints).
It is automatically killed and restarted on `login` / `logout` so it picks up the new auth state.
There is one daemon per API base: `daemon.sock` / `daemon.pid` for the production API, `daemon-<12-hex FNV-1a of the base>.sock/.pid` for a `KEENABLE_API_URL` override, so switching bases never stops another base's daemon. `login` / `logout` stop all of them. A socket path at or over the OS limit (104 bytes on macOS) skips the daemon and goes direct.
Each connection starts with an `identity` handshake: current daemons answer with their `DaemonIdentity` (API base, `CARGO_PKG_VERSION`, and a build id from the executable's size + mtime, captured at daemon start), older ones with an untagged "Unknown command". Any difference counts as a mismatch, so a daemon left running across an upgrade or rebuild is replaced instead of silently dropping request fields it doesn't know (serde skips unknown fields). Accepted trade-off: two installs of the same version at different paths (e.g. Homebrew and the installer script) sharing one `KEENABLE_HOME` have different build ids, so alternating between them restarts each other's daemon; results stay correct, only the connection reuse is lost. On a mismatch the real request is never sent; `daemon_request` restarts that daemon once, then falls back to the direct path. Every response is also tagged and checked as a backstop. A daemon on an overridden base refuses untagged requests (older CLIs only know the default base). `stop_daemon` removes socket/pid files only if the pid file is unchanged, so a concurrently started replacement survives.

### API Base URL (`KEENABLE_API_URL`)

All API calls (search, fetch, public endpoints, key validation, login device flow) build URLs via `api::api_url()`, which uses `api::api_base()`: `KEENABLE_API_URL` when set, else `https://api.keenable.ai` (`DEFAULT_API_BASE_URL`). Rule: https only, except http to `localhost` or a loopback IP; trailing slashes are stripped, a path prefix is kept. Same variable as the SDKs, and the same core rule (https, or http to loopback); the CLI additionally rejects a query or fragment (the Go SDK does too, Python doesn't) and accepts any spelling of the loopback IPs (`127.1`, `[0:0::1]`), which the SDKs reject. `main` validates it before any API command and exits 1 with an error naming the variable and the fix — YAML on stdout only for search/fetch without `-p`, `ui::error` + `ui::hint` on stderr otherwise (`api::exit_invalid_api_base`). The MCP URL written by `configure-mcp` always uses the production base — an env override is never persisted into client configs. Never format URLs from the constant directly.

Commands that bypass the daemon on purpose and go direct: `--api-key` / `KEENABLE_API_KEY` (the daemon's cached auth differs) and `--verbose` (so the proxy/CA view and full error chain it reports are the client's own, not the daemon's).

The daemon captures its environment when spawned. Each request carries `net::env_fingerprint()` (the proxy and CA env vars, `KEENABLE_APP_TITLE`, and the `SSL_CERT_FILE` / `SSL_CERT_DIR` metadata); a daemon whose fingerprint differs replies `restart`, stops accepting, lets in-flight requests finish (bounded) and exits, and that request goes direct. A transport failure seen through the daemon (except a timeout) stops it and is retried directly once, which also covers OS trust-store and system-proxy changes the fingerprint can't see.

### Terminal UI Design (`src/ui.rs`)

All human-facing output uses the shared `ui` module for consistent styling. The 👀 emoji is the CLI's brand mark.

**Top-level steps** (used by `login`, `logout`, `configure-mcp`, etc.):

```
👀  keenable login                       ← ui::header()

   ✓  Discovered OAuth endpoints          ← ui::step_done()  — dimmed + strikethrough
   ✓  Opened browser                      ← ui::step_done()
   ✓  Logged in as user@example.com       ← ui::success()    — green

   Next: keenable configure-mcp --all       ← ui::hint()       — dimmed
```

**Sub-steps** (used by `configure-mcp` for per-client configuration):

All sub-items are prefixed with `-` and word-wrapped so continuation lines align with the text start (at column 12).

```
   Claude Code                                    ← ui::label()
      - ✓  No conflicting search MCPs             ← ui::sub_done()    — dimmed + strikethrough
      - ⚠  Different API key configured           ← ui::sub_warning() — yellow
      - ✓  Keenable MCP: added                    ← ui::sub_success() — green
      - ⚠  We recommend disabling standard        ← ui::sub_hint()    — ⚠ yellow, text dimmed
            search & fetch tools in Cursor…              (continuation aligned at column 12)
```

**All helpers:**

| Helper | Use for | Style |
|---|---|---|
| `ui::header(cmd)` | Start of every command | 👀 + bold |
| `ui::step_done(msg)` | Completed intermediate step | dimmed + strikethrough |
| `ui::success(msg)` | Final result / positive outcome | green ✓ |
| `ui::error(msg)` | Failure | red ✗ |
| `ui::warning(msg)` | Non-blocking concern | yellow ⚠ |
| `ui::hint(msg)` | Next-step suggestion | dimmed |
| `ui::info(msg)` | Neutral info, section labels | plain |
| `ui::label(msg)` | Section header | bold |
| `ui::sub_done(msg)` | Sub-step completed (extra indent) | dimmed + strikethrough |
| `ui::sub_success(msg)` | Sub-step positive (extra indent) | green ✓ |
| `ui::sub_warning(msg)` | Sub-step warning (extra indent) | yellow ⚠ |
| `ui::sub_error(msg)` | Sub-step failure (extra indent) | red ✗ |
| `ui::sub_hint(msg)` | Sub-step recommendation (extra indent) | ⚠ yellow icon, text dimmed |

- All human output goes to `stderr` (`eprintln!`), keeping `stdout` clean for YAML/machine output
- Pretty search/fetch output also uses `ui::header()` and `eprintln!` for consistency

**When adding or modifying commands**, always use `ui::` helpers rather than raw `println!`/`eprintln!` with ad-hoc formatting.

### Network: Proxies and Certificates (`src/net.rs`)

Build every HTTP client through `net::client_builder()` (or the `api::api_key_client` / `bare_client` helpers, which add `X-Keenable-Title`), never `reqwest::Client::new()`. Out of scope: `keenable update` downloads through axoupdater's own reqwest client, which this does not cover. `tests/tls_proxy.rs` runs the binary through a local TLS-intercepting proxy to check trust, and `tests/e2e/test_network.py` covers the error output, proxies and `--verbose`.

- **Trust:** bundled Mozilla roots + the OS certificate store. `SSL_CERT_FILE` / `SSL_CERT_DIR` (PEM) replace the OS store when set; the bundled roots always stay. Empty values are removed in `main` before the runtime starts and count as unset. An unusable `SSL_CERT_FILE` is an `Invalid SSL_CERT_FILE` error naming the file (exit 1); `login` and `configure-mcp` check it before key validation so it isn't mistaken for a network blip.
- **Proxy:** reqwest honors `HTTPS_PROXY` / `https_proxy`, `ALL_PROXY` / `all_proxy`, `HTTP_PROXY`, `NO_PROXY` / `no_proxy`, and with the `system-proxy` feature the macOS/Windows proxy settings. Never call `.no_proxy()`. Proxy passwords are never printed.

### Error Handling

- All human output to `stderr` via `eprintln!` or `ui::` helpers; machine output to `stdout` via `println!`
- Exit with code 1 on errors
- Always suggest the fix (e.g., "Run `keenable login` first") — use `ui::error()` + `ui::hint()`
- API errors use the `ApiError` struct (`src/api.rs`) which parses the backend's flat error body: the legacy `{error, message, retryAfter}` plus the error format v1 fields `type`, `code`, `fix`, `docs_url` (and `retry_after_seconds`). The v1 fields are optional: older servers don't send them, and nothing changes then
- Transport errors keep their `source()` chain: build them with `ApiError::from_transport`. Pretty mode prints a `Cause:` line (the full chain with `-v`) via `ApiError::print_details()`; YAML adds `cause` and `hint` fields. Hints point at `SSL_CERT_FILE` for certificate errors and at `HTTPS_PROXY` / `NO_PROXY` for connect, proxy and timeout failures
- In YAML mode, errors are output as structured YAML to stdout: `error`, `type`, `code`, `message`, `fix`, `retry_after`, `rate_limit_reset` (the API's `X-RateLimit-Reset` on a 429), `docs_url`, `request_id` (the API's `X-Request-Id`, for support), `cause` (transport errors only), and a `hint` with the next step. `type`/`code`/`fix`/`docs_url` appear only when the server sent them; the legacy keys never change
- In pretty mode, errors use `ui::error()` + `ui::hint()` to stderr, plus `Docs: …` and `Request ID: …` hints when the API sent them
- The vendored backend catalog (`tests/fixtures/error-catalog.json`, a copy of the backend's `docs/error-catalog.json`) drives the error tests in `api.rs`; refresh it by copying the backend file over it
- Hints come from `ApiError::hint(authenticated)`. The CLI's own hint, by status, comes first (it names CLI commands such as `keenable login`); the server's `fix` fills in only for statuses the CLI has no hint for, and replaces the CLI hint for codes whose cause the status hint gets wrong (`ApiError::status_hint_misleads`: any `target_error`, `private_url_not_allowed`, `payment_required`, `payment_settlement_failed`, and the Keenable-side 429s `search_rate_limited` and `fetch_rate_limited`) when the server sent a `fix`. A 429 that takes the server `fix` leads with the retry time ("Retry after 2s. …"). `authenticated` is whether the failed request was sent with a key, captured at send time (the direct path knows which key it used; the daemon adds `authenticated` to forwarded errors), never re-read from config afterwards (except for daemons too old to report it), so a concurrent login/logout can't flip the hint:
  - 401 / 400 "Authentication failed" → `keenable login`
  - 400 / 422 → fix the request (the message says what was rejected); retrying unchanged fails
  - 402 → out of credits, add credits at https://keenable.ai/console
  - 403 → anonymous: `keenable login` (the feature needs an account); authenticated: the key or plan does not allow this — `keenable login` if the key was revoked, else check the plan at the console
  - 429 → both auth states lead with "Retry after <Retry-After seconds | X-RateLimit-Reset time>" when the API sent either (Retry-After wins); anonymous: then `keenable login` to increase limits; authenticated: then `keenable login` to switch accounts ("Retry later" when no time was sent)

### Help Text & Hints

Every command and subcommand **must** have an `after_help` with:
- Usage examples showing common invocations
- Hints about related commands or next steps (e.g., "After login, run: keenable configure-mcp --all")
- For tool commands, show both default (YAML) and `-p` examples

When adding or modifying a command, always update its help text to stay current.

### Configure MCP Command (`src/commands/configure_mcp.rs`)

`keenable configure-mcp` detects AI clients and shows their configuration status. Client-specific flags trigger configuration.

**Output layout:**
Both modes share a common structure: a "Keenable CLI" section (API key check) followed by a "Your Clients" section.

```
👀  keenable configure-mcp

   Keenable CLI
   ✓  API key is valid              ← ui::success() — green

   Your Clients
   ✓  Claude Code                   ← green, fully configured
   ⚠  Cursor                        ← yellow, configured with issues
      ⚠  Different API key          ← ui::sub_warning()
      We recommend disabling ...    ← ui::sub_hint() — dimmed
   ✗  OpenCode                      ← red, not configured
```

**Two modes:**
- **Status mode** (`keenable configure-mcp`) — shows "Keenable CLI" API key status + "Your Clients" with per-client status (✓ done / ⚠ issues / ✗ not configured) and sub-items for issues and recommendations.
- **Configure mode** (`keenable configure-mcp --cursor`, `--all`, etc.) — same pre-flight, then configures selected clients with interactive confirmation.

**Pre-flight (Keenable CLI section):**
1. API key exists → `ui::error()` + `ui::sub_info()` with fix hint if missing
2. API key is valid (pings search API) → `ui::error()` + `ui::sub_info()` if invalid

**Client detection:**
- Supported: Claude Code, Cursor, Devin Desktop (`~/.config/devin/mcp_config.json`, URL under `url`), Windsurf (legacy `~/.codeium/windsurf/mcp_config.json`, URL under `serverUrl`), Codex, OpenCode
- Detection: parent directory of config file exists

**Client-specific flags:**
`--claude-code`, `--cursor`, `--devin`, `--windsurf`, `--codex`, `--opencode`, `--all`

**Status mode per-client display:**
- **✓ green**: fully configured, no issues. May show dimmed `sub_hint` recommendations.
- **⚠ yellow**: configured but has issues (wrong API key, duplicate entries, conflicting MCPs, standard tools not disabled). Issues shown as `sub_warning` sub-items.
- **✗ red**: not configured. Shows dimmed hint with configure-mcp command.

**Client-specific recommendations** (shown as `sub_hint` under configured clients):
- **Cursor**: "We recommend disabling standard search & fetch tools in Cursor Settings → Tools" and "We recommend setting a custom rule to use Keenable search"

**Per-client configuration (when a flag is set):**
1. **Duplicate cleanup** — removes non-`keenable` entries pointing at `api.keenable.ai` or `api-test.keenable.ai`.
2. **Keenable MCP entry** — adds or updates the `keenable` MCP entry with correct URL and API key.
3. **Standard tools** (Claude Code only) — adds `WebSearch` and `WebFetch` to `permissions.deny` in `~/.claude/settings.json` (the file Claude Code reads; legacy denies in `~/.claude.json` are migrated out). Tools it added are recorded in `~/.keenable/config.json` (`managed_deny_tools`) so reset restores only those.
4. **Conflicting MCPs** — warns about known search MCPs (`brave-search`, `tavily`, `exa`, etc.) but does not auto-remove them.

**Interactive confirmation:**
Before modifying configs, shows a prompt with three options: "Proceed", "Proceed and don't ask again", "Cancel". The "don't ask again" preference is stored in `~/.keenable/config.json` as `skip_setup_confirmation: true`.

**Non-interactive use:** `configure-mcp` and `reset` accept `-y`/`--yes` to skip the confirmation prompt (CI, headless agents). Without `--yes`, a non-TTY stdin exits 1 with a hint instead of hanging on the prompt.

**Adding a new client:** Add an `IDEDef` entry to `all_ides()` in `ide.rs` with `flag`, config path, `servers_key`, and `entry_style`. Also add the corresponding `--flag` to both `ConfigureMcp` and `Reset` commands in `main.rs`. If the client needs recommendations, add them to `show_recommendations()` in `mcp_common.rs`.

### Reset Command (`src/commands/reset.rs`)

`keenable reset` is the inverse of `configure-mcp` — removes Keenable MCP entries and restores default settings.

**Two modes:**
- **Status mode** (`keenable reset`) — shows "Your Clients" section listing clients that have Keenable configured, with per-client reset hints.
- **Reset mode** (`keenable reset --cursor`, `--all`, etc.) — removes Keenable from selected clients with an interactive confirmation prompt.

**Per-client reset:**
1. Removes the `keenable` MCP entry from the IDE config.
2. Removes any other entries pointing at `api.keenable.ai` / `api-test.keenable.ai`.
3. **Claude Code only**: removes from `permissions.deny` the tools recorded in `managed_deny_tools` (all of `WebSearch`/`WebFetch` when no record exists) to re-enable standard tools without undoing denies the user set independently. Cleans up empty `deny`/`permissions` objects.

### Shared IDE Module (`src/commands/ide.rs`)

Contains IDE definitions (`IDEDef`, `all_ides()`), config read/write helpers, and MCP entry utilities shared by both `configure-mcp` and `reset`.

### Adding New Commands

1. Create the command in `src/commands/`
2. Add it to `src/commands/mod.rs`
3. Add the clap variant in `src/main.rs` with `after_help` containing examples and hints
4. If it's a tool command (agent-facing), default to YAML output and support `-p`/`--pretty`
5. If it makes API calls, route through the daemon (supports both auth and unauth)
6. Tool commands should accept `--api-key` for one-off use without stored config; build HTTP clients via `net::client_builder()` (see Network)
7. Tool commands should work without login (use public endpoints when no API key)
8. Use `ui::` helpers for all human output (see Terminal UI Design above)
9. All human output to `stderr`; only machine-readable output (YAML/JSON) to `stdout`

### Browser Callback Pages (`assets/`)

OAuth login callback pages (`login_success.html`, `login_failure.html`) are embedded at compile time via `include_str!`. They match the Keenable landing site's design system (Crimson Pro headings, EB Garamond body, `#f8f7f4` background, inline logo SVG). Keep them self-contained — no external assets except Google Fonts.

## Public Docs and SDK

Every user-facing change ships with its docs. A change is user-facing when it alters a command, a flag, an output format, an error message, or what `configure-mcp` writes into a client's config. For example, a new flag such as `--max-chars` or a new key in the YAML output is user-facing; an internal refactor, a new test, or a dependency bump that changes no behavior is not.

The API surface is the Keenable HTTP API the SDKs wrap: endpoints, request parameters, response fields, error bodies and response headers. Changing how the CLI calls an endpoint without changing the request or response is not an API surface change.

When it is unclear whether a change is user-facing or touches the API surface, treat it as if it does and ask the user (or the PR reviewer) to confirm.

- **Docs:** update [keenable-external-docs](https://github.com/keenableai/keenable-external-docs) in the same change set: open a docs PR alongside this one and link it in the description.
- **SDK:** if the change touches the API surface, also update [keenable-sdk](https://github.com/keenableai/keenable-sdk) (Python, TypeScript and Go), or open an issue there when it can't land in the same change set.
- **Docs reference the SDKs.** Code examples use the official SDKs, with the HTTP API (curl) shown next to them. If the docs describe something the SDK can't do, that's an SDK gap: fix it or file an issue.
- **New user-facing features ship with docs; existing ones are documented, not removed.** If you find an existing user-facing feature that has no docs, document it. Never remove or rename an existing command, flag, output field or key, error message, or `configure-mcp` entry without the user's explicit confirmation: propose the removal in the PR description and wait for the user to approve it.
