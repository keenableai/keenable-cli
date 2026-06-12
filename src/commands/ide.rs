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
    /// Path whose existence indicates the IDE is installed. When `None`,
    /// detection falls back to "parent dir of config_path exists".
    pub detect_path: Option<PathBuf>,
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
            // config_path's parent is $HOME, which always exists — without
            // this, Claude Code is "detected" on every machine.
            detect_path: Some(home.join(".claude")),
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
            detect_path: None,
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
            detect_path: None,
        },
        IDEDef {
            name: "Codex",
            flag: "codex",
            config_path: home.join(".codex/config.toml"),
            servers_key: "mcp_servers",
            entry_style: McpEntryStyle::Toml,
            has_standard_tools: false,
            detect_path: None,
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
            detect_path: None,
        },
    ]
}

/// Check if an IDE is "detected" — its detect_path (or the config file's
/// parent directory) exists.
pub fn is_detected(ide: &IDEDef) -> bool {
    if let Some(p) = &ide.detect_path {
        return p.exists() || ide.config_path.exists();
    }
    ide.config_path
        .parent()
        .map_or(false, |p| p.exists())
}

// ── Config helpers ──────────────────────────────────────────────────

fn is_toml(path: &PathBuf) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("toml")
}

/// Read an IDE config file. Missing file reads as an empty object; a file
/// that exists but can't be read or parsed is an `Err` — callers that write
/// the config back MUST NOT proceed on `Err`, or they would replace the
/// user's entire file with just the keenable entry.
pub fn read_config(path: &PathBuf) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("cannot read: {}", e))?;
    let value: Value = if is_toml(path) {
        let toml_val: toml::Value =
            toml::from_str(&content).map_err(|e| format!("invalid TOML: {}", e))?;
        serde_json::to_value(&toml_val).map_err(|e| format!("invalid TOML: {}", e))?
    } else {
        serde_json::from_str(&content).map_err(|e| format!("invalid JSON: {}", e))?
    };
    if !value.is_object() {
        return Err("root is not an object".to_string());
    }
    Ok(value)
}

/// Lenient read for status display only — invalid configs read as empty.
pub fn read_config_lenient(path: &PathBuf) -> Value {
    read_config(path).unwrap_or_else(|_| json!({}))
}

pub fn write_config(path: &PathBuf, config: &Value) -> Result<(), std::io::Error> {
    let content = if is_toml(path) {
        toml_patch(path, config)?
    } else {
        serde_json::to_string_pretty(config).map_err(std::io::Error::other)?
    };
    // Atomic: a crash mid-write must not leave a truncated config
    // (e.g. ~/.claude.json holds all of Claude Code's state).
    crate::config::atomic_write(path, &content, false)
}

/// Render `desired` as TOML by minimally patching the existing file.
/// A full JSON→TOML re-serialization would destroy comments and formatting
/// and turn datetime literals into strings (hand-maintained
/// ~/.codex/config.toml); instead, subtrees whose JSON form is unchanged
/// keep their original text and only changed/removed keys are rewritten.
fn toml_patch(path: &PathBuf, desired: &Value) -> Result<String, std::io::Error> {
    let original_text = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = original_text
        .parse()
        .map_err(|e| std::io::Error::other(format!("invalid TOML: {}", e)))?;
    // JSON view of the original for change detection — must be the same
    // conversion `read_config` used to produce `desired`.
    let original_json: Value = toml::from_str::<toml::Table>(&original_text)
        .ok()
        .and_then(|t| serde_json::to_value(&t).ok())
        .unwrap_or_else(|| json!({}));

    let empty = serde_json::Map::new();
    sync_toml_table(
        doc.as_table_mut(),
        original_json.as_object(),
        desired.as_object().unwrap_or(&empty),
    );
    Ok(doc.to_string())
}

fn sync_toml_table(
    table: &mut toml_edit::Table,
    original: Option<&serde_json::Map<String, Value>>,
    desired: &serde_json::Map<String, Value>,
) {
    let existing: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    for k in existing {
        if !desired.contains_key(&k) {
            table.remove(&k);
        }
    }
    for (k, dv) in desired {
        let ov = original.and_then(|o| o.get(k));
        if ov == Some(dv) {
            continue; // unchanged — keep original formatting verbatim
        }
        // Descend into existing tables so sibling entries keep their text.
        let descend = matches!(table.get(k), Some(toml_edit::Item::Table(_))) && dv.is_object();
        if descend {
            if let Some(toml_edit::Item::Table(t)) = table.get_mut(k) {
                sync_toml_table(t, ov.and_then(|v| v.as_object()), dv.as_object().unwrap());
            }
        } else {
            table.insert(k, json_to_toml_item(dv));
        }
    }
}

fn json_to_toml_item(v: &Value) -> toml_edit::Item {
    match v {
        Value::Object(map) => {
            let mut t = toml_edit::Table::new();
            for (k, val) in map {
                t.insert(k, json_to_toml_item(val));
            }
            toml_edit::Item::Table(t)
        }
        other => toml_edit::Item::Value(json_to_toml_value(other)),
    }
}

fn json_to_toml_value(v: &Value) -> toml_edit::Value {
    match v {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64().unwrap_or(0.0).into(),
        },
        Value::Array(arr) => {
            let mut a = toml_edit::Array::new();
            for x in arr {
                a.push(json_to_toml_value(x));
            }
            toml_edit::Value::Array(a)
        }
        Value::Object(map) => {
            let mut t = toml_edit::InlineTable::new();
            for (k, val) in map {
                t.insert(k, json_to_toml_value(val));
            }
            toml_edit::Value::InlineTable(t)
        }
        // TOML has no null; our entries never produce one.
        Value::Null => "".into(),
    }
}

pub fn build_keenable_entry(ide: &IDEDef, api_key: Option<&str>) -> Value {
    let mcp_url = format!("{}/mcp", API_BASE_URL);
    match &ide.entry_style {
        McpEntryStyle::Http {
            url_key,
            transport_type,
        } => {
            let mut entry = json!({ *url_key: mcp_url });
            if let Some(key) = api_key {
                entry["headers"] = json!({ "X-API-Key": key });
            }
            if let Some(transport) = transport_type {
                entry["type"] = json!(*transport);
            }
            entry
        }
        McpEntryStyle::Toml => {
            let mut entry = json!({ "url": mcp_url });
            if let Some(key) = api_key {
                entry["http_headers"] = json!({ "X-API-Key": key });
            }
            entry
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
        let first_arg = args.first().and_then(|v| v.as_str()).unwrap_or("");
        // Legacy npx mcp-remote format
        if first_arg == "mcp-remote" {
            if let Some(url) = args.get(1).and_then(|v| v.as_str()) {
                return Some(url.to_string());
            }
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
/// Auth is via `X-API-Key` header (same as Keenable MCP).
pub fn build_webql_entry(ide: &IDEDef, api_key: Option<&str>) -> Value {
    let mcp_url = format!("{}/mcp", WEBQL_BASE_URL);
    match &ide.entry_style {
        McpEntryStyle::Http {
            url_key,
            transport_type,
        } => {
            let mut entry = json!({ *url_key: mcp_url });
            if let Some(key) = api_key {
                entry["headers"] = json!({ "X-API-Key": key });
            }
            if let Some(transport) = transport_type {
                entry["type"] = json!(*transport);
            }
            entry
        }
        McpEntryStyle::Toml => {
            let mut entry = json!({ "url": mcp_url });
            if let Some(key) = api_key {
                entry["http_headers"] = json!({ "X-API-Key": key });
            }
            entry
        }
    }
}

/// Extract the API key from a WebQL MCP entry.
/// Checks header-based auth first, then falls back to legacy `?token=` in URL.
pub fn extract_webql_key(entry: &Value) -> Option<String> {
    // New format: X-API-Key header (same as Keenable MCP)
    if let Some(key) = extract_entry_api_key(entry) {
        return Some(key);
    }
    // Legacy format: ?token= query parameter in URL. Match the parameter
    // boundary — a bare "token=" search would also hit e.g. "access_token=".
    let url = extract_url(entry)?;
    let start = url
        .find("?token=")
        .or_else(|| url.find("&token="))
        .map(|i| i + "?token=".len())?;
    let t = &url[start..];
    Some(t.split('&').next().unwrap_or(t).to_string())
}

/// Check if a WebQL MCP entry uses the legacy `?token=` URL auth.
pub fn uses_webql_token_auth(entry: &Value) -> bool {
    let url = extract_url(entry).unwrap_or_default();
    url.contains("?token=") || url.contains("&token=")
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
            // Legacy npx mcp-remote format: --header X-API-Key:<KEY>
            if arg.as_str() == Some("--header") {
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
