//! Shared configure/reset logic for MCP products (Keenable, WebQL).

use colored::Colorize;
use dialoguer::Select;
use serde_json::{json, Value};
use std::fs;

use crate::api::{api_key_client, api_url};
use crate::config;
use crate::ui;

use super::ide::*;

// ── Product definition ──────────────────────────────────────────────

/// Describes an MCP product that can be configured/reset in IDE clients.
pub struct McpProduct {
    /// Entry name in IDE config (e.g. "keenable", "keenable-webql").
    pub entry_name: &'static str,
    /// Human-readable name (e.g. "Keenable MCP", "WebQL MCP").
    pub display_name: &'static str,
    /// CLI command for configure (e.g. "configure-mcp", "configure-webql").
    pub configure_cmd: &'static str,
    /// CLI command for reset (e.g. "reset", "reset-webql").
    pub reset_cmd: &'static str,
    /// Build the MCP entry JSON for this product.
    pub build_entry: fn(&IDEDef, &str) -> Value,
    /// Extract the API key/token from an existing entry.
    pub extract_key: fn(&Value) -> Option<String>,
    /// Check if a URL belongs to this product.
    pub is_product_url: fn(&str) -> bool,
    /// Whether to check for conflicting search MCPs.
    pub check_conflicts: bool,
    /// Whether to disable/restore standard tools (WebSearch, WebFetch).
    pub manage_standard_tools: bool,
    /// Whether to check for legacy npx mcp-remote entries.
    pub check_legacy_npx: bool,
    /// Whether to clean Codex Apps cache on reset.
    pub clean_codex_cache: bool,
    /// Show client-specific recommendations after configure.
    pub show_recommendations: fn(&IDEDef),
}

pub fn keenable_product() -> McpProduct {
    McpProduct {
        entry_name: "keenable",
        display_name: "Keenable MCP",
        configure_cmd: "configure-mcp",
        reset_cmd: "reset",
        build_entry: build_keenable_entry,
        extract_key: extract_entry_api_key,
        is_product_url: is_keenable_url,
        check_conflicts: true,
        manage_standard_tools: true,
        check_legacy_npx: true,
        clean_codex_cache: true,
        show_recommendations: |ide| {
            if ide.name == "Claude Desktop" {
                ui::sub_hint("Disable built-in web search manually (+ button near the chat text field)");
            }
            if ide.name == "Cursor" {
                ui::sub_hint("We recommend disabling standard search & fetch tools in Cursor Settings → Tools");
                ui::sub_hint("We recommend setting a custom rule to use Keenable search");
            }
        },
    }
}

pub fn webql_product() -> McpProduct {
    McpProduct {
        entry_name: "keenable-webql",
        display_name: "WebQL MCP",
        configure_cmd: "configure-webql",
        reset_cmd: "reset-webql",
        build_entry: build_webql_entry,
        extract_key: extract_webql_token,
        is_product_url: is_webql_url,
        check_conflicts: false,
        manage_standard_tools: false,
        check_legacy_npx: false,
        clean_codex_cache: false,
        show_recommendations: |_| {},
    }
}

// ── Product status ──────────────────────────────────────────────────

pub struct ProductStatus {
    pub has_entry: bool,
    pub wrong_api_key: bool,
    pub uses_legacy_npx: bool,
    pub standard_tools_disabled: bool,
    pub duplicate_entries: Vec<String>,
    pub conflicting_mcps: Vec<String>,
}

pub fn get_product_status(product: &McpProduct, ide: &IDEDef, api_key: &str) -> ProductStatus {
    let config = read_config(&ide.config_path);
    let existing = config
        .pointer(&format!("/{}/{}", ide.servers_key, product.entry_name))
        .cloned();

    let has_entry = existing.is_some();

    let wrong_api_key = if let Some(ref entry) = existing {
        let existing_key = (product.extract_key)(entry);
        let desired_key = Some(api_key.to_string());
        existing_key.is_some() && existing_key != desired_key
    } else {
        false
    };

    let uses_legacy_npx = if product.check_legacy_npx {
        existing
            .as_ref()
            .and_then(|e| e["command"].as_str())
            .map_or(false, |cmd| cmd == "npx")
    } else {
        false
    };

    let standard_tools_disabled = if product.manage_standard_tools && ide.has_standard_tools {
        if ide.flag == "opencode" {
            OPENCODE_STANDARD_TOOLS.iter().all(|tool| {
                config
                    .pointer(&format!("/permission/{}", tool))
                    .and_then(|v| v.as_str())
                    == Some("deny")
            })
        } else {
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
        }
    } else {
        true // not applicable = no issue
    };

    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut duplicate_entries = Vec::new();
    let mut conflicting_mcps = Vec::new();

    for (name, entry) in &servers {
        if name == product.entry_name {
            continue;
        }
        if let Some(url) = extract_url(entry) {
            if (product.is_product_url)(&url) {
                duplicate_entries.push(name.clone());
                continue;
            }
        }
        if product.check_conflicts && is_conflicting_name(name) {
            conflicting_mcps.push(name.clone());
        }
    }

    ProductStatus {
        has_entry,
        wrong_api_key,
        uses_legacy_npx,
        standard_tools_disabled,
        duplicate_entries,
        conflicting_mcps,
    }
}

// ── Configure flow ──────────────────────────────────────────────────

pub async fn configure(product: &McpProduct, selected_flags: Vec<String>) {
    ui::header(&format!("keenable {}", product.configure_cmd));

    // Pre-flight: validate API key
    let api_key_result = match config::get_api_key() {
        Some(key) => {
            if validate_api_key(&key).await {
                Ok(key)
            } else {
                Err("API key is invalid or expired")
            }
        }
        None => Err("No API key configured"),
    };

    ui::label("Keenable CLI");
    match api_key_result {
        Ok(ref _key) => {
            ui::success("API key configured");
        }
        Err(msg) => {
            ui::error(msg);
            ui::sub_info(&format!(
                "Run {} or {}",
                "keenable login".cyan(),
                "keenable login --api-key <KEY>".cyan()
            ));
            eprintln!();
            std::process::exit(1);
        }
    }
    let api_key = api_key_result.unwrap();

    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();
    let not_detected: Vec<&IDEDef> = all.iter().filter(|ide| !is_detected(ide)).collect();

    if selected_flags.is_empty() {
        show_configure_status(product, &detected, &not_detected, &api_key);
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

        if !is_all {
            warn_unmatched_flags(&selected_flags, &all, &detected);
        }

        if targets.is_empty() {
            ui::error("No matching clients found to configure");
            ui::hint(&format!(
                "Run `keenable {}` to see available clients",
                product.configure_cmd
            ));
            eprintln!();
            std::process::exit(1);
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        if !confirm_configure(product, &target_names) {
            eprintln!();
            return;
        }

        for ide in &targets {
            ui::label(ide.name);
            configure_ide(product, ide, &api_key);
            (product.show_recommendations)(ide);
        }

        eprintln!();
        ui::success("Configuration complete");
    }

    eprintln!();
}

fn configure_ide(product: &McpProduct, ide: &IDEDef, api_key: &str) {
    let mut config = read_config(&ide.config_path);
    let mut config_changed = false;

    // Step 1: Remove duplicate entries (other names pointing at this product's URL)
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut duplicate_entries: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    for (name, entry) in &servers {
        if name == product.entry_name {
            continue;
        }
        if let Some(url) = extract_url(entry) {
            if (product.is_product_url)(&url) {
                duplicate_entries.push(name.clone());
                continue;
            }
        }
        if product.check_conflicts && is_conflicting_name(name) {
            conflicts.push(name.clone());
        }
    }

    if !duplicate_entries.is_empty() {
        for name in &duplicate_entries {
            if let Some(obj) = config.get_mut(ide.servers_key).and_then(|v| v.as_object_mut()) {
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

    // Step 2: Add/update product MCP entry
    let desired = (product.build_entry)(ide, api_key);
    let existing = config
        .pointer(&format!("/{}/{}", ide.servers_key, product.entry_name))
        .cloned();

    match existing {
        Some(ref entry) if *entry == desired => {
            ui::sub_done(&format!("{} already configured", product.display_name));
        }
        Some(ref entry) => {
            let existing_key = (product.extract_key)(entry);
            let desired_key = Some(api_key.to_string());
            if existing_key != desired_key && existing_key.is_some() {
                ui::sub_warning(&format!(
                    "Updating API key in {} entry",
                    product.display_name
                ));
            }
            if product.check_legacy_npx && entry["command"].as_str() == Some("npx") {
                ui::sub_warning("Replacing npx mcp-remote with built-in stdio bridge (no Node.js needed)");
            }
            config[ide.servers_key][product.entry_name] = desired;
            config_changed = true;
            ui::sub_success(&format!("{} updated", product.display_name));
        }
        None => {
            if config.get(ide.servers_key).is_none() {
                config[ide.servers_key] = json!({});
            }
            config[ide.servers_key][product.entry_name] = desired;
            config_changed = true;
            ui::sub_success(&format!("{} added", product.display_name));
        }
    }

    // Step 3: Disable standard tools (only for products that manage them)
    if product.manage_standard_tools && ide.has_standard_tools {
        if ide.flag == "opencode" {
            disable_opencode_standard_tools(&mut config, &mut config_changed);
        } else {
            disable_standard_tools(&mut config, &mut config_changed);
        }
    }

    if config_changed {
        write_config(&ide.config_path, &config);
    }
}

// ── Reset flow ──────────────────────────────────────────────────────

pub fn reset(product: &McpProduct, selected_flags: Vec<String>) {
    ui::header(&format!("keenable {}", product.reset_cmd));

    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();

    let configured: Vec<&IDEDef> = detected
        .iter()
        .filter(|ide| {
            has_product_entry(product, ide)
                || (product.clean_codex_cache && ide.flag == "codex" && has_codex_apps_cache())
        })
        .copied()
        .collect();

    if selected_flags.is_empty() {
        show_reset_status(product, &configured);
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

        if !is_all {
            for flag in &selected_flags {
                let matched = all.iter().any(|ide| ide.flag == flag.as_str());
                let configured_match = configured.iter().any(|ide| ide.flag == flag.as_str());
                if !matched {
                    ui::warning(&format!("Unknown client: --{}", flag));
                } else if !configured_match {
                    let ide_name = all.iter().find(|ide| ide.flag == flag.as_str()).unwrap().name;
                    let is_installed = detected.iter().any(|ide| ide.flag == flag.as_str());
                    if !is_installed {
                        ui::warning(&format!("{} is not installed", ide_name));
                    } else {
                        ui::warning(&format!(
                            "{} doesn't have {} configured",
                            ide_name, product.display_name
                        ));
                    }
                }
            }
        }

        if targets.is_empty() {
            ui::info(&format!(
                "No clients with {} configuration found to reset.",
                product.display_name
            ));
            eprintln!();
            return;
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        ui::save_cursor();
        if !confirm_reset(product, &target_names) {
            eprintln!();
            return;
        }
        ui::restore_and_clear();

        ui::label("Your Clients");
        for ide in &targets {
            eprintln!("   {} {}", "✓".green(), ide.name.green());
        }

        for ide in &targets {
            ui::label(ide.name);
            reset_ide(product, ide);
        }

        eprintln!();
        ui::success("Reset complete");
    }

    eprintln!();
}

fn reset_ide(product: &McpProduct, ide: &IDEDef) {
    let mut config = read_config(&ide.config_path);
    let mut config_changed = false;

    // Step 1: Remove the product's MCP entry
    if let Some(servers) = config.get_mut(ide.servers_key).and_then(|v| v.as_object_mut()) {
        if servers.remove(product.entry_name).is_some() {
            config_changed = true;
            ui::sub_success(&format!("Removed {} entry", product.display_name));
        } else {
            ui::sub_done(&format!("No {} entry to remove", product.display_name));
        }
    } else {
        ui::sub_done(&format!("No {} entry to remove", product.display_name));
    }

    // Step 2: Remove any other entries pointing at this product's URL
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut other_entries: Vec<String> = Vec::new();
    for (name, entry) in &servers {
        if let Some(url) = extract_url(entry) {
            if (product.is_product_url)(&url) {
                other_entries.push(name.clone());
            }
        }
    }

    if !other_entries.is_empty() {
        for name in &other_entries {
            if let Some(obj) = config.get_mut(ide.servers_key).and_then(|v| v.as_object_mut()) {
                obj.remove(name.as_str());
            }
        }
        config_changed = true;
        ui::sub_success(&format!(
            "Removed additional {} entries: {}",
            product.display_name,
            other_entries.join(", ")
        ));
    }

    // Step 3: Restore standard tools (only for products that manage them)
    if product.manage_standard_tools && ide.has_standard_tools {
        if ide.flag == "opencode" {
            restore_opencode_standard_tools(&mut config, &mut config_changed);
        } else {
            restore_standard_tools(&mut config, &mut config_changed);
        }
    }

    // Step 4: Clean Codex Apps cache if applicable
    if product.clean_codex_cache && ide.flag == "codex" {
        clean_codex_apps_cache();
    }

    if config_changed {
        write_config(&ide.config_path, &config);
    }
}

// ── Status display ──────────────────────────────────────────────────

fn show_configure_status(
    product: &McpProduct,
    detected: &[&IDEDef],
    not_detected: &[&IDEDef],
    api_key: &str,
) {
    ui::label("Your Clients");

    if detected.is_empty() {
        ui::info("No supported clients detected.");
        return;
    }

    let mut any_unconfigured = false;

    for ide in detected {
        let status = get_product_status(product, ide, api_key);

        let has_issues = status.wrong_api_key
            || status.uses_legacy_npx
            || !status.duplicate_entries.is_empty()
            || !status.conflicting_mcps.is_empty()
            || (product.manage_standard_tools
                && ide.has_standard_tools
                && !status.standard_tools_disabled);

        if !status.has_entry {
            any_unconfigured = true;
            eprintln!("   {} {}", "✗".red(), ide.name);
            eprintln!(
                "      {}",
                format!("- Run keenable {} --{}", product.configure_cmd, ide.flag).dimmed()
            );
        } else if has_issues {
            any_unconfigured = true;
            eprintln!("   {} {}", "⚠".yellow(), ide.name.yellow());
            show_status_issues(product, ide, &status);
            (product.show_recommendations)(ide);
        } else {
            eprintln!("   {} {}", "✓".green(), ide.name.green());
            (product.show_recommendations)(ide);
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
            format!("keenable {} --all", product.configure_cmd).cyan()
        ));
    }
}

fn show_status_issues(product: &McpProduct, ide: &IDEDef, status: &ProductStatus) {
    if status.uses_legacy_npx {
        ui::sub_warning(&format!(
            "Uses npx mcp-remote (requires Node.js). Re-run {} to switch to built-in bridge",
            product.configure_cmd
        ));
    }
    if status.wrong_api_key {
        ui::sub_warning("Different API key configured");
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
    if product.manage_standard_tools && ide.has_standard_tools && !status.standard_tools_disabled {
        let tools = if ide.flag == "opencode" {
            OPENCODE_STANDARD_TOOLS.join(", ")
        } else {
            CLAUDE_CODE_STANDARD_TOOLS.join(", ")
        };
        ui::sub_warning(&format!("Standard tools ({}) not disabled", tools));
    }
}

fn show_reset_status(product: &McpProduct, configured: &[&IDEDef]) {
    ui::label("Your Clients");

    if configured.is_empty() {
        ui::info(&format!(
            "No clients have {} configured. Nothing to reset.",
            product.display_name
        ));
        return;
    }

    for ide in configured {
        eprintln!("   {} {}", "✓".green(), ide.name.green());
        eprintln!(
            "      {}",
            format!("- Run keenable {} --{}", product.reset_cmd, ide.flag).dimmed()
        );
    }

    ui::hint(&format!(
        "Run {} to reset all at once",
        format!("keenable {} --all", product.reset_cmd).cyan()
    ));
}

// ── Helpers ─────────────────────────────────────────────────────────

fn has_product_entry(product: &McpProduct, ide: &IDEDef) -> bool {
    let config = read_config(&ide.config_path);
    config
        .pointer(&format!("/{}/{}", ide.servers_key, product.entry_name))
        .is_some()
}

fn warn_unmatched_flags(selected_flags: &[String], all: &[IDEDef], detected: &[&IDEDef]) {
    for flag in selected_flags {
        let matched = all.iter().any(|ide| ide.flag == flag.as_str());
        let detected_match = detected.iter().any(|ide| ide.flag == flag.as_str());
        if !matched {
            ui::warning(&format!("Unknown client: --{}", flag));
        } else if !detected_match {
            let ide_name = all.iter().find(|ide| ide.flag == flag.as_str()).unwrap().name;
            ui::warning(&format!("{} is not installed", ide_name));
        }
    }
}

async fn validate_api_key(api_key: &str) -> bool {
    let client = api_key_client(api_key);
    match client.get(api_url("/v1/auth/user")).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn confirm_configure(product: &McpProduct, ide_names: &[&str]) -> bool {
    if config::get_skip_setup_confirmation() {
        return true;
    }

    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };

    if product.manage_standard_tools {
        eprintln!(
            "   This will add {} to {} and disable\n   built-in search tools where applicable.",
            product.display_name,
            target.bold()
        );
    } else {
        eprintln!(
            "   This will add {} to {}.",
            product.display_name,
            target.bold()
        );
    }
    eprintln!();

    let choices = &["Proceed", "Proceed and don't ask again", "Cancel"];

    let selection = Select::new()
        .items(choices)
        .default(0)
        .interact_opt();

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

fn confirm_reset(product: &McpProduct, ide_names: &[&str]) -> bool {
    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };

    if product.manage_standard_tools {
        eprintln!(
            "   This will remove {} from {} and re-enable\n   built-in search tools where applicable.",
            product.display_name,
            target.bold()
        );
    } else {
        eprintln!(
            "   This will remove {} from {}.",
            product.display_name,
            target.bold()
        );
    }
    eprintln!();

    let choices = &["Proceed", "Cancel"];

    let selection = Select::new()
        .items(choices)
        .default(0)
        .interact_opt();

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

fn disable_standard_tools(config: &mut Value, changed: &mut bool) {
    let deny_list = config
        .pointer("/permissions/deny")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let missing: Vec<&&str> = CLAUDE_CODE_STANDARD_TOOLS
        .iter()
        .filter(|tool| !deny_list.iter().any(|d| d == **tool))
        .collect();

    if missing.is_empty() {
        ui::sub_done(&format!(
            "Standard tools already disabled: {}",
            CLAUDE_CODE_STANDARD_TOOLS.join(", ")
        ));
    } else {
        let missing_names: Vec<&str> = missing.iter().map(|s| **s).collect();
        let mut new_deny: Vec<String> = deny_list;
        for tool in &missing_names {
            new_deny.push(tool.to_string());
        }
        if config.pointer("/permissions").is_none() {
            config["permissions"] = json!({});
        }
        config["permissions"]["deny"] = json!(new_deny);
        *changed = true;
        ui::sub_success(&format!(
            "Disabled standard tools: {}",
            missing_names.join(", ")
        ));
    }
}

fn disable_opencode_standard_tools(config: &mut Value, changed: &mut bool) {
    let already_denied: Vec<&&str> = OPENCODE_STANDARD_TOOLS
        .iter()
        .filter(|tool| {
            config
                .pointer(&format!("/permission/{}", tool))
                .and_then(|v| v.as_str())
                == Some("deny")
        })
        .collect();

    if already_denied.len() == OPENCODE_STANDARD_TOOLS.len() {
        ui::sub_done(&format!(
            "Standard tools already disabled: {}",
            OPENCODE_STANDARD_TOOLS.join(", ")
        ));
    } else {
        let mut missing = Vec::new();
        if config.pointer("/permission").is_none() {
            config["permission"] = json!({});
        }
        for tool in OPENCODE_STANDARD_TOOLS {
            let current = config
                .pointer(&format!("/permission/{}", tool))
                .and_then(|v| v.as_str());
            if current != Some("deny") {
                config["permission"][*tool] = json!("deny");
                missing.push(*tool);
            }
        }
        *changed = true;
        ui::sub_success(&format!(
            "Disabled standard tools: {}",
            missing.join(", ")
        ));
    }
}

fn restore_standard_tools(config: &mut Value, changed: &mut bool) {
    let deny_list = config
        .pointer("/permissions/deny")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let filtered: Vec<String> = deny_list
        .into_iter()
        .filter(|d| !CLAUDE_CODE_STANDARD_TOOLS.contains(&d.as_str()))
        .collect();

    let had_tools = CLAUDE_CODE_STANDARD_TOOLS.iter().any(|tool| {
        config
            .pointer("/permissions/deny")
            .and_then(|v| v.as_array())
            .map_or(false, |arr| arr.iter().any(|v| v.as_str() == Some(tool)))
    });

    if had_tools {
        if filtered.is_empty() {
            if let Some(perms) = config.get_mut("permissions").and_then(|v| v.as_object_mut()) {
                perms.remove("deny");
                if perms.is_empty() {
                    if let Some(obj) = config.as_object_mut() {
                        obj.remove("permissions");
                    }
                }
            }
        } else {
            config["permissions"]["deny"] = json!(filtered);
        }
        *changed = true;
        ui::sub_success(&format!(
            "Re-enabled standard tools: {}",
            CLAUDE_CODE_STANDARD_TOOLS.join(", ")
        ));
    } else {
        ui::sub_done("Standard tools were not disabled");
    }
}

fn restore_opencode_standard_tools(config: &mut Value, changed: &mut bool) {
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
                if perms.is_empty() {
                    if let Some(obj) = config.as_object_mut() {
                        obj.remove("permission");
                    }
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

// ── Codex Apps cache ────────────────────────────────────────────────

fn has_codex_apps_cache() -> bool {
    let cache_dir = match dirs::home_dir() {
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
            if let Ok(content) = fs::read_to_string(&path) {
                if content.to_lowercase().contains("keenable") {
                    return true;
                }
            }
        }
    }
    false
}

fn clean_codex_apps_cache() {
    let cache_dir = match dirs::home_dir() {
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
            if let Ok(content) = fs::read_to_string(&path) {
                if content.to_lowercase().contains("keenable") {
                    if fs::remove_file(&path).is_ok() {
                        removed += 1;
                    }
                }
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
