//! Shared IDE definitions and config helpers used by `setup` and `reset`.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::constants::API_BASE_URL;

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

/// Claude Code built-in tools that overlap with Keenable.
pub const CLAUDE_CODE_STANDARD_TOOLS: &[&str] = &["WebSearch", "WebFetch"];

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
            name: "VS Code",
            flag: "vscode",
            config_path: home.join(".vscode/mcp.json"),
            servers_key: "servers",
            entry_style: McpEntryStyle::Http {
                url_key: "url",
                transport_type: Some("http"),
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
    ]
}

/// Check if an IDE is "detected" — parent directory exists.
pub fn is_detected(ide: &IDEDef) -> bool {
    ide.config_path
        .parent()
        .map_or(false, |p| p.exists())
}

/// Check if Keenable MCP is present in an IDE's config (any entry pointing at keenable.ai).
pub fn has_keenable_entry(ide: &IDEDef) -> bool {
    let config = read_config(&ide.config_path);
    config
        .pointer(&format!("/{}/keenable", ide.servers_key))
        .is_some()
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

/// Detailed status of a client's Keenable configuration.
pub struct IdeStatus {
    /// Is Keenable MCP entry present?
    pub has_entry: bool,
    /// Does the entry have a different API key?
    pub wrong_api_key: bool,
    /// Uses legacy npx mcp-remote instead of built-in stdio bridge?
    pub uses_legacy_npx: bool,
    /// Are standard tools disabled (only relevant for has_standard_tools)?
    pub standard_tools_disabled: bool,
    /// Duplicate Keenable entries under other names.
    pub duplicate_entries: Vec<String>,
    /// Conflicting search MCP servers.
    pub conflicting_mcps: Vec<String>,
}

pub fn get_ide_status(ide: &IDEDef, api_key: &str) -> IdeStatus {
    let config = read_config(&ide.config_path);
    let existing = config
        .pointer(&format!("/{}/keenable", ide.servers_key))
        .cloned();

    let has_entry = existing.is_some();

    let wrong_api_key = if let Some(ref entry) = existing {
        let existing_key = extract_entry_api_key(entry);
        let desired_key = Some(api_key.to_string());
        existing_key.is_some() && existing_key != desired_key
    } else {
        false
    };

    let uses_legacy_npx = existing
        .as_ref()
        .and_then(|e| e["command"].as_str())
        .map_or(false, |cmd| cmd == "npx");

    let standard_tools_disabled = if ide.has_standard_tools {
        let deny_list = config
            .pointer("/permissions/deny")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        CLAUDE_CODE_STANDARD_TOOLS
            .iter()
            .all(|tool| deny_list.iter().any(|d| d == *tool))
    } else {
        true
    };

    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut duplicate_entries = Vec::new();
    let mut conflicting_mcps = Vec::new();

    for (name, entry) in &servers {
        if name == "keenable" {
            continue;
        }
        if let Some(url) = extract_url(entry) {
            if is_keenable_url(&url) {
                duplicate_entries.push(name.clone());
                continue;
            }
        }
        if is_conflicting_name(name) {
            conflicting_mcps.push(name.clone());
        }
    }

    IdeStatus {
        has_entry,
        wrong_api_key,
        uses_legacy_npx,
        standard_tools_disabled,
        duplicate_entries,
        conflicting_mcps,
    }
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
