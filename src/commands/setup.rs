use colored::Colorize;
use dialoguer::Select;
use serde_json::json;

use crate::api::{api_key_client, api_url};
use crate::config;
use crate::ui;

use super::ide::*;

// ── Per-client configuration ────────────────────────────────────────

fn configure_ide(ide: &IDEDef, api_key: &str) {
    let mut config = read_config(&ide.config_path);
    let mut config_changed = false;

    // Step 1: Remove duplicate Keenable entries (other names pointing at keenable.ai)
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut keenable_other_entries: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    for (name, entry) in &servers {
        if name == "keenable" {
            continue;
        }

        if let Some(url) = extract_url(entry) {
            if is_keenable_url(&url) {
                keenable_other_entries.push(name.clone());
                continue;
            }
        }

        if is_conflicting_name(name) {
            conflicts.push(name.clone());
        }
    }

    if !keenable_other_entries.is_empty() {
        for name in &keenable_other_entries {
            if let Some(obj) = config.get_mut(ide.servers_key).and_then(|v| v.as_object_mut()) {
                obj.remove(name.as_str());
            }
        }
        config_changed = true;
        ui::sub_success(&format!(
            "Removed duplicate entries: {}",
            keenable_other_entries.join(", ")
        ));
    }

    if !conflicts.is_empty() {
        ui::sub_warning(&format!(
            "Other search MCPs found: {}",
            conflicts.join(", ")
        ));
    }

    // Step 2: Add/update Keenable MCP entry
    let desired = build_keenable_entry(ide, api_key);
    let existing = config
        .pointer(&format!("/{}/keenable", ide.servers_key))
        .cloned();

    match existing {
        Some(ref entry) if *entry == desired => {
            ui::sub_done("Keenable MCP already configured");
        }
        Some(ref entry) => {
            let existing_key = extract_entry_api_key(entry);
            let desired_key = Some(api_key.to_string());
            if existing_key != desired_key && existing_key.is_some() {
                ui::sub_warning("Updating API key in Keenable MCP entry");
            }
            config[ide.servers_key]["keenable"] = desired;
            config_changed = true;
            ui::sub_success("Keenable MCP updated");
        }
        None => {
            if config.get(ide.servers_key).is_none() {
                config[ide.servers_key] = json!({});
            }
            config[ide.servers_key]["keenable"] = desired;
            config_changed = true;
            ui::sub_success("Keenable MCP added");
        }
    }

    // Step 3: Disable standard tools (Claude Code only)
    if ide.has_standard_tools {
        disable_standard_tools(&mut config, &mut config_changed);
    }

    if config_changed {
        write_config(&ide.config_path, &config);
    }
}

fn disable_standard_tools(config: &mut serde_json::Value, changed: &mut bool) {
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

// ── Interactive confirmation ────────────────────────────────────────

fn confirm_setup(ide_names: &[&str]) -> bool {
    if config::get_skip_setup_confirmation() {
        return true;
    }

    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };
    eprintln!(
        "   This will add Keenable MCP to {} and disable\n   built-in search tools where applicable.",
        target.bold()
    );
    eprintln!();

    let choices = &[
        "Proceed",
        "Proceed and don't ask again",
        "Cancel",
    ];

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
            ui::info("Setup cancelled.");
            false
        }
    }
}

// ── Validate API key ────────────────────────────────────────────────

async fn validate_api_key(api_key: &str) -> bool {
    let client = api_key_client(api_key);
    match client
        .get(api_url("/v1/search"))
        .query(&[("query", "test"), ("count", "1")])
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

// ── Client-specific recommendations ────────────────────────────────

fn show_client_recommendations(ide: &IDEDef) {
    if ide.name == "Claude Desktop" {
        ui::sub_hint("Disable built-in web search manually (+ button near the chat text field)");
    }
    if ide.name == "Cursor" {
        ui::sub_hint("We recommend disabling standard search & fetch tools in Cursor Settings → Tools");
        ui::sub_hint("We recommend setting a custom rule to use Keenable search");
    }
}

// ── Main setup flow ─────────────────────────────────────────────────

pub async fn setup(selected_flags: Vec<String>) {
    ui::header("keenable setup");

    // ── Pre-flight: validate API key before showing anything ──────
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

    // Show Keenable CLI section (label + result together, no flicker)
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
                "keenable configure --api-key <KEY>".cyan()
            ));
            eprintln!();
            std::process::exit(1);
        }
    }
    let api_key = api_key_result.unwrap();

    // ── Clients section ─────────────────────────────────────────────
    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();
    let not_detected: Vec<&IDEDef> = all.iter().filter(|ide| !is_detected(ide)).collect();

    if selected_flags.is_empty() {
        // ── Status mode: show what's installed and configured ────────
        show_status(&detected, &not_detected, &api_key);
    } else {
        // ── Configure mode: set up selected clients ─────────────────
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

        // Warn about flags that don't match any detected client
        if !is_all {
            for flag in &selected_flags {
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

        if targets.is_empty() {
            ui::error("No matching clients found to configure");
            ui::hint("Run `keenable setup` to see available clients");
            eprintln!();
            std::process::exit(1);
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        if !confirm_setup(&target_names) {
            eprintln!();
            return;
        }

        for ide in &targets {
            ui::label(ide.name);
            configure_ide(ide, &api_key);
            show_client_recommendations(ide);
        }

        eprintln!();
        ui::success("Setup complete");
    }

    eprintln!();
}

fn show_status(detected: &[&IDEDef], not_detected: &[&IDEDef], api_key: &str) {
    ui::label("Your Clients");

    if detected.is_empty() {
        ui::info("No supported clients detected.");
        return;
    }

    let mut any_unconfigured = false;

    for ide in detected {
        let status = get_ide_status(ide, api_key);

        let has_issues = status.wrong_api_key
            || !status.duplicate_entries.is_empty()
            || !status.conflicting_mcps.is_empty()
            || (ide.has_standard_tools && !status.standard_tools_disabled);

        if !status.has_entry {
            // Not configured
            any_unconfigured = true;
            eprintln!("   {} {}", "✗".red(), ide.name);
            eprintln!("      {}", format!("- Run keenable setup --{}", ide.flag).dimmed());
        } else if has_issues {
            // Configured with issues
            any_unconfigured = true;
            eprintln!("   {} {}", "⚠".yellow(), ide.name.yellow());
            show_status_issues(ide, &status);
            show_client_recommendations(ide);
        } else {
            // Fully configured
            eprintln!("   {} {}", "✓".green(), ide.name.green());
            show_client_recommendations(ide);
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
            "keenable setup --all".cyan()
        ));
    }
}

fn show_status_issues(_ide: &IDEDef, status: &IdeStatus) {
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
    if _ide.has_standard_tools && !status.standard_tools_disabled {
        ui::sub_warning("Standard tools (WebSearch, WebFetch) not disabled");
    }
}
