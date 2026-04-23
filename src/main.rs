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
    after_help = "Get started:\n  keenable login                       Authenticate with your Keenable account\n  keenable login --api-key sk_abc123   Save API key directly (no browser)\n  keenable configure-mcp               See which clients are configured\n  keenable configure-mcp --all         Configure Keenable MCP in all detected clients\n  keenable search \"query\"              Search the web (YAML output for agents)\n  keenable search \"query\" -p           Same, but pretty-printed for humans"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Keenable and provision an API key
    #[command(after_help = "Authenticates by showing a code to approve in your browser.\nWorks on local machines, remote servers, and agent environments.\n\nWith --api-key, skips browser login and saves the key directly.\nUseful for CI, servers, or agent machines.\n\nAfter login, run: keenable configure-mcp --all\n\nExamples:\n  keenable login                         Interactive browser login\n  keenable login --api-key sk_abc123     Save API key directly (no browser)\n  keenable login --api-key $KEENABLE_API_KEY")]
    Login {
        /// API key to save directly (skips browser login)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Remove stored credentials and API key
    #[command(after_help = "Clears stored tokens and API key from ~/.keenable/")]
    Logout,

    /// Configure Keenable MCP in your AI clients
    #[command(name = "configure-mcp", after_help = "Without flags, shows which clients are detected and configured.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable configure-mcp                  Show status of all detected clients\n  keenable configure-mcp --cursor         Configure Cursor only\n  keenable configure-mcp --all            Configure all detected clients\n  keenable configure-mcp --claude-code --cursor   Configure specific clients")]
    ConfigureMcp {
        /// Configure all detected clients
        #[arg(long)]
        all: bool,

        /// Configure Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Configure Claude Desktop
        #[arg(long)]
        claude_desktop: bool,

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
    #[command(name = "configure-webql", after_help = "Without flags, shows which clients are detected and configured for WebQL.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable configure-webql                  Show status of all detected clients\n  keenable configure-webql --cursor         Configure Cursor only\n  keenable configure-webql --all            Configure all detected clients")]
    ConfigureWebql {
        /// Configure all detected clients
        #[arg(long)]
        all: bool,

        /// Configure Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Configure Claude Desktop
        #[arg(long)]
        claude_desktop: bool,

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
    #[command(name = "reset-webql", after_help = "Without flags, shows which clients have WebQL configured.\nWith client flags, removes WebQL MCP entries.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable reset-webql                  Show which clients can be reset\n  keenable reset-webql --cursor         Reset Cursor only\n  keenable reset-webql --all            Reset all configured clients")]
    ResetWebql {
        /// Reset all configured clients
        #[arg(long)]
        all: bool,

        /// Reset Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Reset Claude Desktop
        #[arg(long)]
        claude_desktop: bool,

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
    #[command(after_help = "Without flags, shows which clients have Keenable configured.\nWith client flags, removes Keenable MCP and restores default settings.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable reset                  Show which clients can be reset\n  keenable reset --cursor         Reset Cursor only\n  keenable reset --all            Reset all configured clients")]
    Reset {
        /// Reset all configured clients
        #[arg(long)]
        all: bool,

        /// Reset Claude Code
        #[arg(long)]
        claude_code: bool,

        /// Reset Claude Desktop
        #[arg(long)]
        claude_desktop: bool,

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

    /// Search the web (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Examples:\n  keenable search \"rust async\"                      YAML output (for agents)\n  keenable search \"rust async\" -p                   Pretty output (for humans)\n  keenable search \"rust async\" -n 5                 Limit to 5 results\n  keenable search \"rust async\" --api-key sk_abc123  Use a specific API key")]
    Search {
        /// Search query
        query: String,

        /// Number of results
        #[arg(short = 'n', long = "count", default_value = "10")]
        count: u32,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Fetch page content as markdown (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Examples:\n  keenable fetch https://example.com                         YAML output\n  keenable fetch https://a.com https://b.com                 Multiple URLs\n  keenable fetch https://example.com -p                      Pretty output\n  keenable fetch https://example.com --api-key sk_abc123     Use a specific API key")]
    Fetch {
        /// URLs to fetch
        urls: Vec<String>,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Submit search relevance feedback (outputs YAML by default, use -p for pretty output)
    #[command(after_help = "Score format: url=score (0=irrelevant, 5=perfect)\n\nExamples:\n  keenable feedback \"rust async\" \"https://tokio.rs=5\" \"https://unrelated.com=1\"\n  keenable feedback \"rust async\" \"https://tokio.rs=5\" -t \"great result\"")]
    Feedback {
        /// Original search query
        query: String,

        /// URL=score pairs (score 0-5)
        scores: Vec<String>,

        /// Free-form feedback text
        #[arg(short = 't', long = "text")]
        text: Option<String>,

        /// Pretty-print output for humans instead of YAML
        #[arg(short = 'p', long = "pretty")]
        pretty: bool,

        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Stdio-to-HTTP bridge for MCP (used by Claude Desktop)
    #[command(name = "mcp-stdio", hide = true)]
    McpStdio {
        /// API key (overrides stored key)
        #[arg(long = "api-key")]
        api_key: Option<String>,

        /// Full MCP URL to proxy (used for WebQL)
        #[arg(long = "url")]
        url: Option<String>,
    },

    /// Run the background daemon (internal, auto-started)
    #[command(hide = true)]
    Daemon,

}

fn collect_client_flags(
    all: bool,
    claude_code: bool,
    claude_desktop: bool,
    cursor: bool,
    windsurf: bool,
    codex: bool,
    opencode: bool,
) -> Vec<String> {
    let pairs: &[(bool, &str)] = &[
        (all, "all"),
        (claude_code, "claude-code"),
        (claude_desktop, "claude-desktop"),
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

    // Non-blocking update check (fire and forget for most commands)
    let update_handle = tokio::spawn(async {
        update::check_for_update().await
    });

    match cli.command {
        Commands::Login { api_key } => {
            commands::login::login(api_key.as_deref()).await;
        }
        Commands::Logout => {
            commands::login::logout();
        }
        Commands::ConfigureMcp {
            all, claude_code, claude_desktop, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, windsurf, codex, opencode);
            commands::configure_mcp::configure_mcp(flags).await;
        }
        Commands::ConfigureWebql {
            all, claude_code, claude_desktop, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, windsurf, codex, opencode);
            commands::configure_webql::configure_webql(flags).await;
        }
        Commands::ResetWebql {
            all, claude_code, claude_desktop, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, windsurf, codex, opencode);
            commands::reset_webql::reset_webql(flags);
        }
        Commands::Reset {
            all, claude_code, claude_desktop, cursor, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, windsurf, codex, opencode);
            commands::reset::reset(flags);
        }
        Commands::Search { query, count, pretty, api_key } => {
            commands::search::search(&query, count, pretty, api_key.as_deref()).await;
        }
        Commands::Fetch { urls, pretty, api_key } => {
            commands::search::fetch(&urls, pretty, api_key.as_deref()).await;
        }
        Commands::Feedback {
            query,
            scores,
            text,
            pretty,
            api_key,
        } => {
            commands::search::feedback(&query, &scores, text.as_deref(), pretty, api_key.as_deref()).await;
        }
        Commands::McpStdio { api_key, url } => {
            commands::mcp_stdio::run(api_key.as_deref(), url.as_deref()).await;
        }
        Commands::Daemon => {
            daemon::run_daemon().await;
        }
    }

    // Show update notification if available
    if let Ok(Some(version)) = update_handle.await {
        use colored::Colorize;
        eprintln!(
            "\n{} A newer version of keenable ({}) is available. Run:\n  {}",
            "Update:".yellow().bold(),
            version,
            update::install_hint().cyan()
        );
    }
}
