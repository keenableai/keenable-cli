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
    after_help = "Get started:\n  keenable search \"query\" --mode pro       Search the web (works without login)\n  keenable search \"query\" --mode pro -p    Same, but pretty-printed for humans\n  keenable login                           Authenticate (agent-friendly device flow)\n  keenable login --api-key keen_***_*****  Save API key directly\n  keenable configure-mcp --all             Configure Keenable MCP in all detected clients"
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
    #[command(after_help = "Agent-friendly device flow: shows a code for the user to approve.\nWorks on local machines, remote servers, and agent environments.\n\nWith --api-key, saves the key directly (useful for CI and servers).\n\nAfter login, run: keenable configure-mcp --all\n\nExamples:\n  keenable login                             Device flow (agent-friendly)\n  keenable login --api-key keen_***_*****    Save API key directly\n  keenable login --api-key $KEENABLE_API_KEY")]
    Login {
        /// API key to save directly (skips browser login)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Remove stored credentials and API key
    #[command(after_help = "Clears stored tokens and API key from ~/.keenable/")]
    Logout,

    /// Configure Keenable MCP in your AI clients
    #[command(name = "configure-mcp", after_help = "Without flags, shows which clients are detected and configured.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable configure-mcp                  Show status of all detected clients\n  keenable configure-mcp --cursor         Configure Cursor only\n  keenable configure-mcp --all            Configure all detected clients\n  keenable configure-mcp --claude-code --cursor   Configure specific clients\n  keenable configure-mcp --all --yes      Configure without confirmation (CI, agents)")]
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

    /// Configure Keenable WebQL MCP in your AI clients
    #[command(name = "configure-webql", after_help = "Without flags, shows which clients are detected and configured for WebQL.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable configure-webql                  Show status of all detected clients\n  keenable configure-webql --cursor         Configure Cursor only\n  keenable configure-webql --all            Configure all detected clients\n  keenable configure-webql --all --yes      Configure without confirmation (CI, agents)")]
    ConfigureWebql {
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

    /// Remove Keenable WebQL MCP from your AI clients
    #[command(name = "reset-webql", after_help = "Without flags, shows which clients have WebQL configured.\nWith client flags, removes WebQL MCP entries.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable reset-webql                  Show which clients can be reset\n  keenable reset-webql --cursor         Reset Cursor only\n  keenable reset-webql --all            Reset all configured clients\n  keenable reset-webql --all --yes      Reset without confirmation (CI, agents)")]
    ResetWebql {
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

    /// Remove Keenable MCP from your AI clients and restore defaults
    #[command(after_help = "Without flags, shows which clients have Keenable configured.\nWith client flags, removes Keenable MCP and restores default settings.\n\nSupported clients:\n  --claude-code, --cursor, --windsurf,\n  --codex, --opencode\n\nExamples:\n  keenable reset                  Show which clients can be reset\n  keenable reset --cursor         Reset Cursor only\n  keenable reset --all            Reset all configured clients\n  keenable reset --all --yes      Reset without confirmation (CI, agents)")]
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
    #[command(after_help = "View all settings:\n  keenable config\n\nSet a value:\n  keenable config set default_search_mode pro\n  keenable config set forced_search_mode realtime\n\nGet a single value:\n  keenable config get default_search_mode\n\nRemove a value:\n  keenable config unset forced_search_mode\n\nSupported keys:\n  default_search_mode   Search mode when --mode is not specified (realtime, pro)\n  forced_search_mode    Always use this mode, ignoring --mode (realtime, pro)")]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },

    /// Search the web (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Works without login (free tier). Log in for higher rate limits.\n\nModes:\n  --mode realtime   Fast results\n  --mode pro        Higher quality (default)\n\nSet a default: keenable config set default_search_mode realtime\nForce a mode:  keenable config set forced_search_mode realtime\n\nExamples:\n  keenable search \"rust async\"                                    YAML output (for agents)\n  keenable search \"rust async\" -p                                 Pretty output (for humans)\n  keenable search \"rust async\" --mode pro                         Use pro mode (higher quality)\n  keenable search \"AI news\" --site techcrunch.com                 Restrict to site\n  keenable search \"dodgers braves\" --published-after 2026-01-01   Date filter (YYYY-MM-DD)\n  keenable search \"AI news\" --acquired-after 7d                   Relative date (min, h, d, mo, y)\n  keenable search \"AI news\" --acquired-after 2026-01-15T10:30:00Z ISO 8601 datetime\n  keenable search \"rust async\" --api-key keen_***_*****                Use a specific API key")]
    Search {
        /// Search query
        query: String,

        /// Search mode: "realtime" (fast) or "pro" (higher quality, default)
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

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Fetch page content as markdown (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable fetch https://example.com                         YAML output\n  keenable fetch https://example.com -p                      Pretty output\n  keenable fetch https://example.com --api-key keen_***_*****     Use a specific API key")]
    Fetch {
        /// URL to fetch
        url: String,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Submit search relevance feedback (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Works without login (free tier). Log in for higher rate limits.\n\nScore format: url=score=comment (0=irrelevant, 5=perfect; comment is required)\n\nExamples:\n  keenable feedback \"rust async\" \"https://tokio.rs=5=great overview\" \"https://unrelated.com=1=off topic\"")]
    Feedback {
        /// Original search query
        query: String,

        /// URL=score=comment entries (score 0-5, comment required)
        scores: Vec<String>,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

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
    let wants_update_check = match &cli.command {
        Commands::Search { pretty, .. }
        | Commands::Fetch { pretty, .. }
        | Commands::Feedback { pretty, .. } => *pretty,
        Commands::Daemon => false,
        _ => true,
    };
    let update_handle = wants_update_check.then(|| {
        tokio::spawn(async { update::check_for_update().await })
    });

    match cli.command {
        Commands::Login { api_key } => {
            commands::login::login(api_key.as_deref()).await;
        }
        Commands::Logout => {
            commands::login::logout();
        }
        Commands::ConfigureMcp {
            all, yes, claude_code, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::configure_mcp::configure_mcp(flags, yes).await;
        }
        Commands::ConfigureWebql {
            all, yes, claude_code, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::configure_webql::configure_webql(flags, yes).await;
        }
        Commands::ResetWebql {
            all, yes, claude_code, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::reset_webql::reset_webql(flags, yes);
        }
        Commands::Reset {
            all, yes, claude_code, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, cursor, windsurf, codex, opencode);
            commands::reset::reset(flags, yes);
        }
        Commands::Config { action } => {
            match action {
                None => commands::config_cmd::config_view(),
                Some(ConfigAction::Set { key, value }) => commands::config_cmd::config_set(&key, &value),
                Some(ConfigAction::Get { key }) => commands::config_cmd::config_get(&key),
                Some(ConfigAction::Unset { key }) => commands::config_cmd::config_unset(&key),
            }
        }
        Commands::Search { query, mode, site, acquired_after, acquired_before, published_after, published_before, pretty, api_key } => {
            let filters = commands::search::SearchFilters {
                site, acquired_after, acquired_before, published_after, published_before,
            };
            commands::search::search(&query, mode.as_deref(), filters, pretty, api_key.as_deref()).await;
        }
        Commands::Fetch { url, pretty, api_key } => {
            commands::search::fetch(&url, pretty, api_key.as_deref()).await;
        }
        Commands::Feedback {
            query,
            scores,
            pretty,
            api_key,
        } => {
            commands::search::feedback(&query, &scores, pretty, api_key.as_deref()).await;
        }
        Commands::Daemon => {
            daemon::run_daemon().await;
        }
    }

    // Show update notification if available
    if let Some(handle) = update_handle {
        if let Ok(Some(version)) = handle.await {
            use colored::Colorize;
            eprintln!(
                "\n{} A newer version of keenable ({}) is available. Run:\n  {}",
                "Update:".yellow().bold(),
                version,
                update::install_hint().cyan()
            );
        }
    }
}
