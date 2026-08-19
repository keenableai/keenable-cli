mod api;
mod commands;
mod config;
mod constants;
mod daemon;
mod ui;
mod update;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "keenable",
    about = "Keenable CLI — authenticate, manage API keys, configure MCP, and search the web",
    version,
    after_help = "Get started:\n  keenable search \"query\"                  Search the web (works without login)\n  keenable search \"query\" -p               Same, but pretty-printed for humans\n  keenable login                           Authenticate (agent-friendly device flow)\n  keenable login --api-key keen_***_*****  Save API key directly\n  keenable configure-mcp --all             Configure Keenable MCP in all detected clients"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set a config value
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// Remove a config value
    Unset {
        /// Config key
        key: String,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Keenable and provision an API key
    #[command(
        after_help = "Agent-friendly device flow: shows a code for the user to approve.\nWorks on local machines, remote servers, and agent environments.\n\nWith --api-key, saves the key directly (useful for CI and servers).\n\nAfter login, run: keenable configure-mcp --all\n\nExamples:\n  keenable login                             Device flow (agent-friendly)\n  keenable login --api-key keen_***_*****    Save API key directly\n  keenable login --api-key $KEENABLE_API_KEY"
    )]
    Login {
        /// API key to save directly (skips browser login)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Remove stored credentials and API key
    #[command(after_help = "Clears stored tokens and API key from ~/.keenable/")]
    Logout,

    /// Configure Keenable MCP in your AI clients
    #[command(
        name = "configure-mcp",
        after_help = "Without flags, shows which clients are detected and configured.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable configure-mcp                  Show status of all detected clients\n  keenable configure-mcp --cursor         Configure Cursor only\n  keenable configure-mcp --all            Configure all detected clients\n  keenable configure-mcp --claude-code --cursor   Configure specific clients\n  keenable configure-mcp --all --yes      Configure without confirmation (CI, agents)"
    )]
    ConfigureMcp {
        /// Configure all detected clients
        #[arg(long)]
        all: bool,

        /// Skip the confirmation prompt (for CI and non-interactive use)
        #[arg(short, long)]
        yes: bool,

        /// Configure Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Configure Cursor
        #[arg(long)]
        cursor: bool,

        /// Configure Windsurf
        #[arg(long)]
        windsurf: bool,

        /// Configure Codex
        #[arg(long)]
        codex: bool,

        /// Configure OpenCode
        #[arg(long)]
        opencode: bool,
    },

    /// Remove Keenable MCP from your AI clients and restore defaults
    #[command(
        after_help = "Without flags, shows which clients have Keenable configured.\nWith client flags, removes Keenable MCP and restores default settings.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable reset                  Show which clients can be reset\n  keenable reset --cursor         Reset Cursor only\n  keenable reset --all            Reset all configured clients\n  keenable reset --all --yes      Reset without confirmation (CI, agents)"
    )]
    Reset {
        /// Reset all configured clients
        #[arg(long)]
        all: bool,

        /// Skip the confirmation prompt (for CI and non-interactive use)
        #[arg(short, long)]
        yes: bool,

        /// Reset Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Reset Cursor
        #[arg(long)]
        cursor: bool,

        /// Reset Windsurf
        #[arg(long)]
        windsurf: bool,

        /// Reset Codex
        #[arg(long)]
        codex: bool,

        /// Reset OpenCode
        #[arg(long)]
        opencode: bool,
    },

    /// View or modify CLI configuration
    #[command(
        after_help = "View all settings:\n  keenable config\n\nSet a value:\n  keenable config set <key> <value>\n\nGet a single value:\n  keenable config get <key>\n\nRemove a value:\n  keenable config unset <key>\n\nRun `keenable config` to see the available keys and their allowed values."
    )]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Search the web (outputs YAML by default, use -p for pretty output)
    #[command(
        after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable search \"rust async\"                                    YAML output (for agents)\n  keenable search \"rust async\" -p                                 Pretty output (for humans)\n  keenable search \"AI news\" --site techcrunch.com                 Restrict to site\n  keenable search \"dodgers braves\" --published-after 2026-01-01   Date filter (YYYY-MM-DD)\n  keenable search \"AI news\" --acquired-after 7d                   Relative date (min, h, d, mo, y)\n  keenable search \"AI news\" --acquired-after 2026-01-15T10:30:00Z ISO 8601 datetime\n  keenable search \"rust async\" --snippet-max-length 2000          Longer snippets (180-10000)\n  keenable search \"rust async\" --max-results 25                   More results (1-50, default 10)\n  keenable search \"AI news\" --query-time 2026-01-01T00:00:00Z     Point-in-time search\n  keenable search \"rust async\" --api-key keen_***_*****                Use a specific API key"
    )]
    Search {
        /// Search query
        query: String,

        /// Search mode
        #[arg(long)]
        mode: Option<String>,

        /// Restrict results to a specific site (e.g. "docs.rs")
        #[arg(long)]
        site: Option<String>,

        /// Filter to pages acquired/indexed after this date (YYYY-MM-DD, ISO 8601, or relative e.g. 7d, 3mo, 1y)
        #[arg(long)]
        acquired_after: Option<String>,

        /// Filter to pages acquired/indexed before this date (YYYY-MM-DD, ISO 8601, or relative e.g. 7d, 3mo, 1y)
        #[arg(long)]
        acquired_before: Option<String>,

        /// Filter to pages published after this date (YYYY-MM-DD, ISO 8601, or relative e.g. 7d, 3mo, 1y)
        #[arg(long)]
        published_after: Option<String>,

        /// Filter to pages published before this date (YYYY-MM-DD, ISO 8601, or relative e.g. 7d, 3mo, 1y)
        #[arg(long)]
        published_before: Option<String>,

        /// Maximum snippet length in characters (API accepts 180-10000)
        #[arg(long = "snippet-max-length")]
        snippet_max_length: Option<u64>,

        /// Maximum number of results (API accepts 1-50, default 10)
        #[arg(long = "max-results")]
        max_results: Option<u64>,

        /// Point-in-time search: only pages available on or before this timestamp (YYYY-MM-DD, ISO 8601, or relative e.g. 7d, 3mo, 1y)
        #[arg(long)]
        query_time: Option<String>,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Fetch page content as markdown (outputs YAML by default, use -p for pretty output)
    #[command(
        after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable fetch https://example.com                         YAML output\n  keenable fetch https://example.com -p                      Pretty output\n  keenable fetch https://example.com --live                  Fetch the live page (skip cache)\n  keenable fetch https://example.com --prompt \"List all pricing tiers\"     Extract with an LLM\n  keenable fetch https://example.com --max-chars 200000      Raise the 50000-char content cap\n  keenable fetch https://example.com --api-key keen_***_*****     Use a specific API key"
    )]
    Fetch {
        /// URL to fetch
        url: String,

        /// Fetch the live page instead of the cached copy
        #[arg(long)]
        live: bool,

        /// Extraction instruction: an LLM reads the page and returns only
        /// this instruction's output instead of the full page (max 2000 chars)
        #[arg(long)]
        prompt: Option<String>,

        /// Truncate content at this many characters (default: 50000)
        #[arg(long = "max-chars", value_parser = clap::value_parser!(u64).range(1..))]
        max_chars: Option<u64>,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Update keenable to the latest version
    #[command(
        after_help = "Downloads and installs the latest release from GitHub.\nWorks for installs made with the Keenable installer scripts.\nHomebrew installs: use `brew update && brew upgrade keenable-cli` instead.\n\nExamples:\n  keenable update       Update to the latest version\n  keenable --version    Show the current version"
    )]
    Update,

    /// Run the background daemon (internal, auto-started)
    #[command(hide = true)]
    Daemon,
}

fn collect_client_flags(
    all: bool,
    claude_code: bool,
    cursor: bool,
    windsurf: bool,
    codex: bool,
    opencode: bool,
) -> Vec<String> {
    let pairs: &[(bool, &str)] = &[
        (all, "all"),
        (claude_code, "claude-code"),
        (cursor, "cursor"),
        (windsurf, "windsurf"),
        (codex, "codex"),
        (opencode, "opencode"),
    ];
    pairs
        .iter()
        .filter(|(set, _)| *set)
        .map(|(_, name)| name.to_string())
        .collect()
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Update check only for human-facing output: awaiting it would add up to
    // ~5s (on cache miss) to agent-facing YAML commands and the daemon.
    // `update` is excluded because it performs its own explicit check.
    let wants_update_check = match &cli.command {
        Commands::Search { pretty, .. } | Commands::Fetch { pretty, .. } => *pretty,
        Commands::Daemon | Commands::Update => false,
        _ => true,
    };
    let update_handle =
        wants_update_check.then(|| tokio::spawn(async { update::check_for_update().await }));

    match cli.command {
        Commands::Login { api_key } => {
            commands::login::login(api_key.as_deref()).await;
        }
        Commands::Logout => {
            commands::login::logout();
        }
        Commands::ConfigureMcp {
            all,
            yes,
            claude_code,
            cursor,
            windsurf,
            codex,
            opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::configure_mcp::configure_mcp(flags, yes).await;
        }
        Commands::Reset {
            all,
            yes,
            claude_code,
            cursor,
            windsurf,
            codex,
            opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::reset::reset(flags, yes);
        }
        Commands::Config { action } => match action {
            None => commands::config_cmd::config_view(),
            Some(ConfigAction::Set { key, value }) => {
                commands::config_cmd::config_set(&key, &value)
            }
            Some(ConfigAction::Get { key }) => commands::config_cmd::config_get(&key),
            Some(ConfigAction::Unset { key }) => commands::config_cmd::config_unset(&key),
        },
        Commands::Search {
            query,
            mode,
            site,
            acquired_after,
            acquired_before,
            published_after,
            published_before,
            snippet_max_length,
            max_results,
            query_time,
            pretty,
            api_key,
        } => {
            let filters = commands::search::SearchFilters {
                site,
                acquired_after,
                acquired_before,
                published_after,
                published_before,
                query_time,
            };
            commands::search::search(
                &query,
                mode.as_deref(),
                filters,
                snippet_max_length,
                max_results,
                pretty,
                api_key.as_deref(),
            )
            .await;
        }
        Commands::Fetch {
            url,
            live,
            prompt,
            max_chars,
            pretty,
            api_key,
        } => {
            commands::search::fetch(&url, live, prompt, max_chars, pretty, api_key.as_deref())
                .await;
        }
        Commands::Update => {
            commands::update_cmd::update().await;
        }
        Commands::Daemon => {
            daemon::run_daemon().await;
        }
    }

    // Show update notification if available
    if let Some(handle) = update_handle
        && let Ok(Some(version)) = handle.await
    {
        eprintln!();
        ui::warning(&format!(
            "A newer version of keenable ({}) is available",
            version
        ));
        ui::hint(&format!("Run: {}", update::update_hint()));
    }
}
