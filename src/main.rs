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
        after_help = "Agent-friendly device flow: shows a code for the user to approve.\nWorks on local machines, remote servers, and agent environments.\n\nWith --api-key, saves the key directly (useful for CI and servers).\n\nAfter login, run: keenable configure-mcp --all\n\nExamples:\n  keenable login                             Device flow (agent-friendly)\n  printenv KEENABLE_API_KEY | keenable login --api-key -\n                                             Save a key from stdin (stays out of\n                                             the process list and shell history)\n  keenable login --api-key keen_***_*****    Save API key directly\n                                             (an empty key is refused, exit 1)\n  keenable login -v                          Show proxy, trusted CAs and full errors\n\nBehind a corporate proxy: set HTTPS_PROXY, and SSL_CERT_FILE to your company's CA bundle\nif it inspects TLS."
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
        after_help = "Works without login (free tier). Log in for higher rate limits.\n\nExamples:\n  keenable search \"rust async\"                                    YAML output (for agents)\n  keenable search \"rust async\" -p                                 Pretty output (for humans)\n  keenable search \"AI news\" --mode realtime                       Fastest response (default mode: pro)\n  keenable search \"AI news\" --site techcrunch.com                 Restrict to site\n  keenable search \"AI news\" --exclude-domain spam.com,ads.net     Never these domains (repeatable)\n  keenable search \"tax forms\" --exclude-domain .gov               Never any host under .gov\n  keenable search \"dodgers braves\" --published-after 2026-01-01   Date filter (YYYY-MM-DD)\n  keenable search \"AI news\" --acquired-after 7d                   Relative date (min, h, d, w, mo, y)\n  keenable search \"AI news\" --acquired-after 2026-01-15T10:30:00Z ISO 8601 datetime\n  keenable search \"rust async\" --snippet-max-length 2000          Longer snippets (180-10000)\n  keenable search \"rust async\" --max-results 25                   More results (1-50, default 10)\n  keenable search \"AI news\" --query-time 2026-01-01T00:00:00Z     Point-in-time search\n  keenable search \"rust async\" --session-id agent-run-42           Group related calls in a session\n  keenable search \"rust async\" --objective \"Find the tokio docs page on spawn_blocking\"   Say what the search is for (used in ranking)\n  keenable search \"query\" --moderation strict                     Remove unsafe results (moderate or strict)\n  keenable search \"rust async\" --api-key keen_***_*****                Use a specific API key\n  keenable search \"rust async\" -p -v                              Show proxy, trusted CAs and full errors\n\nExit codes:\n  0 success   1 other error   2 invalid arguments   3 authentication or permission\n  4 rate limited   5 out of credits   6 the page or site failed (fetch)   7 network\n  8 invalid request   9 server error (retry later)\nOn an error, YAML mode prints the structured error on stdout and a one-line summary on stderr."
    )]
    Search(Box<SearchArgs>),

    /// Fetch pages as markdown (outputs YAML by default, use -p for pretty output)
    #[command(
        after_help = "Without a key, fetch takes one URL with --max-chars, --prompt and --max-age-hours (free tier).\nSeveral URLs, --schema, --links and --images need an API key (keenable login).\nWith a key, each URL is billed as one fetch; the call counts as one request toward the rate limit.\n\nExamples:\n  keenable fetch https://example.com                         YAML output\n  keenable fetch https://example.com -p                      Pretty output\n  keenable fetch https://a.com https://b.com                 Several pages in one call (up to 20)\n  keenable fetch https://example.com --max-age-hours 0       Read the live page\n  keenable fetch https://example.com --max-age-hours 24      Re-read the page if the stored copy is older\n  keenable fetch https://example.com --max-age-hours -1      Stored copy only, never the live page\n  keenable fetch https://example.com --prompt \"List all pricing tiers\"     Extract with an LLM\n  keenable fetch https://example.com --schema @plans.json    Typed answer in `structured` (JSON or @file)\n  keenable fetch https://example.com --links 50 --images 5   Links and images found on the page\n  keenable fetch https://example.com --max-chars 200000      Raise the 50000-char content cap\n  keenable fetch https://example.com --session-id agent-run-42   Group with related calls in a session\n  keenable fetch https://example.com --api-key keen_***_*****     Use a specific API key (an empty value is an error)\n  keenable fetch https://example.com -p -v                   Show proxy, trusted CAs and full errors\n\nWith several URLs, YAML prints `results`: one page or `{url, error}` per URL, in order. The command\nfails only when the call fails or every URL failed (exit code of the most common error).\n\nExit codes:\n  0 success   1 other error   2 invalid arguments   3 authentication or permission\n  4 rate limited   5 out of credits   6 the page or site failed (fetch)   7 network\n  8 invalid request   9 server error (retry later)\nOn an error, YAML mode prints the structured error on stdout and a one-line summary on stderr."
    )]
    Fetch {
        /// URLs to fetch (1 to 20, each once; more than one needs an API key)
        #[arg(value_name = "URL", required = true, num_args = 1..=commands::search::MAX_FETCH_URLS)]
        urls: Vec<String>,

        /// Truncate content at this many characters (default: 50000)
        #[arg(long = "max-chars", value_parser = clap::value_parser!(u64).range(1..))]
        max_chars: Option<u64>,

        /// Extraction instruction: an LLM reads the page and returns only
        /// this instruction's output instead of the full page (max 2000 chars)
        #[arg(long)]
        prompt: Option<String>,

        /// Oldest stored copy to accept, in hours (0 to 720, fractions allowed); 0 reads the live page,
        /// -1 reads only the stored copy, of any age, never the live page
        #[arg(
            long = "max-age-hours",
            value_name = "HOURS",
            allow_negative_numbers = true,
            value_parser = commands::search::parse_max_age_hours
        )]
        max_age_hours: Option<f64>,

        /// Deprecated alias of `--max-age-hours 0`, kept for scripts written for 0.6.0
        #[arg(long, hide = true)]
        live: bool,

        /// JSON Schema (root type object, inline or @file) for a typed answer in `structured`. Needs an API key
        #[arg(long, value_name = "JSON|@FILE", value_parser = commands::search::parse_schema)]
        schema: Option<serde_json::Value>,

        /// Return up to N links found on the page (1 to 1000). Needs an API key
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..=1000))]
        links: Option<u64>,

        /// Return up to N images from the page (0 to 25). Needs an API key
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(0..=25))]
        images: Option<u64>,

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

    /// Watch the web for news about a topic (outputs YAML by default, use -p for pretty output)
    #[command(
        after_help = "A monitor is a standing query: every newly crawled page that is news about its topic is kept as a\nresult of that monitor, and sent to your webhook or returned in scheduled runs. Needs an API key\n(run `keenable login`). Monitors are free for now; an organization holds up to 100 active monitors.\n\nExamples:\n  keenable monitors create \"new open-weights LLM releases\" --every 1d     Daily runs\n  keenable monitors create \"EU AI Act enforcement\" --webhook https://example.com/hook\n                                                                          Each result to a webhook as it is found\n  keenable monitors list -p                                               Your organization's monitors\n  keenable monitors results mon_123 --min-importance 4 -p                 The important results it found\n  keenable monitors pause mon_123                                         Stop it for a while\n  keenable monitors delete mon_123 --yes                                  Delete it and its results\n\nDocs: https://docs.keenable.ai/products/monitors\n\nExit codes:\n  0 success   1 other error   2 invalid arguments   3 authentication or permission\n  4 rate limited   7 network   8 invalid request   9 server error (retry later)\nOn an error, YAML mode prints the structured error on stdout and a one-line summary on stderr."
    )]
    Monitors(Box<MonitorsArgs>),

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
struct MonitorsArgs {
    #[command(subcommand)]
    action: MonitorsAction,

    /// Pretty-print output for humans instead of YAML
    #[arg(short = 'p', long = "pretty", global = true)]
    pretty: bool,

    /// API key (overrides stored key)
    #[arg(long = "api-key", global = true)]
    api_key: Option<String>,
}

#[derive(clap::Args)]
struct ScheduleFlags {
    /// Run at this interval, from 1h to 90d (e.g. 6h, 1d, 7d)
    #[arg(long, value_name = "INTERVAL", conflicts_with_all = ["cron", "once"])]
    every: Option<String>,

    /// Run on this five-field cron expression, at most hourly (e.g. "0 9 * * 1-5")
    #[arg(long, value_name = "EXPR", conflicts_with = "once")]
    cron: Option<String>,

    /// IANA time zone for --cron (default UTC)
    #[arg(long, value_name = "TZ", requires = "cron")]
    timezone: Option<String>,

    /// Run once: "now", a delay such as 2h, or a timestamp such as 2026-12-01T12:00:00Z
    #[arg(long, value_name = "WHEN")]
    once: Option<String>,
}

#[derive(clap::Args)]
struct MonitorFlags {
    /// A name for the monitor
    #[arg(long)]
    name: Option<String>,

    /// Results a run returns at most (1-100, default 10)
    #[arg(long = "max-results", value_parser = clap::value_parser!(u64).range(1..=100))]
    max_results: Option<u64>,

    /// Only results on these sites. Repeatable, or a comma list
    #[arg(long = "include-domain", value_name = "DOMAIN", value_delimiter = ',')]
    include_domains: Vec<String>,

    /// Never results on these sites. Repeatable, or a comma list
    #[arg(long = "exclude-domain", value_name = "DOMAIN", value_delimiter = ',')]
    exclude_domains: Vec<String>,

    /// Also return each result's page text (first 6,000 characters, kept 3 days)
    #[arg(long)]
    text: bool,

    #[command(flatten)]
    schedule: ScheduleFlags,

    /// Send events to this public HTTPS address
    #[arg(long, value_name = "URL")]
    webhook: Option<String>,

    /// Webhook events to send (default: the monitor.* and monitor.run.* events, monitor.result.found and monitor.result.changed, and monitor.page.changed once a page is attached; listing events sends only those). Repeatable, or a comma list
    #[arg(long = "event", value_name = "EVENT", value_delimiter = ',')]
    events: Vec<String>,

    /// Your own tag, echoed in every webhook (key=value, repeatable)
    #[arg(long, value_name = "KEY=VALUE", value_parser = commands::monitors::parse_metadata)]
    metadata: Vec<(String, String)>,
}

impl MonitorFlags {
    fn fields(
        self,
        query: Option<String>,
        remove_schedule: bool,
        remove_webhook: bool,
    ) -> commands::monitors::MonitorFields {
        let s = self.schedule;
        commands::monitors::MonitorFields {
            query,
            name: self.name,
            max_results: self.max_results,
            include_domains: self.include_domains,
            exclude_domains: self.exclude_domains,
            text: self.text,
            schedule: commands::monitors::ScheduleArg::from_flags(
                s.every,
                s.cron,
                s.timezone,
                s.once,
                remove_schedule,
            ),
            webhook: self.webhook,
            events: self.events,
            remove_webhook,
            metadata: self.metadata,
            status: None,
        }
    }
}

#[derive(Subcommand)]
enum MonitorsAction {
    /// Create a monitor
    #[command(
        after_help = "The monitor starts empty: it matches only pages crawled after it is created.\nWithout a schedule, runs happen only with `keenable monitors trigger`.\nThe answer carries webhook.secret (whsec_...) once: store it to check signatures.\n\nExamples:\n  keenable monitors create \"new open-weights LLM releases\" --every 1d\n  keenable monitors create \"Series A rounds in robotics\" --cron \"0 9 * * 1-5\" --timezone Europe/Paris\n  keenable monitors create \"Rust releases\" --include-domain blog.rust-lang.org --max-results 5\n  keenable monitors create \"CVE in OpenSSL\" --webhook https://example.com/hook      Each result as it is found\n  keenable monitors create \"q\" --idempotency-key run-42      Safe to retry: the same key returns the same monitor"
    )]
    Create {
        /// The topic in plain words, e.g. "new open-weights LLM releases"
        query: String,

        #[command(flatten)]
        flags: MonitorFlags,

        /// Make a retry safe: within 24 hours the same key and body return the monitor the first call created
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },

    /// List your organization's monitors, newest first
    #[command(
        after_help = "Examples:\n  keenable monitors list\n  keenable monitors list --status active -p\n  keenable monitors list --name robotics --limit 5"
    )]
    List {
        /// Only monitors with this status
        #[arg(long, value_parser = ["active", "paused", "disabled", "completed"])]
        status: Option<String>,

        /// Only monitors whose name contains this
        #[arg(long)]
        name: Option<String>,

        /// How many to return (1-100, default 50)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,

        /// next_cursor of the previous page
        #[arg(long)]
        cursor: Option<String>,
    },

    /// Show one monitor
    #[command(after_help = "Example:\n  keenable monitors get mon_123 -p")]
    Get {
        /// The monitor's id (mon_...)
        id: String,
    },

    /// Change a monitor's name, query, schedule, sites or webhook
    #[command(
        after_help = "Only the flags you pass change; the rest stays.\n\nExamples:\n  keenable monitors update mon_123 --query \"open-weights LLM releases and benchmarks\"\n  keenable monitors update mon_123 --every 6h\n  keenable monitors update mon_123 --no-schedule            Run only when triggered\n  keenable monitors update mon_123 --no-webhook             Stop sending events"
    )]
    Update {
        /// The monitor's id (mon_...)
        id: String,

        /// A new topic
        #[arg(long)]
        query: Option<String>,

        #[command(flatten)]
        flags: MonitorFlags,

        /// Remove the schedule: runs happen only with `keenable monitors trigger`
        #[arg(long = "no-schedule", conflicts_with_all = ["every", "cron", "once"])]
        no_schedule: bool,

        /// Remove the webhook
        #[arg(long = "no-webhook", conflicts_with_all = ["webhook", "events"])]
        no_webhook: bool,
    },

    /// Pause a monitor: it stops finding results until resumed
    #[command(after_help = "Example:\n  keenable monitors pause mon_123")]
    Pause {
        /// The monitor's id (mon_...)
        id: String,
    },

    /// Resume a paused monitor
    #[command(after_help = "Example:\n  keenable monitors resume mon_123")]
    Resume {
        /// The monitor's id (mon_...)
        id: String,
    },

    /// Delete a monitor and every result it found (cannot be undone)
    #[command(
        after_help = "Asks first; without a terminal it needs --yes. To stop a monitor for a while, pause it instead.\n\nExamples:\n  keenable monitors delete mon_123\n  keenable monitors delete mon_123 --yes"
    )]
    Delete {
        /// The monitor's id (mon_...)
        id: String,

        /// Delete without asking
        #[arg(short, long)]
        yes: bool,
    },

    /// Start a run now, outside the schedule
    #[command(
        after_help = "The run collects the results found since the previous run. Check it with `keenable monitors run <id> <run_id>`.\n\nExample:\n  keenable monitors trigger mon_123"
    )]
    Trigger {
        /// The monitor's id (mon_...)
        id: String,
    },

    /// List a monitor's runs, newest first
    #[command(
        after_help = "Examples:\n  keenable monitors runs mon_123 -p\n  keenable monitors runs mon_123 --limit 1"
    )]
    Runs {
        /// The monitor's id (mon_...)
        id: String,

        /// How many to return (1-100, default 50)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,

        /// next_cursor of the previous page
        #[arg(long)]
        cursor: Option<String>,
    },

    /// Show one run and its results
    #[command(after_help = "Example:\n  keenable monitors run mon_123 run_456 -p")]
    Run {
        /// The monitor's id (mon_...)
        id: String,

        /// The run's id
        run_id: String,
    },

    /// List the results a monitor found, newest first (its last 500)
    #[command(
        after_help = "One result per story: same_story counts the copies left out.\n\nExamples:\n  keenable monitors results mon_123 -p\n  keenable monitors results mon_123 --min-importance 4    Only the results not to be missed"
    )]
    Results {
        /// The monitor's id (mon_...)
        id: String,

        /// How many to return (1-100, default 50)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,

        /// next_cursor of the previous page
        #[arg(long)]
        cursor: Option<String>,

        /// Only results rated at least this important to the topic (1-5)
        #[arg(long = "min-importance", value_parser = clap::value_parser!(u8).range(1..=5))]
        min_importance: Option<u8>,
    },

    /// List a monitor's webhook deliveries of the last 7 days
    #[command(
        after_help = "The place to look when a webhook seems quiet.\n\nExample:\n  keenable monitors deliveries mon_123 -p"
    )]
    Deliveries {
        /// The monitor's id (mon_...)
        id: String,

        /// How many to return (1-100, default 50)
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,

        /// next_cursor of the previous page
        #[arg(long)]
        cursor: Option<String>,
    },

    /// Count your organization's monitors by status and schedule
    #[command(after_help = "Example:\n  keenable monitors stats -p")]
    Stats,
}

#[derive(clap::Args)]
struct SearchArgs {
    /// Search query
    query: String,

    /// Search mode: "pro" for deeper retrieval, or "realtime" for the fastest response.
    /// Without it, `keenable config` default_search_mode applies, then your organization's
    /// mode, then "pro". A forced_search_mode set with `keenable config` overrides this flag
    #[arg(long, value_parser = commands::search::mode_value_parser())]
    mode: Option<String>,

    /// Restrict results to a specific site (e.g. "docs.rs")
    #[arg(long)]
    site: Option<String>,

    /// Never return results from this host or its subdomains. Repeatable, or a comma list (a.com,b.com); at most 1,000 distinct entries. A public suffix with a leading dot (.gov, .co.uk) excludes every host under it; a dot or wildcard on any other host (.un.org, *.un.org) and a bare suffix (com) are rejected, as are paths and IPs. Private-suffix hosts like github.io are accepted
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

    /// What this search is for: what the query leaves out and what earlier searches found, in one to three sentences (at most 1,000 characters). Keenable uses it together with the query to rank results; use it for multi-step or ambiguous searches
    #[arg(long, value_name = "TEXT")]
    objective: Option<String>,

    /// Remove results a safety model judges unsafe: "moderate" or "strict" (stricter). Results are removed, never replaced, so a search can return fewer or none. Without it, or with an empty value, no moderation. Only in "pro" and "realtime" modes, and not for a URL query fetched as-is (the API rejects those, exit 8)
    #[arg(long, value_name = "LEVEL", value_parser = commands::search::parse_moderation_flag)]
    moderation: Option<commands::search::ModerationFlag>,

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

async fn run_monitors(action: MonitorsAction, human: bool, api_key: Option<&str>) {
    use commands::monitors::{self as m, View};
    let (call, view) = match action {
        MonitorsAction::Create {
            query,
            flags,
            idempotency_key,
        } => {
            m::create(
                flags.fields(Some(query), false, false),
                idempotency_key,
                human,
                api_key,
            )
            .await;
            return;
        }
        MonitorsAction::Delete { id, yes } => {
            m::delete(&id, yes, human, api_key).await;
            return;
        }
        MonitorsAction::List {
            status,
            name,
            limit,
            cursor,
        } => (m::list_call(status, name, limit, cursor), View::Monitors),
        MonitorsAction::Get { id } => (m::get_call(&id), View::Monitor),
        MonitorsAction::Update {
            id,
            query,
            flags,
            no_schedule,
            no_webhook,
        } => {
            let fields = flags.fields(query, no_schedule, no_webhook);
            if m::monitor_body(&fields, false)
                .as_object()
                .is_some_and(|b| b.is_empty())
            {
                use clap::CommandFactory;
                let mut cli = Cli::command();
                cli.build();
                let update = cli
                    .find_subcommand_mut("monitors")
                    .and_then(|c| c.find_subcommand_mut("update"))
                    .expect("monitors update is a subcommand");
                update
                    .error(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "nothing to change: pass at least one flag (see `keenable monitors update --help`)",
                    )
                    .exit();
            }
            (m::update_call(&id, &fields), View::Monitor)
        }
        MonitorsAction::Pause { id } => (m::status_call(&id, "paused"), View::Monitor),
        MonitorsAction::Resume { id } => (m::status_call(&id, "active"), View::Monitor),
        MonitorsAction::Trigger { id } => (m::trigger_call(&id), View::Run),
        MonitorsAction::Runs { id, limit, cursor } => {
            (m::runs_call(&id, limit, cursor), View::Runs)
        }
        MonitorsAction::Run { id, run_id } => (m::run_call(&id, &run_id), View::Run),
        MonitorsAction::Results {
            id,
            limit,
            cursor,
            min_importance,
        } => (
            m::results_call(&id, limit, cursor, min_importance),
            View::Results,
        ),
        MonitorsAction::Deliveries { id, limit, cursor } => {
            (m::deliveries_call(&id, limit, cursor), View::Deliveries)
        }
        MonitorsAction::Stats => (m::stats_call(), View::Plain),
    };
    m::run(call, view, human, api_key).await;
}

/// Fail fast on an invalid `KEENABLE_API_URL` before any command talks to
/// the API. Agent-facing output (search/fetch without `-p`) also gets the
/// error as YAML on stdout, like API errors.
fn check_api_base(command: &Commands) {
    let yaml = match command {
        Commands::Search(args) => !args.pretty,
        Commands::Fetch { pretty, .. } => !*pretty,
        Commands::Monitors(args) => !args.pretty,
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
        Commands::Search(args) => args.pretty,
        Commands::Fetch { pretty, .. } => *pretty,
        Commands::Monitors(args) => args.pretty,
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
                    objective: args.objective.clone(),
                    moderation: args.moderation.and_then(|flag| flag.0),
                },
                args.pretty,
                args.api_key.as_deref(),
            )
            .await;
        }
        Commands::Fetch {
            urls,
            max_chars,
            prompt,
            max_age_hours,
            live,
            schema,
            links,
            images,
            session_id,
            pretty,
            api_key,
        } => {
            let max_age_hours = match live_max_age(live, max_age_hours) {
                Ok((hours, warning)) => {
                    if let Some(warning) = warning {
                        ui::warning_line(warning);
                    }
                    hours
                }
                Err(msg) => {
                    use clap::CommandFactory;
                    let mut cli = Cli::command();
                    cli.build();
                    cli.find_subcommand_mut("fetch")
                        .expect("fetch is a subcommand")
                        .error(clap::error::ErrorKind::ArgumentConflict, msg)
                        .exit();
                }
            };
            commands::search::fetch(
                &urls,
                commands::search::FetchOptions {
                    max_chars,
                    prompt,
                    max_age_hours,
                    schema,
                    links,
                    images,
                    session_id,
                },
                pretty,
                api_key.as_deref(),
            )
            .await;
        }
        Commands::Monitors(args) => {
            let MonitorsArgs {
                action,
                pretty,
                api_key,
            } = *args;
            run_monitors(action, pretty, api_key.as_deref()).await;
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

const LIVE_DEPRECATED: &str = "--live is deprecated; use --max-age-hours 0";

/// Maps the deprecated `--live` onto `--max-age-hours 0`, with the warning to print.
fn live_max_age(
    live: bool,
    max_age_hours: Option<f64>,
) -> Result<(Option<f64>, Option<&'static str>), &'static str> {
    match (live, max_age_hours) {
        (false, hours) => Ok((hours, None)),
        (true, None) => Ok((Some(0.0), Some(LIVE_DEPRECATED))),
        (true, Some(_)) => Err(
            "--live and --max-age-hours were both given; --live is deprecated, send only --max-age-hours (0 reads the live page)",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fetch_live(args: &[&str]) -> (bool, Option<f64>) {
        let argv = ["keenable", "fetch", "https://example.com"]
            .iter()
            .chain(args);
        match Cli::try_parse_from(argv).expect("args parse").command {
            Commands::Fetch {
                live,
                max_age_hours,
                ..
            } => (live, max_age_hours),
            _ => panic!("expected the fetch subcommand"),
        }
    }

    #[test]
    fn live_flag_maps_to_max_age_hours_0_and_warns() {
        let (live, hours) = fetch_live(&["--live"]);
        assert!(live);
        assert_eq!(
            live_max_age(live, hours),
            Ok((
                Some(0.0),
                Some("--live is deprecated; use --max-age-hours 0")
            ))
        );
        let (live, hours) = fetch_live(&["--max-age-hours", "24"]);
        assert_eq!(live_max_age(live, hours), Ok((Some(24.0), None)));
        assert_eq!(live_max_age(false, None), Ok((None, None)));
    }

    #[test]
    fn live_flag_with_max_age_hours_is_an_error() {
        let (live, hours) = fetch_live(&["--live", "--max-age-hours", "0"]);
        let msg = live_max_age(live, hours).expect_err("both flags are rejected");
        assert!(msg.contains("send only --max-age-hours"), "{msg}");
    }

    #[test]
    fn live_flag_is_hidden_from_help() {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        let help = cli
            .find_subcommand_mut("fetch")
            .expect("fetch is a subcommand")
            .render_long_help()
            .to_string();
        assert!(!help.contains("--live"), "{help}");
    }

    fn search_mode(mode: &str) -> Result<Option<String>, clap::Error> {
        match Cli::try_parse_from(["keenable", "search", "q", "--mode", mode])?.command {
            Commands::Search(search) => Ok(search.mode),
            _ => panic!("expected the search subcommand"),
        }
    }

    #[test]
    fn mode_flag_takes_the_search_modes_and_the_legacy_alias() {
        for mode in ["pro", "realtime", "standard"] {
            assert_eq!(search_mode(mode).unwrap().as_deref(), Some(mode));
        }
    }

    #[test]
    fn mode_flag_rejects_other_values_as_a_usage_error() {
        for bad in ["turbo", "PRO", "standard2"] {
            let err = search_mode(bad).expect_err("invalid mode is rejected");
            assert_eq!(err.exit_code(), 2, "{bad:?}");
            let msg = err.to_string();
            assert!(
                msg.contains("[possible values: realtime, pro]"),
                "{bad:?}: {msg}"
            );
        }
    }

    fn fetch_max_age(args: &[&str]) -> Result<Option<f64>, clap::Error> {
        let argv = ["keenable", "fetch", "https://example.com"]
            .iter()
            .chain(args);
        match Cli::try_parse_from(argv)?.command {
            Commands::Fetch { max_age_hours, .. } => Ok(max_age_hours),
            _ => panic!("expected the fetch subcommand"),
        }
    }

    #[test]
    fn fetch_max_age_hours_takes_minus_1_in_both_spellings() {
        assert_eq!(
            fetch_max_age(&["--max-age-hours", "-1"]).unwrap(),
            Some(-1.0)
        );
        assert_eq!(fetch_max_age(&["--max-age-hours=-1"]).unwrap(), Some(-1.0));
        assert_eq!(
            fetch_max_age(&["--max-age-hours", "-1", "-p"]).unwrap(),
            Some(-1.0)
        );
    }

    fn search_filters(args: &[&str]) -> serde_json::Value {
        let argv = ["keenable", "search", "q"].iter().chain(args);
        let Commands::Search(search) = Cli::try_parse_from(argv).expect("args parse").command
        else {
            panic!("expected the search subcommand");
        };
        search.filters().to_json()
    }

    fn search_options(args: &[&str]) -> serde_json::Value {
        let argv = ["keenable", "search", "q"].iter().chain(args);
        let Commands::Search(search) = Cli::try_parse_from(argv).expect("args parse").command
        else {
            panic!("expected the search subcommand");
        };
        commands::search::SearchOptions {
            moderation: search.moderation.and_then(|flag| flag.0),
            ..Default::default()
        }
        .to_json()
    }

    #[test]
    fn moderation_flag_takes_moderate_or_strict_and_is_absent_by_default() {
        assert_eq!(search_options(&[]), json!({}));
        assert_eq!(
            search_options(&["--moderation", "moderate"]),
            json!({"moderation": "moderate"})
        );
        assert_eq!(
            search_options(&["--moderation", "strict"]),
            json!({"moderation": "strict"})
        );
    }

    #[test]
    fn moderation_flag_counts_an_empty_value_as_not_given() {
        for empty in ["", "  "] {
            assert_eq!(
                search_options(&["--moderation", empty]),
                json!({}),
                "{empty:?}"
            );
        }
        assert_eq!(
            search_options(&["--moderation", " Strict "]),
            json!({"moderation": "strict"})
        );
    }

    #[test]
    fn moderation_flag_rejects_other_values_as_a_usage_error() {
        for bad in ["off", "STRICTER", "none"] {
            let err = Cli::try_parse_from(["keenable", "search", "q", "--moderation", bad])
                .err()
                .expect("invalid value is rejected");
            assert_eq!(err.exit_code(), 2, "{bad:?}");
            assert!(
                err.to_string().contains("moderate, strict"),
                "{bad:?}: {err}"
            );
        }
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

    fn monitors_action(args: &[&str]) -> MonitorsAction {
        let argv = ["keenable", "monitors"].iter().chain(args);
        let Commands::Monitors(parsed) = Cli::try_parse_from(argv).expect("args parse").command
        else {
            panic!("expected the monitors subcommand");
        };
        parsed.action
    }

    #[test]
    fn monitors_create_flags_become_the_v1_body() {
        let MonitorsAction::Create {
            query,
            flags,
            idempotency_key,
        } = monitors_action(&[
            "create",
            "open-weights LLM releases",
            "--every",
            "1d",
            "--max-results",
            "5",
            "--exclude-domain",
            "a.com,b.com",
            "--webhook",
            "https://example.com/h",
            "--event",
            "monitor.result.found",
            "--idempotency-key",
            "k1",
        ])
        else {
            panic!("expected create");
        };
        assert_eq!(idempotency_key.as_deref(), Some("k1"));
        let body = commands::monitors::monitor_body(&flags.fields(Some(query), false, false), true);
        assert_eq!(
            body,
            json!({
                "search": {"query": "open-weights LLM releases", "max_results": 5, "exclude_domains": ["a.com", "b.com"]},
                "schedule": {"type": "interval", "value": "1d"},
                "webhook": {"url": "https://example.com/h", "events": ["monitor.result.found"]}
            })
        );
    }

    #[test]
    fn monitors_schedule_flags_exclude_each_other() {
        let argv = [
            "keenable",
            "monitors",
            "create",
            "q",
            "--every",
            "1d",
            "--cron",
            "0 9 * * *",
        ];
        assert!(Cli::try_parse_from(argv).is_err());
        let argv = [
            "keenable",
            "monitors",
            "create",
            "q",
            "--timezone",
            "Europe/Paris",
        ];
        assert!(
            Cli::try_parse_from(argv).is_err(),
            "--timezone needs --cron"
        );
        let argv = [
            "keenable",
            "monitors",
            "update",
            "mon_1",
            "--no-schedule",
            "--every",
            "1d",
        ];
        assert!(Cli::try_parse_from(argv).is_err());
    }

    #[test]
    fn monitors_global_flags_go_after_the_subcommand() {
        let argv = ["keenable", "monitors", "list", "-p", "--api-key", "keen_x"];
        let Commands::Monitors(args) = Cli::try_parse_from(argv).unwrap().command else {
            panic!("expected monitors");
        };
        assert!(args.pretty);
        assert_eq!(args.api_key.as_deref(), Some("keen_x"));
    }

    #[test]
    fn monitors_limits_are_checked_locally() {
        assert!(
            Cli::try_parse_from([
                "keenable",
                "monitors",
                "results",
                "m",
                "--min-importance",
                "6"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["keenable", "monitors", "list", "--limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["keenable", "monitors", "list", "--status", "gone"]).is_err());
        assert!(
            Cli::try_parse_from([
                "keenable",
                "monitors",
                "create",
                "q",
                "--metadata",
                "novalue"
            ])
            .is_err()
        );
    }
}
