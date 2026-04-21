# Keenable CLI

Rust CLI for Keenable — authenticate, manage API keys, configure MCP, and search the web.

## Build

```bash
cargo build --release
```

## Project Structure

```
src/
  main.rs              # CLI entry point, clap definitions
  ui.rs                # Shared terminal UI helpers (header, step_done, success, error, hint)
  constants.rs         # API URLs, OAuth config, ports
  config.rs            # ~/.keenable/ config and credentials read/write
  api.rs               # HTTP client factories (api_key_client, bearer_client)
  update.rs            # GitHub Releases version check (cached, non-blocking)
  daemon.rs            # Background daemon for connection reuse
  commands/
    login.rs           # OAuth 2.1 PKCE flow
    logout (in login.rs)
    configure.rs       # Save API key for headless/agentic use
    ide.rs             # Shared IDE definitions and config helpers
    setup.rs           # Client detection, MCP configuration, interactive setup
    reset.rs           # Remove Keenable MCP and restore defaults
    keys.rs            # API key create (requires login)
    search.rs          # search, fetch, feedback commands
assets/
  login_success.html   # Styled OAuth callback success page
  login_failure.html   # Styled OAuth callback failure page
```

## Conventions

### Two Setup Modes

- **`keenable login`** — for personal machines. OAuth login, auto-provisions API key, enables MCP configuration via `setup`.
- **`keenable configure --api-key <KEY>`** — for CI, servers, agent machines. Saves API key only. No browser needed. MCP IDE configuration not supported (requires login).

### Output Format

All tool commands (`search`, `fetch`, `feedback`) output **YAML by default** (token-efficient for agents).
Use `-p` / `--pretty` flag for pretty-printed human-readable output.
Use `--api-key <KEY>` to override the stored key for one-off calls.

```bash
keenable search "query"                        # YAML output (for agents)
keenable search "query" -p                     # Pretty output (for humans)
keenable search "query" --api-key sk_abc123    # Use specific API key
```

Management commands (`login`, `logout`, `configure`, `setup`, `keys-create`) always output human-readable text.

### Daemon

The CLI uses a background daemon (`~/.keenable/daemon.sock`) to reuse HTTP connections.
Commands auto-start the daemon on first call. It auto-exits after 5 minutes of idle.

### Terminal UI Design (`src/ui.rs`)

All human-facing output uses the shared `ui` module for consistent styling. The 👀 emoji is the CLI's brand mark.

**Top-level steps** (used by `login`, `logout`, `configure`, etc.):

```
👀  keenable login                       ← ui::header()

   ✓  Discovered OAuth endpoints          ← ui::step_done()  — dimmed + strikethrough
   ✓  Opened browser                      ← ui::step_done()
   ✓  Logged in as user@example.com       ← ui::success()    — green

   Next: keenable setup --all              ← ui::hint()       — dimmed
```

**Sub-steps** (used by `setup` for per-client configuration):

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

### Error Handling

- All human output to `stderr` via `eprintln!` or `ui::` helpers; machine output to `stdout` via `println!`
- Exit with code 1 on errors
- Always suggest the fix (e.g., "Run `keenable login` first") — use `ui::error()` + `ui::hint()`

### Help Text & Hints

Every command and subcommand **must** have an `after_help` with:
- Usage examples showing common invocations
- Hints about related commands or next steps (e.g., "After login, run: keenable setup --all")
- For tool commands, show both default (YAML) and `-p` examples

When adding or modifying a command, always update its help text to stay current.

### Setup Command (`src/commands/setup.rs`)

`keenable setup` detects AI clients and shows their configuration status. Client-specific flags trigger configuration.

**Output layout:**
Both modes share a common structure: a "Keenable CLI" section (API key check) followed by a "Your Clients" section.

```
👀  keenable setup

   Keenable CLI
   ✓  API key is valid              ← ui::success() — green

   Your Clients
   ✓  Claude Code                   ← green, fully configured
   ⚠  Cursor                        ← yellow, configured with issues
      ⚠  Different API key          ← ui::sub_warning()
      We recommend disabling ...    ← ui::sub_hint() — dimmed
   ✗  VS Code                       ← red, not configured
```

**Two modes:**
- **Status mode** (`keenable setup`) — shows "Keenable CLI" API key status + "Your Clients" with per-client status (✓ done / ⚠ issues / ✗ not configured) and sub-items for issues and recommendations.
- **Configure mode** (`keenable setup --cursor`, `--all`, etc.) — same pre-flight, then configures selected clients with interactive confirmation.

**Pre-flight (Keenable CLI section):**
1. API key exists → `ui::error()` + `ui::sub_info()` with fix hint if missing
2. API key is valid (pings search API) → `ui::error()` + `ui::sub_info()` if invalid

**Client detection:**
- Supported: Claude Code, Claude Desktop, Cursor, VS Code, Windsurf, Codex
- Detection: parent directory of config file exists

**Client-specific flags:**
`--claude-code`, `--claude-desktop`, `--cursor`, `--vscode`, `--windsurf`, `--codex`, `--all`

**Status mode per-client display:**
- **✓ green**: fully configured, no issues. May show dimmed `sub_hint` recommendations.
- **⚠ yellow**: configured but has issues (wrong API key, duplicate entries, conflicting MCPs, standard tools not disabled). Issues shown as `sub_warning` sub-items.
- **✗ red**: not configured. Shows dimmed hint with setup command.

**Client-specific recommendations** (shown as `sub_hint` under configured clients):
- **Claude Desktop**: "Disable built-in web search manually (+ button near the chat text field)"
- **Cursor**: "We recommend disabling standard search & fetch tools in Cursor Settings → Tools" and "We recommend setting a custom rule to use Keenable search"

**Per-client configuration (when a flag is set):**
1. **Duplicate cleanup** — removes non-`keenable` entries pointing at `api.keenable.ai` or `api-test.keenable.ai`.
2. **Keenable MCP entry** — adds or updates the `keenable` MCP entry with correct URL and API key.
3. **Standard tools** (Claude Code only) — adds `WebSearch` and `WebFetch` to `permissions.deny`.
4. **Conflicting MCPs** — warns about known search MCPs (`brave-search`, `tavily`, `exa`, etc.) but does not auto-remove them.

**Interactive confirmation:**
Before modifying configs, shows a prompt with three options: "Proceed", "Proceed and don't ask again", "Cancel". The "don't ask again" preference is stored in `~/.keenable/config.json` as `skip_setup_confirmation: true`.

**Adding a new client:** Add an `IDEDef` entry to `all_ides()` in `ide.rs` with `flag`, config path, `servers_key`, and `entry_style`. Also add the corresponding `--flag` to both `Setup` and `Reset` commands in `main.rs`. If the client needs recommendations, add them to `show_client_recommendations()` in `setup.rs`.

### Reset Command (`src/commands/reset.rs`)

`keenable reset` is the inverse of `setup` — removes Keenable MCP entries and restores default settings.

**Two modes:**
- **Status mode** (`keenable reset`) — shows "Your Clients" section listing clients that have Keenable configured, with per-client reset hints.
- **Reset mode** (`keenable reset --cursor`, `--all`, etc.) — removes Keenable from selected clients with an interactive confirmation prompt.

**Per-client reset:**
1. Removes the `keenable` MCP entry from the IDE config.
2. Removes any other entries pointing at `api.keenable.ai` / `api-test.keenable.ai`.
3. **Claude Code only**: removes `WebSearch` and `WebFetch` from `permissions.deny` to re-enable standard tools. Cleans up empty `deny`/`permissions` objects.

### Shared IDE Module (`src/commands/ide.rs`)

Contains IDE definitions (`IDEDef`, `all_ides()`), config read/write helpers, and MCP entry utilities shared by both `setup` and `reset`.

### Adding New Commands

1. Create the command in `src/commands/`
2. Add it to `src/commands/mod.rs`
3. Add the clap variant in `src/main.rs` with `after_help` containing examples and hints
4. If it's a tool command (agent-facing), default to YAML output and support `-p`/`--pretty`
5. If it makes API calls, route through the daemon client (skipped when `--api-key` is passed directly)
6. Tool commands should accept `--api-key` for one-off use without stored config
7. Use `ui::` helpers for all human output (see Terminal UI Design above)
8. All human output to `stderr`; only machine-readable output (YAML/JSON) to `stdout`

### Browser Callback Pages (`assets/`)

OAuth login callback pages (`login_success.html`, `login_failure.html`) are embedded at compile time via `include_str!`. They match the Keenable landing site's design system (Crimson Pro headings, EB Garamond body, `#f8f7f4` background, inline logo SVG). Keep them self-contained — no external assets except Google Fonts.
