/// Production API. `KEENABLE_API_URL` overrides it for API calls (see
/// `api::api_base`); the MCP URL written into client configs always uses this.
pub const DEFAULT_API_BASE_URL: &str = "https://api.keenable.ai";
/// Env var that overrides the API base URL. Same name the SDKs use (see `api::resolve_api_base` for how the rule differs).
pub const API_URL_ENV: &str = "KEENABLE_API_URL";
pub const SEARCH_MODES: &[&str] = &["realtime", "pro"];
pub const CLIENT_ID: &str = "keenable-cli";
pub const UPDATE_CHECK_INTERVAL_SECONDS: u64 = 3600; // 1 hour
pub const GITHUB_REPO: &str = "keenableai/keenable-cli";
