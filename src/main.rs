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
    after_help = "Get started:\n  keenable login              Authenticate with your Keenable account\n  keenable setup              See which clients are configured\n  keenable setup --all        Configure Keenable MCP in all detected clients\n  keenable search \"query\"     Search the web (YAML output for agents)\n  keenable search \"query\" -p  Same, but pretty-printed for humans"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate with Keenable and provision an API key
    #[command(after_help = "Authenticates by showing a code to approve in your browser.\nWorks on local machines, remote servers, and agent environments.\n\nAfter login, run: keenable setup --all\n\nFor pre-existing API keys (CI, scripts), use keenable configure instead.")]
    Login,

    /// Remove stored credentials and API key
    #[command(after_help = "Clears stored tokens and API key from ~/.keenable/")]
    Logout,

    /// Configure CLI with an API key (for agentic/headless use)
    #[command(after_help = "Use this on CI, servers, or agent machines where browser login isn't possible.\nSaves the API key locally so search and fetch commands work.\n\nNote: MCP IDE configuration requires keenable login instead.\n\nExamples:\n  keenable configure --api-key sk_abc123\n  keenable configure --api-key $KEENABLE_API_KEY")]
    Configure {
        /// API key to save
        #[arg(long = "api-key")]
        api_key: String,
    },

    /// Configure Keenable MCP in your AI clients
    #[command(after_help = "Without flags, shows which clients are detected and configured.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor, --vscode,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable setup                  Show status of all detected clients\n  keenable setup --cursor         Configure Cursor only\n  keenable setup --all            Configure all detected clients\n  keenable setup --claude-code --vscode   Configure specific clients")]
    Setup {
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

        /// Configure VS Code
        #[arg(long)]
        vscode: bool,

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
    #[command(after_help = "Without flags, shows which clients have Keenable configured.\nWith client flags, removes Keenable MCP and restores default settings.\n\nSupported clients:\n  --claude-code, --claude-desktop, --cursor, --vscode,\n  --windsurf, --codex, --opencode\n\nExamples:\n  keenable reset                  Show which clients can be reset\n  keenable reset --cursor         Reset Cursor only\n  keenable reset --all            Reset all configured clients")]
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

        /// Reset VS Code
        #[arg(long)]
        vscode: bool,

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
    vscode: bool,
    windsurf: bool,
    codex: bool,
    opencode: bool,
) -> Vec<String> {
    let pairs: &[(bool, &str)] = &[
        (all, "all"),
        (claude_code, "claude-code"),
        (claude_desktop, "claude-desktop"),
        (cursor, "cursor"),
        (vscode, "vscode"),
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
        Commands::Login => {
            commands::login::login().await;
        }
        Commands::Logout => {
            commands::login::logout();
        }
        Commands::Configure { api_key } => {
            commands::configure::configure(&api_key);
        }
        Commands::Setup {
            all, claude_code, claude_desktop, cursor, vscode, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, vscode, windsurf, codex, opencode);
            commands::setup::setup(flags).await;
        }
        Commands::Reset {
            all, claude_code, claude_desktop, cursor, vscode, windsurf, codex, opencode,
        } => {
            let flags = collect_client_flags(all, claude_code, claude_desktop, cursor, vscode, windsurf, codex, opencode);
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
        Commands::McpStdio { api_key } => {
            commands::mcp_stdio::run(api_key.as_deref()).await;
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
