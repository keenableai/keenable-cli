//! Shared IDE definitions and config helpers used by `configure-mcp` and `reset`.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::constants::{API_BASE_URL, WEBQL_BASE_URL};

// ── Known conflicting search MCP server names ───────────────────────

pub const CONFLICTING_NAMES: &[&str] = &[
    "brave-search",
    "tavily",
    "bing",
    "exa",
    "perplexity",
    "serper",
    "firecrawl",
    "browserbase",
];

/// URLs that indicate a Keenable-related MCP entry (prod or test).
pub const KEENABLE_URLS: &[&str] = &["api.keenable.ai", "api-test.keenable.ai"];

/// URLs that indicate a WebQL MCP entry (prod or test).
pub const WEBQL_URLS: &[&str] = &["webql.keenable.ai", "webql-test.keenable.ai"];

/// Claude Code built-in tools that overlap with Keenable.
pub const CLAUDE_CODE_STANDARD_TOOLS: &[&str] = &["WebSearch", "WebFetch"];

/// OpenCode built-in tools that overlap with Keenable.
pub const OPENCODE_STANDARD_TOOLS: &[&str] = &["websearch"];

// ── IDE definitions ─────────────────────────────────────────────────

/// How the MCP entry should be serialised in the IDE config file.
pub enum McpEntryStyle {
    /// Direct HTTP entry: `{ "url_key": "...", "headers": {...}, "type"?: "..." }`
    Http {
        url_key: &'static str,
        transport_type: Option<&'static str>,
    },
    /// Stdio bridge via `keenable mcp-stdio` (needed by Claude Desktop).
    Stdio,
    /// TOML-based config (Codex CLI): `[mcp_servers.name] url = "..." `
    Toml,
}

pub struct IDEDef {
    pub name: &'static str,
    /// CLI flag name (e.g. "claude-code", "cursor").
    pub flag: &'static str,
    pub config_path: PathBuf,
    /// Top-level key holding MCP servers ("mcpServers", "servers", "context_servers").
    pub servers_key: &'static str,
    /// How the MCP entry is represented in this IDE's config.
    pub entry_style: McpEntryStyle,
    /// Whether this IDE has standard tools that can be disabled via config.
    pub has_standard_tools: bool,
}

pub fn all_ides() -> Vec<IDEDef> {
    let home = dirs::home_dir().expect("cannot determine home directory");

    vec![
        IDEDef {
            name: "Claude Code",
            flag: "claude-code",
            config_path: home.join(".claude.json"),
            servers_key: "mcpServers",
            entry_style: McpEntryStyle::Http {
                url_key: "url",
                transport_type: Some("http"),
            },
            has_standard_tools: true,
        },
        IDEDef {
            name: "Claude Desktop",
            flag: "claude-desktop",
            config_path: home.join("Library/Application Support/Claude/claude_desktop_config.json"),
            servers_key: "mcpServers",
            entry_style: McpEntryStyle::Stdio,
            has_standard_tools: false,
        },
        IDEDef {
            name: "Cursor",
            flag: "cursor",
            config_path: home.join(".cursor/mcp.json"),
            servers_key: "mcpServers",
            entry_style: McpEntryStyle::Http {
                url_key: "url",
                transport_type: Some("streamable-http"),
            },
            has_standard_tools: false,
        },
        IDEDef {
            name: "Windsurf",
            flag: "windsurf",
            config_path: home.join(".codeium/windsurf/mcp_config.json"),
            servers_key: "mcpServers",
            entry_style: McpEntryStyle::Http {
                url_key: "serverUrl",
                transport_type: None,
            },
            has_standard_tools: false,
        },
        IDEDef {
            name: "Codex",
            flag: "codex",
            config_path: home.join(".codex/config.toml"),
            servers_key: "mcp_servers",
            entry_style: McpEntryStyle::Toml,
            has_standard_tools: false,
        },
        IDEDef {
            name: "OpenCode",
            flag: "opencode",
            config_path: home.join(".config/opencode/opencode.json"),
            servers_key: "mcp",
            entry_style: McpEntryStyle::Http {
                url_key: "url",
                transport_type: Some("remote"),
            },
            has_standard_tools: true,
        },
    ]
}

/// Check if an IDE is "detected" — parent directory exists.
pub fn is_detected(ide: &IDEDef) -> bool {
    ide.config_path
        .parent()
        .map_or(false, |p| p.exists())
}

// ── Config helpers ──────────────────────────────────────────────────

fn is_toml(path: &PathBuf) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("toml")
}

pub fn read_config(path: &PathBuf) -> Value {
    if path.exists() {
        let content = fs::read_to_string(path).unwrap_or_default();
        if is_toml(path) {
            let toml_val: toml::Value = toml::from_str(&content).unwrap_or(toml::Value::Table(Default::default()));
            serde_json::to_value(&toml_val).unwrap_or(json!({}))
        } else {
            serde_json::from_str(&content).unwrap_or(json!({}))
        }
    } else {
        json!({})
    }
}

pub fn write_config(path: &PathBuf, config: &Value) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
    }
    if is_toml(path) {
        let toml_val: toml::Value = serde_json::from_value(config.clone()).unwrap_or(toml::Value::Table(Default::default()));
        fs::write(path, toml::to_string_pretty(&toml_val).unwrap_or_default()).ok();
    } else {
        fs::write(path, serde_json::to_string_pretty(config).unwrap_or_default()).ok();
    }
}

pub fn build_keenable_entry(ide: &IDEDef, api_key: &str) -> Value {
    let mcp_url = format!("{}/mcp", API_BASE_URL);
    match &ide.entry_style {
        McpEntryStyle::Http {
            url_key,
            transport_type,
        } => {
            let mut entry = json!({
                *url_key: mcp_url,
                "headers": {
                    "X-API-Key": api_key
                }
            });
            if let Some(transport) = transport_type {
                entry["type"] = json!(*transport);
            }
            entry
        }
        McpEntryStyle::Stdio => {
            json!({
                "command": "keenable",
                "args": [
                    "mcp-stdio",
                    "--api-key",
                    api_key
                ]
            })
        }
        McpEntryStyle::Toml => {
            json!({
                "url": mcp_url,
                "http_headers": {
                    "X-API-Key": api_key
                }
            })
        }
    }
}

/// Extract URL from a server entry, checking all known URL keys and mcp-remote args.
pub fn extract_url(entry: &Value) -> Option<String> {
    if let Some(url) = entry["url"]
        .as_str()
        .or_else(|| entry["serverUrl"].as_str())
    {
        return Some(url.to_string());
    }
    if let Some(args) = entry["args"].as_array() {
        let cmd = entry["command"].as_str().unwrap_or("");
        let first_arg = args.first().and_then(|v| v.as_str()).unwrap_or("");
        // Legacy npx mcp-remote format
        if first_arg == "mcp-remote" {
            if let Some(url) = args.get(1).and_then(|v| v.as_str()) {
                return Some(url.to_string());
            }
        }
        // New keenable mcp-stdio format — infer URL from the command itself
        if cmd == "keenable" && first_arg == "mcp-stdio" {
            return Some(format!("{}/mcp", API_BASE_URL));
        }
    }
    None
}

/// Check if a URL points to Keenable (prod or test).
pub fn is_keenable_url(url: &str) -> bool {
    KEENABLE_URLS.iter().any(|k| url.contains(k))
}

/// Check if a server name is a known conflicting search server.
pub fn is_conflicting_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    CONFLICTING_NAMES.iter().any(|c| lower.contains(c))
}

/// Check if a URL points to WebQL (prod or test).
pub fn is_webql_url(url: &str) -> bool {
    WEBQL_URLS.iter().any(|k| url.contains(k))
}

/// Build the `keenable-webql` MCP entry for a given IDE.
/// Auth is via `?token=` query parameter in the URL (no headers needed).
pub fn build_webql_entry(ide: &IDEDef, api_key: &str) -> Value {
    let mcp_url = format!("{}/mcp?token={}", WEBQL_BASE_URL, api_key);
    match &ide.entry_style {
        McpEntryStyle::Http {
            url_key,
            transport_type,
        } => {
            let mut entry = json!({ *url_key: mcp_url });
            if let Some(transport) = transport_type {
                entry["type"] = json!(*transport);
            }
            entry
        }
        McpEntryStyle::Stdio => {
            json!({
                "command": "keenable",
                "args": [
                    "mcp-stdio",
                    "--url",
                    mcp_url
                ]
            })
        }
        McpEntryStyle::Toml => {
            json!({ "url": mcp_url })
        }
    }
}

/// Extract the API key (token) from a WebQL MCP entry's URL.
pub fn extract_webql_token(entry: &Value) -> Option<String> {
    let url = extract_url(entry)?;
    // Parse ?token=... from URL
    url.split("token=")
        .nth(1)
        .map(|t| t.split('&').next().unwrap_or(t).to_string())
}

/// Extract the API key from a Keenable MCP entry's headers or mcp-remote args.
pub fn extract_entry_api_key(entry: &Value) -> Option<String> {
    if let Some(key) = entry["headers"]["X-API-Key"]
        .as_str()
        .or_else(|| entry["http_headers"]["X-API-Key"].as_str())
    {
        return Some(key.to_string());
    }
    if let Some(args) = entry["args"].as_array() {
        for (i, arg) in args.iter().enumerate() {
            let s = arg.as_str().unwrap_or("");
            // New format: --api-key <KEY>
            if s == "--api-key" {
                if let Some(key) = args.get(i + 1).and_then(|v| v.as_str()) {
                    return Some(key.to_string());
                }
            }
            // Legacy format: --header X-API-Key:<KEY>
            if s == "--header" {
                if let Some(header_val) = args.get(i + 1).and_then(|v| v.as_str()) {
                    if let Some(key) = header_val.strip_prefix("X-API-Key:") {
                        return Some(key.to_string());
                    }
                }
            }
        }
    }
    None
}
