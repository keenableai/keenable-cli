/// `eprintln!` that never panics: when stderr's reader is gone (`2>&1 | head`)
/// the rest of the human output is dropped instead of crashing with exit 101.
macro_rules! eprintln {
    () => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr());
    }};
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

/// `eprint!` that never panics (see `eprintln!`).
macro_rules! eprint {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stderr(), $($arg)*);
    }};
}

mod api;
mod commands;
mod config;
mod constants;
mod daemon;
mod net;
mod ui;
mod update;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "keenable",
    about = "Keenable CLI — authenticate, manage API keys, configure MCP, and search the web",
    version,
    after_help = "Get started:\n  keenable search \"query\"                  Search the web (works without login)\n  keenable search \"query\" -p               Same, but pretty-printed for humans\n  keenable login                           Authenticate (agent-friendly device flow)\n  keenable login --api-key keen_***_*****  Save API key directly\n  keenable configure-mcp --all             Configure Keenable MCP in all detected clients\n\nNetwork:\n  Trusts the bundled Mozilla roots plus your OS certificate store.\n  SSL_CERT_FILE / SSL_CERT_DIR  PEM CA certificates to trust instead of the OS store\n                                (e.g. your TLS-inspecting proxy's CA)\n  HTTPS_PROXY / ALL_PROXY       Proxy URL (http://user:password@host:port); NO_PROXY to bypass\n  keenable search \"query\" -p -v            Show the proxy, trusted CAs and the full error chain"
)]
struct Cli {
    /// Show network details: proxy in use, trusted CA sources, and the full
    /// error chain on failure (also bypasses the background daemon)
    #[arg(short = 'v', long, global = true)]
    verbose: bool,

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
        after_help = "Agent-friendly device flow: shows a code for the user to approve.\nWorks on local machines, remote servers, and agent environments.\n\nWith --api-key, saves the key directly (useful for CI and servers).\n\nAfter login, run: keenable configure-mcp --all\n\nExamples:\n  keenable login                             Device flow (agent-friendly)\n  printenv KEENABLE_API_KEY | keenable login --api-key -\n                                             Save a key from stdin (stays out of\n                                             the process list and shell history)\n  keenable login --api-key keen_***_*****    Save API key directly\n  keenable login -v                          Show proxy, trusted CAs and full errors\n\nBehind a corporate proxy: set HTTPS_PROXY, and SSL_CERT_FILE to your company's CA bundle\nif it inspects TLS."
    )]
    Login {
        /// API key to save directly (skips browser login); `-` reads it from stdin
        #[arg(long = "api-key")]
        api_key: Option<String>,
    },

    /// Remove stored credentials and API key
    #[command(after_help = "Clears stored tokens and API key from ~/.keenable/")]
    Logout,

    /// Configure Keenable MCP in your AI clients
    #[command(
        name = "configure-mcp",
        after_help = "Without flags, shows which clients are detected and configured.\nWith client flags, configures the selected clients.\n\nSupported clients:\n  --claude-code, --cursor, --devin (Devin Desktop),\n  --windsurf (legacy Windsurf path), --codex, --opencode\n\nExamples:\n  keenable configure-mcp                  Show status of all detected clients\n  keenable configure-mcp --cursor         Configure Cursor only\n  keenable configure-mcp --all            Configure all detected clients\n  keenable configure-mcp --claude-code --cursor   Configure specific clients\n  keenable configure-mcp --all --yes      Configure without confirmation (CI, agents)"
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

        /// Configure Devin Desktop (~/.config/devin/mcp_config.json)
        #[arg(long)]
        devin: bool,

        /// Configure Windsurf (legacy ~/.codeium/windsurf/mcp_config.json)
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
        after_help = "Without flags, shows which clients have Keenable configured.\nWith client flags, removes Keenable MCP and restores default settings.\n\nSupported clients:\n  --claude-code, --cursor, --devin (Devin Desktop),\n  --windsurf (legacy Windsurf path), --codex, --opencode\n\nExamples:\n  keenable reset                  Show which clients can be reset\n  keenable reset --cursor         Reset Cursor only\n  keenable reset --all            Reset all configured clients\n  keenable reset --all --yes      Reset without confirmation (CI, agents)"
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

        /// Reset Devin Desktop (~/.config/devin/mcp_config.json)
        #[arg(long)]
        devin: bool,

        /// Reset Windsurf (legacy ~/.codeium/windsurf/mcp_config.json)
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
        after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable search \"rust async\"                                    YAML output (for agents)\n  keenable search \"rust async\" -p                                 Pretty output (for humans)\n  keenable search \"AI news\" --mode realtime                       Fastest response (default mode: pro)\n  keenable search \"AI news\" --site techcrunch.com                 Restrict to site\n  keenable search \"AI news\" --exclude-domain spam.com,ads.net     Never these domains (repeatable)\n  keenable search \"dodgers braves\" --published-after 2026-01-01   Date filter (YYYY-MM-DD)\n  keenable search \"AI news\" --acquired-after 7d                   Relative date (min, h, d, w, mo, y)\n  keenable search \"AI news\" --acquired-after 2026-01-15T10:30:00Z ISO 8601 datetime\n  keenable search \"rust async\" --snippet-max-length 2000          Longer snippets (180-10000)\n  keenable search \"rust async\" --max-results 25                   More results (1-50, default 10)\n  keenable search \"AI news\" --query-time 2026-01-01T00:00:00Z     Point-in-time search\n  keenable search \"rust async\" --session-id agent-run-42           Group related calls in a session\n  keenable search \"rust async\" --api-key keen_***_*****                Use a specific API key\n  keenable search \"rust async\" -p -v                              Show proxy, trusted CAs and full errors\n\nExit codes:\n  0 success   1 other error   2 invalid arguments   3 authentication or permission\n  4 rate limited   5 out of credits   6 the page or site failed (fetch)   7 network\n  8 invalid request   9 server error (retry later)\nOn an error, YAML mode prints the structured error on stdout and a one-line summary on stderr."
    )]
    Search(SearchArgs),

    /// Fetch page content as markdown (outputs YAML by default, use -p for pretty output)
    #[command(
        after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable fetch https://example.com                         YAML output\n  keenable fetch https://example.com -p                      Pretty output\n  keenable fetch https://example.com --live                  Fetch the live page (skip cache)\n  keenable fetch https://example.com --prompt \"List all pricing tiers\"     Extract with an LLM\n  keenable fetch https://example.com --max-chars 200000      Raise the 50000-char content cap\n  keenable fetch https://example.com --session-id agent-run-42   Group with related calls in a session\n  keenable fetch https://example.com --api-key keen_***_*****     Use a specific API key\n  keenable fetch https://example.com -p -v                   Show proxy, trusted CAs and full errors\n\nExit codes:\n  0 success   1 other error   2 invalid arguments   3 authentication or permission\n  4 rate limited   5 out of credits   6 the page or site failed (fetch)   7 network\n  8 invalid request   9 server error (retry later)\nOn an error, YAML mode prints the structured error on stdout and a one-line summary on stderr."
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

        /// Session ID that groups related searches and fetches (e.g. one agent run)
        #[arg(long = "session-id", value_name = "ID")]
        session_id: Option<String>,

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

#[derive(clap::Args)]
struct SearchArgs {
    /// Search query
    query: String,

    /// Search mode: "pro" for deeper retrieval, or "realtime" for the fastest response.
    /// Without it, `keenable config` default_search_mode applies, then your organization's
    /// mode, then "pro". A forced_search_mode set with `keenable config` overrides this flag
    #[arg(long)]
    mode: Option<String>,

    /// Restrict results to a specific site (e.g. "docs.rs")
    #[arg(long)]
    site: Option<String>,

    /// Never return results from this host or its subdomains. Repeatable, or a comma list (a.com,b.com); at most 1,000 distinct hosts. Hosts only, no paths, suffixes like .gov, wildcards or IPs. Private-suffix hosts like github.io are accepted
    #[arg(long = "exclude-domain", value_name = "DOMAIN", value_delimiter = ',')]
    exclude_domains: Vec<String>,

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

    /// Session ID that groups related searches and fetches (e.g. one agent run)
    #[arg(long = "session-id", value_name = "ID")]
    session_id: Option<String>,

    /// Pretty-print output for humans instead of YAML
    #[arg(short = 'p', long = "pretty")]
    pretty: bool,

    /// API key (overrides stored key)
    #[arg(long = "api-key")]
    api_key: Option<String>,
}

impl SearchArgs {
    fn filters(&self) -> commands::search::SearchFilters {
        commands::search::SearchFilters {
            site: self.site.clone(),
            exclude_domains: commands::search::trim_domain_entries(self.exclude_domains.clone()),
            acquired_after: self.acquired_after.clone(),
            acquired_before: self.acquired_before.clone(),
            published_after: self.published_after.clone(),
            published_before: self.published_before.clone(),
            query_time: self.query_time.clone(),
        }
    }
}

fn collect_client_flags(
    all: bool,
    claude_code: bool,
    cursor: bool,
    devin: bool,
    windsurf: bool,
    codex: bool,
    opencode: bool,
) -> Vec<String> {
    let pairs: &[(bool, &str)] = &[
        (all, "all"),
        (claude_code, "claude-code"),
        (cursor, "cursor"),
        (devin, "devin"),
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

/// Fail fast on an invalid `KEENABLE_API_URL` before any command talks to
/// the API. Agent-facing output (search/fetch without `-p`) also gets the
/// error as YAML on stdout, like API errors.
fn check_api_base(command: &Commands) {
    let yaml = match command {
        Commands::Search(SearchArgs { pretty, .. }) | Commands::Fetch { pretty, .. } => !*pretty,
        Commands::Login { .. } | Commands::ConfigureMcp { .. } | Commands::Daemon => false,
        _ => return,
    };
    if let Err(msg) = api::resolved_api_base() {
        api::exit_invalid_api_base(msg, yaml);
    }
}

fn main() {
    // Before any thread exists (the tokio runtime is built below), so
    // removing env vars is sound. An empty SSL_CERT_FILE/SSL_CERT_DIR would
    // make rustls-native-certs skip the OS store for an empty path; drop it
    // so empty means unset everywhere, including in the spawned daemon.
    for name in net::empty_cert_vars(|n| std::env::var_os(n)) {
        // SAFETY: single-threaded at this point; no other code reads or
        // writes the environment concurrently.
        unsafe { std::env::remove_var(name) };
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the async runtime")
        .block_on(run());
}

async fn run() {
    let cli = Cli::parse();
    net::set_verbose(cli.verbose);
    check_api_base(&cli.command);

    // Update check only for human-facing output: awaiting it would add up to
    // ~5s (on cache miss) to agent-facing YAML commands and the daemon.
    // `update` is excluded because it performs its own explicit check.
    let wants_update_check = match &cli.command {
        Commands::Search(SearchArgs { pretty, .. }) | Commands::Fetch { pretty, .. } => *pretty,
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
            devin,
            windsurf,
            codex,
            opencode,
        } => {
            let flags =
                collect_client_flags(all, claude_code, cursor, devin, windsurf, codex, opencode);
            commands::configure_mcp::configure_mcp(flags, yes).await;
        }
        Commands::Reset {
            all,
            yes,
            claude_code,
            cursor,
            devin,
            windsurf,
            codex,
            opencode,
        } => {
            let flags =
                collect_client_flags(all, claude_code, cursor, devin, windsurf, codex, opencode);
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
        Commands::Search(args) => {
            commands::search::search(
                &args.query,
                args.mode.as_deref(),
                args.filters(),
                commands::search::SearchOptions {
                    snippet_max_length: args.snippet_max_length,
                    max_results: args.max_results,
                    session_id: args.session_id.clone(),
                },
                args.pretty,
                args.api_key.as_deref(),
            )
            .await;
        }
        Commands::Fetch {
            url,
            live,
            prompt,
            max_chars,
            session_id,
            pretty,
            api_key,
        } => {
            commands::search::fetch(
                &url,
                live,
                prompt,
                max_chars,
                session_id,
                pretty,
                api_key.as_deref(),
            )
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
            ui::one_line(&version)
        ));
        ui::hint(&format!("Run: {}", update::update_hint()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_filters(args: &[&str]) -> serde_json::Value {
        let argv = ["keenable", "search", "q"].iter().chain(args);
        let Commands::Search(search) = Cli::try_parse_from(argv).expect("args parse").command
        else {
            panic!("expected the search subcommand");
        };
        search.filters().to_json()
    }

    #[test]
    fn exclude_flag_absent_by_default() {
        let v = search_filters(&[]);
        assert!(v.get("exclude_domains").is_none());
    }

    #[test]
    fn exclude_flag_accepts_repeats_and_comma_lists() {
        let v = search_filters(&[
            "--exclude-domain",
            "press.un.org,undocs.org",
            "--exclude-domain",
            "who.int",
        ]);
        assert_eq!(
            v["exclude_domains"],
            json!(["press.un.org", "undocs.org", "who.int"])
        );
    }

    #[test]
    fn site_flag_is_kept() {
        let v = search_filters(&["--site", "docs.rs"]);
        assert_eq!(v["site"], json!("docs.rs"));
        assert!(v.get("exclude_domains").is_none());
    }

    #[test]
    fn site_and_exclude_domain_are_both_forwarded() {
        let v = search_filters(&["--site", "a.com", "--exclude-domain", "old.a.com"]);
        assert_eq!(v["site"], json!("a.com"));
        assert_eq!(v["exclude_domains"], json!(["old.a.com"]));
    }

    #[test]
    fn comma_lists_trim_spaces_and_keep_stray_commas() {
        let v = search_filters(&[
            "--exclude-domain",
            " a.com , b.com",
            "--exclude-domain",
            ",spam.com,,",
        ]);
        assert_eq!(
            v["exclude_domains"],
            json!(["a.com", "b.com", "", "spam.com", "", ""])
        );
    }

    #[test]
    fn domain_entries_are_sent_as_typed_for_the_server_to_normalize() {
        let v = search_filters(&[
            "--exclude-domain",
            " https://WWW.UN.org/ ,un.org/press,.gov",
        ]);
        assert_eq!(
            v["exclude_domains"],
            json!(["https://WWW.UN.org/", "un.org/press", ".gov"])
        );
    }

    #[test]
    fn blank_domain_flag_still_reaches_the_server() {
        for (blank, want) in [
            ("", json!([""])),
            (" ", json!([""])),
            (",", json!(["", ""])),
        ] {
            let v = search_filters(&[&format!("--exclude-domain={blank}")]);
            assert_eq!(v["exclude_domains"], want, "{blank:?}");
        }
    }

    #[test]
    fn blank_domain_flag_beside_a_real_one_is_sent() {
        let flag = "--exclude-domain";
        let v = search_filters(&["--exclude-domain=", flag, "a.com", flag, " "]);
        assert_eq!(v["exclude_domains"], json!(["", "a.com", ""]));
        let v = search_filters(&[flag, "a.com,"]);
        assert_eq!(v["exclude_domains"], json!(["a.com", ""]));
    }

    #[test]
    fn date_flags_reach_the_filters() {
        let v = search_filters(&[
            "--acquired-after",
            "7d",
            "--acquired-before",
            "2026-01-01",
            "--published-after",
            "2024-01-01",
            "--published-before",
            "2025-01-01",
            "--query-time",
            "2026-05-01T00:00:00Z",
        ]);
        assert_eq!(
            v,
            json!({
                "acquired_after": "7d",
                "acquired_before": "2026-01-01",
                "published_after": "2024-01-01",
                "published_before": "2025-01-01",
                "query_time": "2026-05-01T00:00:00Z",
            })
        );
    }
}
