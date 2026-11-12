//! Shared configure/reset logic for the Keenable MCP product.

use colored::Colorize;
use dialoguer::Select;
use serde_json::{Value, json};
use std::fs;
use std::io::IsTerminal;

use crate::api::{KeyCheck, validate_api_key};
use crate::config;
use crate::ui;

use std::path::PathBuf;

use super::ide::*;

/// Path to Claude Code's user-level settings file.
fn claude_code_settings_path() -> Option<PathBuf> {
    crate::config::home_dir().map(|h| h.join(".claude/settings.json"))
}

// ── Product definition ──────────────────────────────────────────────

/// Entry name in IDE config.
const ENTRY_NAME: &str = "keenable";
/// Human-readable name shown in output.
const DISPLAY_NAME: &str = "Keenable MCP";
/// CLI command names, used in hints.
const CONFIGURE_CMD: &str = "configure-mcp";
const RESET_CMD: &str = "reset";

/// Client-specific recommendations shown after configure.
fn show_recommendations(ide: &IDEDef) {
    if ide.name == "Cursor" {
        ui::sub_hint(
            "We recommend disabling standard search & fetch tools in Cursor Settings → Tools",
        );
        ui::sub_hint("We recommend setting a custom rule to use Keenable search");
    }
}

// ── Product status ──────────────────────────────────────────────────

pub struct ProductStatus {
    pub has_entry: bool,
    pub wrong_api_key: bool,
    /// Entry exists without API key but user now has one configured.
    pub missing_api_key: bool,
    /// Entry has an API key but no key is stored locally — re-running
    /// configure would strip it.
    pub entry_key_without_login: bool,
    pub uses_legacy_npx: bool,
    /// A keyless entry whose URL differs from the one configure would write
    /// now (keyless Codex moved from /mcp to /mcp-public).
    pub outdated_keyless_url: bool,
    pub standard_tools_disabled: bool,
    /// Denies exist in ~/.claude.json but not in ~/.claude/settings.json.
    pub has_legacy_deny_only: bool,
    pub duplicate_entries: Vec<String>,
    pub conflicting_mcps: Vec<String>,
}

pub fn get_product_status(ide: &IDEDef, api_key: Option<&str>) -> ProductStatus {
    let config = read_config_lenient(&ide.config_path);
    let existing = config
        .pointer(&format!("/{}/{}", ide.servers_key, ENTRY_NAME))
        .cloned();

    let has_entry = existing.is_some();

    let desired_key = api_key.map(|k| k.to_string());
    let existing_key = existing.as_ref().and_then(extract_entry_api_key);

    // wrong_api_key: both have keys but they differ
    let wrong_api_key =
        existing_key.is_some() && desired_key.is_some() && existing_key != desired_key;

    // missing_api_key: user has a key but the entry doesn't
    let missing_api_key = has_entry && existing_key.is_none() && desired_key.is_some();

    let entry_key_without_login = existing_key.is_some() && desired_key.is_none();

    let uses_legacy_npx = existing.as_ref().and_then(|e| e["command"].as_str()) == Some("npx");

    let outdated_keyless_url = !uses_legacy_npx
        && existing_key.is_none()
        && desired_key.is_none()
        && existing
            .as_ref()
            .is_some_and(|e| extract_url(e) != extract_url(&build_keenable_entry(ide, None)));

    let (standard_tools_disabled, has_legacy_deny_only) = if ide.has_standard_tools {
        if ide.flag == "opencode" {
            let disabled = OPENCODE_STANDARD_TOOLS.iter().all(|tool| {
                config
                    .pointer(&format!("/permission/{}", tool))
                    .and_then(|v| v.as_str())
                    == Some("deny")
            });
            (disabled, false)
        } else {
            // Only check ~/.claude/settings.json for deny (the file Claude Code reads)
            let settings_deny_list = claude_code_settings_path()
                .map(|p| {
                    let settings = read_config_lenient(&p);
                    settings
                        .pointer("/permissions/deny")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let all_denied = CLAUDE_CODE_STANDARD_TOOLS
                .iter()
                .all(|tool| settings_deny_list.iter().any(|d| d == *tool));

            // Detect legacy-only deny: present in .claude.json but not settings.json
            let legacy_deny_list = config
                .pointer("/permissions/deny")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let has_legacy_only = !all_denied
                && CLAUDE_CODE_STANDARD_TOOLS.iter().any(|tool| {
                    legacy_deny_list.iter().any(|d| d == *tool)
                        && !settings_deny_list.iter().any(|d| d == *tool)
                });

            // Also check that tools are not in allow lists (either file)
            let allow_list = config
                .pointer("/permissions/allow")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let none_allowed_in_config = !CLAUDE_CODE_STANDARD_TOOLS
                .iter()
                .any(|tool| allow_list.iter().any(|a| a == *tool));

            let none_allowed_in_settings = claude_code_settings_path()
                .map(|p| {
                    let settings = read_config_lenient(&p);
                    let allow = settings
                        .pointer("/permissions/allow")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    !CLAUDE_CODE_STANDARD_TOOLS
                        .iter()
                        .any(|tool| allow.iter().any(|a| a == *tool))
                })
                .unwrap_or(true);

            (
                all_denied && none_allowed_in_config && none_allowed_in_settings,
                has_legacy_only,
            )
        }
    } else {
        (true, false) // not applicable = no issue
    };

    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut duplicate_entries = Vec::new();
    let mut conflicting_mcps = Vec::new();

    for (name, entry) in &servers {
        if name == ENTRY_NAME {
            continue;
        }
        if let Some(url) = extract_url(entry)
            && is_keenable_url(&url)
        {
            duplicate_entries.push(name.clone());
            continue;
        }
        if is_conflicting_name(name) {
            conflicting_mcps.push(name.clone());
        }
    }

    ProductStatus {
        has_entry,
        wrong_api_key,
        missing_api_key,
        entry_key_without_login,
        uses_legacy_npx,
        outdated_keyless_url,
        standard_tools_disabled,
        has_legacy_deny_only,
        duplicate_entries,
        conflicting_mcps,
    }
}

// ── Configure flow ──────────────────────────────────────────────────

pub async fn configure(selected_flags: Vec<String>, yes: bool) {
    ui::header(&format!("keenable {}", CONFIGURE_CMD));

    // Pre-flight: validate API key (optional — unauth flow works without one)
    ui::label("Keenable CLI");
    // Key validation would report a bad SSL_CERT_FILE as "can't verify"
    // and carry on; it's a setup error, so stop here instead.
    if config::get_api_key().is_some() {
        crate::api::exit_on_bad_cert_file();
    }
    let api_key: Option<String> = match config::get_api_key() {
        Some(key) => match validate_api_key(&key).await {
            KeyCheck::Valid => {
                ui::success("API key is valid");
                Some(key)
            }
            KeyCheck::Invalid => {
                ui::warning(
                    "Stored API key is invalid — configuring for free tier (IP-based rate limits)",
                );
                ui::sub_info(&format!("Run {} to fix it", "keenable login".cyan()));
                None
            }
            // Can't verify ≠ invalid: silently stripping the key from client
            // configs over a network blip would break working setups.
            KeyCheck::Unreachable(err) => {
                ui::warning("Could not verify API key (network error) — using the stored key");
                if let Some(err) = err {
                    ui::sub_info(&err.display());
                    err.print_details();
                }
                Some(key)
            }
        },
        None => {
            ui::info("No API key — configuring for free tier (IP-based rate limits)");
            ui::sub_info(&format!(
                "Run {} for higher limits",
                "keenable login".cyan()
            ));
            None
        }
    };

    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();
    let not_detected: Vec<&IDEDef> = all.iter().filter(|ide| !is_detected(ide)).collect();

    if selected_flags.is_empty() {
        show_configure_status(&detected, &not_detected, api_key.as_deref());
    } else {
        let is_all = selected_flags.contains(&"all".to_string());

        let targets: Vec<&IDEDef> = if is_all {
            detected.clone()
        } else {
            detected
                .iter()
                .filter(|ide| selected_flags.contains(&ide.flag.to_string()))
                .copied()
                .collect()
        };

        if is_all && selected_flags.len() > 1 {
            ui::warning("--all selects every detected client; individual client flags are ignored");
        }
        if !is_all {
            warn_unmatched_flags(&selected_flags, &all, &detected);
        }

        if targets.is_empty() {
            ui::error("No matching clients found to configure");
            ui::hint(&format!(
                "Run `keenable {}` to see available clients",
                CONFIGURE_CMD
            ));
            eprintln!();
            std::process::exit(1);
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        if !confirm_configure(&target_names, yes) {
            eprintln!();
            return;
        }

        let mut all_ok = true;
        for ide in &targets {
            ui::label(ide.name);
            all_ok &= configure_ide(ide, api_key.as_deref());
            show_recommendations(ide);
        }

        eprintln!();
        if all_ok {
            ui::success("Configuration complete");
        } else {
            ui::error("Some clients could not be configured");
            eprintln!();
            std::process::exit(1);
        }
    }

    eprintln!();
}

fn configure_ide(ide: &IDEDef, api_key: Option<&str>) -> bool {
    let Some(mut config) = read_config_for_write(&ide.config_path, "Fix the file and re-run.")
    else {
        return false;
    };
    let mut config_changed = false;
    let mut delta = Delta::new();
    if !ide.config_path.exists() {
        delta.insert("created_file".into(), json!(true));
    }

    // Step 1: Remove duplicate entries (other names pointing at Keenable's URL)
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut duplicate_entries: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    for (name, entry) in &servers {
        if name == ENTRY_NAME {
            continue;
        }
        if let Some(url) = extract_url(entry)
            && is_keenable_url(&url)
        {
            duplicate_entries.push(name.clone());
            continue;
        }
        if is_conflicting_name(name) {
            conflicts.push(name.clone());
        }
    }

    if !duplicate_entries.is_empty() {
        for name in &duplicate_entries {
            if let Some(obj) = config
                .get_mut(ide.servers_key)
                .and_then(|v| v.as_object_mut())
            {
                obj.remove(name.as_str());
            }
        }
        config_changed = true;
        ui::sub_success(&format!(
            "Removed duplicate entries: {}",
            duplicate_entries.join(", ")
        ));
    }

    if !conflicts.is_empty() {
        ui::sub_warning(&format!(
            "Other search MCPs found: {}",
            conflicts.join(", ")
        ));
    }

    // Step 2: Add/update the Keenable MCP entry
    let desired = build_keenable_entry(ide, api_key);
    let existing = config
        .pointer(&format!("/{}/{}", ide.servers_key, ENTRY_NAME))
        .cloned();

    match existing {
        Some(ref entry) if *entry == desired => {
            ui::sub_done(&format!("{} already configured", DISPLAY_NAME));
        }
        Some(ref entry) => {
            let existing_key = extract_entry_api_key(entry);
            let desired_key = api_key.map(|k| k.to_string());
            if existing_key != desired_key {
                if existing_key.is_some() && desired_key.is_some() {
                    ui::sub_warning(&format!("Updating API key in {} entry", DISPLAY_NAME));
                } else if existing_key.is_none() && desired_key.is_some() {
                    ui::sub_warning(&format!("Adding API key to {} entry", DISPLAY_NAME));
                } else if existing_key.is_some() && desired_key.is_none() {
                    ui::sub_warning(&format!(
                        "Removing API key from {} entry (switching to free tier)",
                        DISPLAY_NAME
                    ));
                }
            }
            if entry["command"].as_str() == Some("npx") {
                ui::sub_warning(
                    "Replacing npx mcp-remote with a direct HTTP entry (no Node.js needed)",
                );
            }
            config[ide.servers_key][ENTRY_NAME] = desired;
            config_changed = true;
            ui::sub_success(&format!("{} updated", DISPLAY_NAME));
        }
        None => {
            if config.get(ide.servers_key).is_none() {
                delta.insert("created_keys".into(), json!([ide.servers_key]));
            }
            ensure_object(&mut config, ide.servers_key);
            config[ide.servers_key][ENTRY_NAME] = desired;
            config_changed = true;
            ui::sub_success(&format!("{} added", DISPLAY_NAME));
        }
    }

    // Step 3: Disable standard tools where the client has them
    if ide.has_standard_tools {
        let tools_delta = if ide.flag == "opencode" {
            disable_opencode_standard_tools(&mut config, &mut config_changed)
        } else {
            disable_standard_tools(&mut config, &mut config_changed)
        };
        for (k, v) in tools_delta {
            if k == "created_keys" {
                let mut keys = delta.remove(&k).unwrap_or_else(|| json!([]));
                keys.as_array_mut()
                    .unwrap()
                    .extend(v.as_array().cloned().unwrap_or_default());
                delta.insert(k, keys);
            } else {
                delta.insert(k, v);
            }
        }
    }

    if config_changed && let Err(e) = write_config(&ide.config_path, &config) {
        ui::sub_error(&format!(
            "Failed to write {}: {}",
            ide.config_path.display(),
            e
        ));
        return false;
    }
    // Only once written: reset must not undo changes that never landed.
    record(&ide.config_path, &delta);
    true
}

// ── Reset flow ──────────────────────────────────────────────────────

pub fn reset(selected_flags: Vec<String>, yes: bool) {
    ui::header(&format!("keenable {}", RESET_CMD));

    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();

    let configured: Vec<&IDEDef> = detected
        .iter()
        .filter(|ide| has_product_entry(ide) || (ide.flag == "codex" && has_codex_apps_cache()))
        .copied()
        .collect();

    if selected_flags.is_empty() {
        show_reset_status(&configured);
    } else {
        let is_all = selected_flags.contains(&"all".to_string());

        let targets: Vec<&IDEDef> = if is_all {
            configured.clone()
        } else {
            configured
                .iter()
                .filter(|ide| selected_flags.contains(&ide.flag.to_string()))
                .copied()
                .collect()
        };

        if is_all && selected_flags.len() > 1 {
            ui::warning(
                "--all selects every configured client; individual client flags are ignored",
            );
        }
        if !is_all {
            for flag in &selected_flags {
                let matched = all.iter().any(|ide| ide.flag == flag.as_str());
                let configured_match = configured.iter().any(|ide| ide.flag == flag.as_str());
                if !matched {
                    ui::warning(&format!("Unknown client: --{}", flag));
                } else if !configured_match {
                    let ide_name = all
                        .iter()
                        .find(|ide| ide.flag == flag.as_str())
                        .unwrap()
                        .name;
                    let is_installed = detected.iter().any(|ide| ide.flag == flag.as_str());
                    if !is_installed {
                        ui::warning(&format!("{} is not installed", ide_name));
                    } else {
                        ui::warning(&format!(
                            "{} doesn't have {} configured",
                            ide_name, DISPLAY_NAME
                        ));
                    }
                }
            }
        }

        if targets.is_empty() {
            ui::info(&format!(
                "No clients with {} configuration found to reset.",
                DISPLAY_NAME
            ));
            eprintln!();
            return;
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        ui::save_cursor();
        if !confirm_reset(&target_names, yes) {
            eprintln!();
            return;
        }
        ui::restore_and_clear();

        ui::label("Your Clients");
        for ide in &targets {
            eprintln!("   {} {}", "✓".green(), ide.name.green());
        }

        let mut all_ok = true;
        for ide in &targets {
            ui::label(ide.name);
            all_ok &= reset_ide(ide);
        }

        eprintln!();
        if all_ok {
            ui::success("Reset complete");
        } else {
            ui::error("Some clients could not be reset");
            eprintln!();
            std::process::exit(1);
        }
    }

    eprintln!();
}

fn reset_ide(ide: &IDEDef) -> bool {
    let Some(mut config) = read_config_for_write(&ide.config_path, "Fix the file and re-run.")
    else {
        return false;
    };
    let mut config_changed = false;
    let rec = config::get_restore_record(&ide.config_path);

    // Step 1: Remove the Keenable MCP entry
    if let Some(servers) = config
        .get_mut(ide.servers_key)
        .and_then(|v| v.as_object_mut())
    {
        if servers.remove(ENTRY_NAME).is_some() {
            config_changed = true;
            ui::sub_success(&format!("Removed {} entry", DISPLAY_NAME));
        } else {
            ui::sub_done(&format!("No {} entry to remove", DISPLAY_NAME));
        }
    } else {
        ui::sub_done(&format!("No {} entry to remove", DISPLAY_NAME));
    }

    // Step 2: Remove any other entries pointing at Keenable's URL
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut other_entries: Vec<String> = Vec::new();
    for (name, entry) in &servers {
        if let Some(url) = extract_url(entry)
            && is_keenable_url(&url)
        {
            other_entries.push(name.clone());
        }
    }

    if !other_entries.is_empty() {
        for name in &other_entries {
            if let Some(obj) = config
                .get_mut(ide.servers_key)
                .and_then(|v| v.as_object_mut())
            {
                obj.remove(name.as_str());
            }
        }
        config_changed = true;
        ui::sub_success(&format!(
            "Removed additional {} entries: {}",
            DISPLAY_NAME,
            other_entries.join(", ")
        ));
    }

    // Step 3: Restore standard tools where the client has them
    if ide.has_standard_tools {
        if ide.flag == "opencode" {
            restore_opencode_standard_tools(&mut config, &mut config_changed, rec.as_ref());
        } else {
            restore_standard_tools(&mut config, &mut config_changed, rec.as_ref());
        }
    }

    // Drop the servers key configure created, if nothing else is in it.
    if let Some(rec) = &rec
        && created_key(rec, ide.servers_key)
        && config
            .get(ide.servers_key)
            .and_then(|v| v.as_object())
            .is_some_and(|m| m.is_empty())
        && let Some(obj) = config.as_object_mut()
    {
        obj.remove(ide.servers_key);
        config_changed = true;
    }

    // Step 4: Clean Codex Apps cache if applicable
    if ide.flag == "codex" {
        clean_codex_apps_cache();
    }

    if config_changed && let Err(e) = write_or_remove(&ide.config_path, &config, rec.as_ref()) {
        ui::sub_error(&format!(
            "Failed to write {}: {}",
            ide.config_path.display(),
            e
        ));
        return false;
    }
    config::clear_restore_record(&ide.config_path);
    true
}

// ── Status display ──────────────────────────────────────────────────

fn show_configure_status(detected: &[&IDEDef], not_detected: &[&IDEDef], api_key: Option<&str>) {
    ui::label("Your Clients");

    if detected.is_empty() {
        ui::info("No supported clients detected.");
        return;
    }

    let mut any_unconfigured = false;

    for ide in detected {
        let status = get_product_status(ide, api_key);

        let has_issues = status.wrong_api_key
            || status.missing_api_key
            || status.entry_key_without_login
            || status.uses_legacy_npx
            || status.outdated_keyless_url
            || !status.duplicate_entries.is_empty()
            || !status.conflicting_mcps.is_empty()
            || (ide.has_standard_tools && !status.standard_tools_disabled)
            || status.has_legacy_deny_only;

        if !status.has_entry {
            any_unconfigured = true;
            eprintln!("   {} {}", "✗".red(), ide.name);
            eprintln!(
                "      {}",
                format!("- Run keenable {} --{}", CONFIGURE_CMD, ide.flag).dimmed()
            );
        } else if has_issues {
            any_unconfigured = true;
            eprintln!("   {} {}", "⚠".yellow(), ide.name.yellow());
            show_status_issues(ide, &status);
            show_recommendations(ide);
        } else {
            eprintln!("   {} {}", "✓".green(), ide.name.green());
            show_recommendations(ide);
        }
    }

    if !not_detected.is_empty() {
        let names: Vec<&str> = not_detected.iter().map(|ide| ide.name).collect();
        eprintln!();
        ui::info(&format!(
            "{} {}",
            "Not installed:".dimmed(),
            names.join(", ").dimmed()
        ));
    }

    if any_unconfigured {
        ui::hint(&format!(
            "Run {} to configure all detected clients at once",
            format!("keenable {} --all", CONFIGURE_CMD).cyan()
        ));
    }
}

fn show_status_issues(ide: &IDEDef, status: &ProductStatus) {
    if status.uses_legacy_npx {
        ui::sub_warning(&format!(
            "Uses npx mcp-remote (requires Node.js). Re-run {} to switch to a direct HTTP entry",
            CONFIGURE_CMD
        ));
    }
    if status.outdated_keyless_url {
        ui::sub_warning(&format!(
            "Points at an older endpoint. Re-run {} to update it",
            CONFIGURE_CMD
        ));
    }
    if status.wrong_api_key {
        ui::sub_warning("Different API key configured");
    }
    if status.missing_api_key {
        ui::sub_warning("Missing API key (configured for free tier)");
    }
    if status.entry_key_without_login {
        ui::sub_warning(&format!(
            "Configured with an API key, but none is stored locally — re-running {} would remove it",
            CONFIGURE_CMD
        ));
    }
    if !status.duplicate_entries.is_empty() {
        ui::sub_warning(&format!(
            "Duplicate entries: {}",
            status.duplicate_entries.join(", ")
        ));
    }
    if !status.conflicting_mcps.is_empty() {
        ui::sub_warning(&format!(
            "Other search MCPs found: {}",
            status.conflicting_mcps.join(", ")
        ));
    }
    if ide.has_standard_tools && !status.standard_tools_disabled {
        let tools = if ide.flag == "opencode" {
            OPENCODE_STANDARD_TOOLS.join(", ")
        } else {
            CLAUDE_CODE_STANDARD_TOOLS.join(", ")
        };
        ui::sub_warning(&format!("Standard tools ({}) not disabled", tools));
    }
    if status.has_legacy_deny_only {
        ui::sub_warning(&format!(
            "Legacy deny found in .claude.json (ignored by Claude Code). Re-run {} to migrate",
            CONFIGURE_CMD
        ));
    }
}

fn show_reset_status(configured: &[&IDEDef]) {
    ui::label("Your Clients");

    if configured.is_empty() {
        ui::info(&format!(
            "No clients have {} configured. Nothing to reset.",
            DISPLAY_NAME
        ));
        return;
    }

    for ide in configured {
        eprintln!("   {} {}", "✓".green(), ide.name.green());
        eprintln!(
            "      {}",
            format!("- Run keenable {} --{}", RESET_CMD, ide.flag).dimmed()
        );
    }

    ui::hint(&format!(
        "Run {} to reset all at once",
        format!("keenable {} --all", RESET_CMD).cyan()
    ));
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Strict read for paths that write the config back. On a read/parse error
/// reports it and returns None — proceeding would replace the user's file
/// with just our entries (see `read_config`).
fn read_config_for_write(path: &std::path::Path, consequence: &str) -> Option<Value> {
    match read_config(&path.to_path_buf()) {
        Ok(c) => Some(c),
        Err(e) => {
            ui::sub_error(&format!(
                "{}: {} — not modifying. {}",
                path.display(),
                e,
                consequence
            ));
            None
        }
    }
}

/// Nested index assignment panics when the key holds a non-object value
/// (hand-edited configs) — reset it to an empty object first.
fn ensure_object(config: &mut Value, key: &str) {
    if !config[key].is_object() {
        config[key] = json!({});
    }
}

/// True when the config holds the `keenable` entry or any other entry whose
/// URL points at Keenable, the same match the reset removal step uses.
fn has_product_entry(ide: &IDEDef) -> bool {
    let config = read_config_lenient(&ide.config_path);
    let Some(servers) = config.get(ide.servers_key).and_then(|v| v.as_object()) else {
        return false;
    };
    servers.contains_key(ENTRY_NAME)
        || servers
            .values()
            .any(|entry| extract_url(entry).is_some_and(|url| is_keenable_url(&url)))
}

fn warn_unmatched_flags(selected_flags: &[String], all: &[IDEDef], detected: &[&IDEDef]) {
    for flag in selected_flags {
        let matched = all.iter().any(|ide| ide.flag == flag.as_str());
        let detected_match = detected.iter().any(|ide| ide.flag == flag.as_str());
        if !matched {
            ui::warning(&format!("Unknown client: --{}", flag));
        } else if !detected_match {
            let ide_name = all
                .iter()
                .find(|ide| ide.flag == flag.as_str())
                .unwrap()
                .name;
            ui::warning(&format!("{} is not installed", ide_name));
        }
    }
}

// dialoguer reads keys from stdin; with piped stdin it errors and we'd
// silently treat that as "Cancel". Fail loudly with the fix instead.
fn require_terminal() {
    if !std::io::stdin().is_terminal() {
        ui::error("Confirmation needed, but stdin is not a terminal");
        ui::hint(&format!(
            "Re-run with {} to skip the confirmation prompt",
            "--yes".cyan()
        ));
        eprintln!();
        std::process::exit(1);
    }
}

fn confirm_configure(ide_names: &[&str], yes: bool) -> bool {
    if yes || config::get_skip_setup_confirmation() {
        return true;
    }
    require_terminal();

    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };

    eprintln!(
        "   This will add {} to {} and disable\n   built-in search tools where applicable.",
        DISPLAY_NAME,
        target.bold()
    );
    eprintln!(
        "   It also removes {} from allow lists in project-level\n   .claude/settings.local.json files under common dev directories.",
        CLAUDE_CODE_STANDARD_TOOLS.join("/")
    );
    eprintln!();

    let choices = &["Proceed", "Proceed and don't ask again", "Cancel"];

    let selection = Select::new().items(choices).default(0).interact_opt();

    match selection {
        Ok(Some(0)) => true,
        Ok(Some(1)) => {
            config::set_skip_setup_confirmation(true);
            true
        }
        _ => {
            eprintln!();
            ui::info("Configuration cancelled.");
            false
        }
    }
}

fn confirm_reset(ide_names: &[&str], yes: bool) -> bool {
    // Unlike configure, the stored skip_setup_confirmation does not apply:
    // reset is destructive, so only an explicit --yes skips the prompt.
    if yes {
        return true;
    }
    require_terminal();

    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };

    eprintln!(
        "   This will remove {} from {} and re-enable\n   built-in search tools where applicable.",
        DISPLAY_NAME,
        target.bold()
    );
    eprintln!();

    let choices = &["Proceed", "Cancel"];

    // Destructive op: Enter must not proceed by accident.
    let selection = Select::new().items(choices).default(1).interact_opt();

    match selection {
        Ok(Some(0)) => true,
        _ => {
            eprintln!();
            ui::info("Reset cancelled.");
            false
        }
    }
}

// ── Standard tools ──────────────────────────────────────────────────

/// Restore-record delta for one file (see `config::update_restore_record`).
type Delta = serde_json::Map<String, Value>;

/// Fold `delta` into a stored record. Repeated configures keep the first
/// prior state: a later run sees our own changes, not the user's original.
fn merge_record(rec: &mut Value, delta: &Delta) {
    if delta.get("created_file") == Some(&json!(true)) {
        rec["created_file"] = json!(true);
    }
    for list in ["created_keys", "allow_removed"] {
        let Some(new) = delta.get(list).and_then(|v| v.as_array()) else {
            continue;
        };
        if !rec[list].is_array() {
            rec[list] = json!([]);
        }
        let arr = rec[list].as_array_mut().unwrap();
        for item in new {
            let same = |a: &Value| match list {
                "allow_removed" => a["tool"] == item["tool"],
                _ => a == item,
            };
            if !arr.iter().any(same) {
                arr.push(item.clone());
            }
        }
    }
    if let Some(prev) = delta.get("permission_prev").and_then(|v| v.as_object()) {
        if !rec["permission_prev"].is_object() {
            rec["permission_prev"] = json!({});
        }
        for (tool, value) in prev {
            if rec["permission_prev"].get(tool).is_none() {
                rec["permission_prev"][tool] = value.clone();
            }
        }
    }
    if let Some(was) = delta.get("permission_was")
        && rec.get("permission_was").is_none()
    {
        rec["permission_was"] = was.clone();
    }
}

fn record(path: &std::path::Path, delta: &Delta) {
    if !delta.is_empty() {
        config::update_restore_record(path, |rec| merge_record(rec, delta));
    }
}

fn display_path(path: &std::path::Path) -> String {
    crate::config::home_dir()
        .and_then(|h| {
            path.strip_prefix(h)
                .ok()
                .map(|p| format!("~/{}", p.display()))
        })
        .unwrap_or_else(|| path.display().to_string())
}

/// Write `config` to `path`, or delete the file when configure created it
/// and nothing else is left in it.
fn write_or_remove(path: &PathBuf, config: &Value, rec: Option<&Value>) -> std::io::Result<()> {
    let created = rec.is_some_and(|r| r["created_file"] == json!(true));
    let empty = config.as_object().is_some_and(|m| m.is_empty());
    if created && empty {
        match fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        write_config(path, config)
    }
}

/// Configure: deny the standard tools and strip them from allow lists.
/// Returns the delta for `config` (~/.claude.json), recorded by the caller
/// once that file is written; the other files are recorded here.
fn disable_standard_tools(config: &mut Value, changed: &mut bool) -> Delta {
    // Claude Code reads permissions from ~/.claude/settings.json — denies in
    // ~/.claude.json are ignored. Migrate legacy entries out instead of
    // adding more.
    let mut legacy_changed = false;
    remove_from_deny_list(config, &mut legacy_changed, CLAUDE_CODE_STANDARD_TOOLS);
    if legacy_changed {
        *changed = true;
        ui::sub_success(
            "Removed legacy deny entries from .claude.json (Claude Code reads settings.json)",
        );
    }

    let mut main_delta = Delta::new();
    let removed = strip_allow_list(config);
    if !removed.is_empty() {
        *changed = true;
        ui::sub_success(&allow_removed_msg(&removed));
        main_delta.insert("allow_removed".into(), allow_removed_json(&removed));
    }

    // Add deny + remove from allow list in ~/.claude/settings.json
    if let Some(settings_path) = claude_code_settings_path()
        && let Some(mut settings) =
            read_config_for_write(&settings_path, "Standard tools may still be enabled")
    {
        let existed = settings_path.exists();
        let mut settings_changed = false;
        let deny_result = add_deny_to_settings_quiet(&mut settings, &mut settings_changed);
        let removed = strip_allow_list(&mut settings);
        settings_changed |= !removed.is_empty();
        if settings_changed {
            match write_config(&settings_path, &settings) {
                Ok(()) => {
                    let mut delta = Delta::new();
                    if !existed {
                        delta.insert("created_file".into(), json!(true));
                    }
                    if !removed.is_empty() {
                        delta.insert("allow_removed".into(), allow_removed_json(&removed));
                    }
                    record(&settings_path, &delta);
                    match deny_result {
                        Some((msg, added)) => {
                            // Record what we added so reset restores only
                            // that, not denies the user set themselves.
                            config::add_managed_deny_tools(&added);
                            ui::sub_success(&msg);
                        }
                        None => config::add_managed_deny_tools(&[]),
                    }
                    if !removed.is_empty() {
                        ui::sub_success(&allow_removed_msg(&removed));
                    }
                }
                Err(e) => {
                    ui::sub_error(&format!(
                        "Failed to write {}: {}. Standard tools may still be enabled",
                        settings_path.display(),
                        e
                    ));
                }
            }
        } else {
            config::add_managed_deny_tools(&[]);
            ui::sub_done(&format!(
                "Standard tools already disabled: {}",
                CLAUDE_CODE_STANDARD_TOOLS.join(", ")
            ));
        }
    }

    // Scan project-level .claude/settings.local.json files and remove from allow lists
    remove_from_project_allow_lists();
    main_delta
}

/// Remove standard tools from `permissions.allow`, dropping the list (and
/// `permissions`) when it ends up empty. Returns each removed tool with its
/// index in the original list, so reset can put it back in place.
fn strip_allow_list(config: &mut Value) -> Vec<(String, usize)> {
    let Some(allow) = config
        .pointer("/permissions/allow")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for (i, v) in allow.iter().enumerate() {
        match v.as_str() {
            Some(t) if CLAUDE_CODE_STANDARD_TOOLS.contains(&t) => removed.push((t.to_string(), i)),
            _ => kept.push(v.clone()),
        }
    }
    if removed.is_empty() {
        return removed;
    }
    if kept.is_empty() {
        if let Some(perms) = config
            .get_mut("permissions")
            .and_then(|v| v.as_object_mut())
        {
            perms.remove("allow");
            if perms.is_empty()
                && let Some(obj) = config.as_object_mut()
            {
                obj.remove("permissions");
            }
        }
    } else {
        config["permissions"]["allow"] = Value::Array(kept);
    }
    removed
}

fn allow_removed_msg(removed: &[(String, usize)]) -> String {
    let tools: Vec<&str> = removed.iter().map(|(t, _)| t.as_str()).collect();
    format!("Removed {} from allow list", tools.join(", "))
}

fn allow_removed_json(removed: &[(String, usize)]) -> Value {
    removed
        .iter()
        .map(|(t, i)| json!({"tool": t, "index": i}))
        .collect()
}

/// Put recorded tools back into `permissions.allow` at their old positions.
/// Returns the tools restored (ones already present are skipped).
fn restore_allow_list(config: &mut Value, rec: &Value) -> Vec<String> {
    let mut entries: Vec<(String, usize)> = rec["allow_removed"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some((
                        e["tool"].as_str()?.to_string(),
                        e["index"].as_u64()? as usize,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by_key(|(_, i)| *i);
    let mut restored = Vec::new();
    for (tool, index) in entries {
        ensure_object(config, "permissions");
        if !config["permissions"]["allow"].is_array() {
            config["permissions"]["allow"] = json!([]);
        }
        let allow = config["permissions"]["allow"].as_array_mut().unwrap();
        if allow.iter().any(|a| a.as_str() == Some(tool.as_str())) {
            continue;
        }
        allow.insert(index.min(allow.len()), json!(tool));
        restored.push(tool);
    }
    restored
}

/// Add standard tools to the `permissions.deny` list in a settings file.
/// Returns a success message and the tools added, if changes were made
/// (caller prints and records after a successful write).
fn add_deny_to_settings_quiet(
    config: &mut Value,
    changed: &mut bool,
) -> Option<(String, Vec<String>)> {
    let deny_list = config
        .pointer("/permissions/deny")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let added: Vec<String> = CLAUDE_CODE_STANDARD_TOOLS
        .iter()
        .filter(|tool| !deny_list.iter().any(|d| d == **tool))
        .map(|s| s.to_string())
        .collect();

    if !added.is_empty() {
        let msg = format!("Added {} to settings.json deny list", added.join(", "));
        let mut new_deny = deny_list;
        new_deny.extend(added.iter().cloned());
        ensure_object(config, "permissions");
        config["permissions"]["deny"] = json!(new_deny);
        *changed = true;
        Some((msg, added))
    } else {
        None
    }
}

/// Scan project-level `.claude/settings.local.json` files and remove standard
/// tools from their allow lists. Walks common project directories to find these
/// files.
fn remove_from_project_allow_lists() {
    let home = match crate::config::home_dir() {
        Some(h) => h,
        None => return,
    };

    // Walk common dev directories looking for .claude/settings.local.json
    let search_dirs = ["dev", "projects", "src", "repos", "work", "code"];
    let mut settings_files: Vec<PathBuf> = Vec::new();

    for dir in &search_dirs {
        let search_root = home.join(dir);
        if search_root.is_dir() {
            find_claude_settings(&search_root, &mut settings_files, 0, 3);
        }
    }

    // Also check home directory itself (for projects directly under ~)
    find_claude_settings(&home, &mut settings_files, 0, 1);

    for path in settings_files {
        let Ok(mut config) = read_config(&path) else {
            continue; // unparseable project settings — leave untouched
        };
        let removed = strip_allow_list(&mut config);
        if removed.is_empty() {
            continue;
        }
        match write_config(&path, &config) {
            Ok(()) => {
                let mut delta = Delta::new();
                delta.insert("allow_removed".into(), allow_removed_json(&removed));
                record(&path, &delta);
                ui::sub_success(&format!(
                    "{} in {}",
                    allow_removed_msg(&removed),
                    display_path(&path)
                ));
            }
            Err(e) => {
                ui::sub_error(&format!("Failed to write {}: {}", display_path(&path), e));
            }
        }
    }
}

/// Recursively find `.claude/settings.local.json` files.
fn find_claude_settings(
    dir: &std::path::Path,
    results: &mut Vec<PathBuf>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    let candidate = dir.join(".claude/settings.local.json");
    if candidate.is_file() {
        results.push(candidate);
    }
    if depth < max_depth
        && let Ok(entries) = fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip hidden dirs (except .claude which we handle above),
                // node_modules, and other noise
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.')
                    || name_str == "node_modules"
                    || name_str == "target"
                    || name_str == "vendor"
                {
                    continue;
                }
                find_claude_settings(&path, results, depth + 1, max_depth);
            }
        }
    }
}

/// Configure: set the OpenCode tools to "deny". Returns the delta recording
/// each tool's prior value (null = unset) and a non-object `permission`.
fn disable_opencode_standard_tools(config: &mut Value, changed: &mut bool) -> Delta {
    let mut delta = Delta::new();
    let missing: Vec<&str> = OPENCODE_STANDARD_TOOLS
        .iter()
        .filter(|tool| {
            config
                .pointer(&format!("/permission/{}", tool))
                .and_then(|v| v.as_str())
                != Some("deny")
        })
        .copied()
        .collect();

    if missing.is_empty() {
        ui::sub_done(&format!(
            "Standard tools already disabled: {}",
            OPENCODE_STANDARD_TOOLS.join(", ")
        ));
        // Recorded as "changed nothing", so reset keeps the user's deny.
        delta.insert("permission_prev".into(), json!({}));
        return delta;
    }

    match config.get("permission") {
        None => {
            delta.insert("created_keys".into(), json!(["permission"]));
        }
        Some(p) if !p.is_object() => {
            delta.insert("permission_was".into(), p.clone());
        }
        _ => {}
    }
    ensure_object(config, "permission");
    let mut prev = serde_json::Map::new();
    for tool in &missing {
        let old = config["permission"]
            .get(*tool)
            .cloned()
            .unwrap_or(Value::Null);
        prev.insert(tool.to_string(), old);
        config["permission"][*tool] = json!("deny");
    }
    delta.insert("permission_prev".into(), Value::Object(prev));
    *changed = true;
    ui::sub_success(&format!("Disabled standard tools: {}", missing.join(", ")));
    delta
}

/// Reset for Claude Code: drop the denies configure added and put back the
/// allow entries it removed, in ~/.claude.json (`config`), settings.json and
/// project settings.local.json files.
fn restore_standard_tools(config: &mut Value, changed: &mut bool, rec: Option<&Value>) {
    // Legacy cleanup: old versions wrote denies into ~/.claude.json too.
    let mut legacy_changed = false;
    remove_from_deny_list(config, &mut legacy_changed, CLAUDE_CODE_STANDARD_TOOLS);
    if legacy_changed {
        *changed = true;
        ui::sub_success("Removed legacy deny entries from .claude.json");
    }

    if let Some(rec) = rec {
        let restored = restore_allow_list(config, rec);
        if !restored.is_empty() {
            *changed = true;
            ui::sub_success(&format!(
                "Restored {} to allow list in .claude.json",
                restored.join(", ")
            ));
        }
    }

    // settings.json: remove only the denies configure recorded as its own.
    // No record (pre-record configs) falls back to removing all of them.
    let managed = config::get_managed_deny_tools();
    let tools: Vec<&str> = match &managed {
        Some(list) => list.iter().map(String::as_str).collect(),
        None => CLAUDE_CODE_STANDARD_TOOLS.to_vec(),
    };

    if let Some(settings_path) = claude_code_settings_path() {
        let settings_rec = config::get_restore_record(&settings_path);
        if tools.is_empty() && settings_rec.is_none() {
            ui::sub_done("Standard tools were not disabled by configure — leaving them as-is");
        } else {
            let Some(mut settings) =
                read_config_for_write(&settings_path, "Standard tools may still be disabled")
            else {
                return; // keep the records so a re-run can still restore
            };
            let mut settings_changed = false;
            remove_from_deny_list(&mut settings, &mut settings_changed, &tools);
            let denies_removed = settings_changed;
            let restored = settings_rec
                .as_ref()
                .map(|r| restore_allow_list(&mut settings, r))
                .unwrap_or_default();
            settings_changed |= !restored.is_empty();
            if settings_changed {
                if let Err(e) = write_or_remove(&settings_path, &settings, settings_rec.as_ref()) {
                    ui::sub_error(&format!(
                        "Failed to write {}: {}",
                        settings_path.display(),
                        e
                    ));
                    return; // keep the records
                }
                if denies_removed {
                    ui::sub_success(&format!("Re-enabled standard tools: {}", tools.join(", ")));
                }
                if !restored.is_empty() {
                    ui::sub_success(&format!(
                        "Restored {} to allow list in settings.json",
                        restored.join(", ")
                    ));
                }
            } else {
                ui::sub_done("Standard tools were not disabled");
            }
            config::clear_restore_record(&settings_path);
        }
    }
    config::clear_managed_deny_tools();

    restore_project_allow_lists();
}

/// Put back the allow entries configure removed from project
/// settings.local.json files.
fn restore_project_allow_lists() {
    let skip: Vec<PathBuf> = all_ides()
        .into_iter()
        .map(|ide| ide.config_path)
        .chain(claude_code_settings_path())
        .collect();
    for (path, rec) in config::restore_records() {
        if skip.contains(&path) || rec.get("allow_removed").is_none() {
            continue;
        }
        if !path.is_file() {
            config::clear_restore_record(&path); // moved or deleted since
            continue;
        }
        let Some(mut settings) = read_config_for_write(&path, "Allow list not restored") else {
            continue;
        };
        let restored = restore_allow_list(&mut settings, &rec);
        if !restored.is_empty() {
            if let Err(e) = write_config(&path, &settings) {
                ui::sub_error(&format!("Failed to write {}: {}", display_path(&path), e));
                continue;
            }
            ui::sub_success(&format!(
                "Restored {} to allow list in {}",
                restored.join(", "),
                display_path(&path)
            ));
        }
        config::clear_restore_record(&path);
    }
}

/// Remove the given tools from a `permissions.deny` list.
fn remove_from_deny_list(config: &mut Value, changed: &mut bool, tools: &[&str]) {
    let deny_list = config
        .pointer("/permissions/deny")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let had_tools = tools
        .iter()
        .any(|tool| deny_list.iter().any(|d| d == *tool));

    if had_tools {
        let filtered: Vec<String> = deny_list
            .into_iter()
            .filter(|d| !tools.contains(&d.as_str()))
            .collect();
        if filtered.is_empty() {
            if let Some(perms) = config
                .get_mut("permissions")
                .and_then(|v| v.as_object_mut())
            {
                perms.remove("deny");
                if perms.is_empty()
                    && let Some(obj) = config.as_object_mut()
                {
                    obj.remove("permissions");
                }
            }
        } else {
            config["permissions"]["deny"] = json!(filtered);
        }
        *changed = true;
    }
}

/// Reset for OpenCode. With a record, each tool configure denied gets its
/// prior value back (if still "deny"); without one (older configs), every
/// standard-tool deny is removed.
fn restore_opencode_standard_tools(config: &mut Value, changed: &mut bool, rec: Option<&Value>) {
    let Some(rec) = rec else {
        return restore_opencode_unrecorded(config, changed);
    };
    let Some(prev) = rec["permission_prev"].as_object().filter(|p| !p.is_empty()) else {
        ui::sub_done("Standard tools were not disabled by configure — leaving them as-is");
        return;
    };

    let mut restored = Vec::new();
    if let Some(perms) = config.get_mut("permission").and_then(|v| v.as_object_mut()) {
        for (tool, old) in prev {
            if perms.get(tool).and_then(|v| v.as_str()) != Some("deny") {
                continue; // changed since configure; leave it
            }
            if old.is_null() {
                perms.remove(tool);
            } else {
                perms.insert(tool.clone(), old.clone());
            }
            restored.push(tool.as_str());
        }
        if perms.is_empty() {
            if let Some(was) = rec.get("permission_was") {
                config["permission"] = was.clone();
            } else if created_key(rec, "permission")
                && let Some(obj) = config.as_object_mut()
            {
                obj.remove("permission");
            }
        }
    }
    if restored.is_empty() {
        ui::sub_done("Standard tools were not disabled");
    } else {
        *changed = true;
        ui::sub_success(&format!(
            "Re-enabled standard tools: {}",
            restored.join(", ")
        ));
    }
}

fn restore_opencode_unrecorded(config: &mut Value, changed: &mut bool) {
    let had_tools = OPENCODE_STANDARD_TOOLS.iter().any(|tool| {
        config
            .pointer(&format!("/permission/{}", tool))
            .and_then(|v| v.as_str())
            == Some("deny")
    });

    if had_tools {
        for tool in OPENCODE_STANDARD_TOOLS {
            if let Some(perms) = config.get_mut("permission").and_then(|v| v.as_object_mut()) {
                perms.remove(*tool);
                if perms.is_empty()
                    && let Some(obj) = config.as_object_mut()
                {
                    obj.remove("permission");
                }
            }
        }
        *changed = true;
        ui::sub_success(&format!(
            "Re-enabled standard tools: {}",
            OPENCODE_STANDARD_TOOLS.join(", ")
        ));
    } else {
        ui::sub_done("Standard tools were not disabled");
    }
}

fn created_key(rec: &Value, key: &str) -> bool {
    rec["created_keys"]
        .as_array()
        .is_some_and(|keys| keys.iter().any(|k| k == key))
}

// ── Codex Apps cache ────────────────────────────────────────────────

fn has_codex_apps_cache() -> bool {
    let cache_dir = match crate::config::home_dir() {
        Some(h) => h.join(".codex/cache/codex_apps_tools"),
        None => return false,
    };
    if !cache_dir.is_dir() {
        return false;
    }
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path)
                && content.to_lowercase().contains("keenable")
            {
                return true;
            }
        }
    }
    false
}

fn clean_codex_apps_cache() {
    let cache_dir = match crate::config::home_dir() {
        Some(h) => h.join(".codex/cache/codex_apps_tools"),
        None => return,
    };
    if !cache_dir.is_dir() {
        return;
    }

    let mut removed = 0u32;
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path)
                && content.to_lowercase().contains("keenable")
                && fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }

    if removed > 0 {
        ui::sub_success(&format!(
            "Removed {} Codex Apps cached tool file(s)",
            removed
        ));
    } else {
        ui::sub_done("No Codex Apps cached tools to remove");
    }
}
