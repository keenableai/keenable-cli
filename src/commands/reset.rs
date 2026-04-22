use colored::Colorize;
use dialoguer::Select;
use serde_json::json;
use std::fs;

use crate::ui;

use super::ide::*;

// ── Per-client reset ────────────────────────────────────────────────

fn reset_ide(ide: &IDEDef) {
    let mut config = read_config(&ide.config_path);
    let mut config_changed = false;

    // Step 1: Remove the "keenable" MCP entry
    if let Some(servers) = config.get_mut(ide.servers_key).and_then(|v| v.as_object_mut()) {
        if servers.remove("keenable").is_some() {
            config_changed = true;
            ui::sub_success("Removed Keenable MCP entry");
        } else {
            ui::sub_done("No Keenable MCP entry to remove");
        }
    } else {
        ui::sub_done("No Keenable MCP entry to remove");
    }

    // Step 2: Also remove any other entries pointing at keenable.ai
    let servers = config
        .get(ide.servers_key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut keenable_other_entries: Vec<String> = Vec::new();
    for (name, entry) in &servers {
        if let Some(url) = extract_url(entry) {
            if is_keenable_url(&url) {
                keenable_other_entries.push(name.clone());
            }
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
            "Removed additional Keenable entries: {}",
            keenable_other_entries.join(", ")
        ));
    }

    // Step 3: Re-enable standard tools (Claude Code, OpenCode)
    if ide.has_standard_tools {
        if ide.flag == "opencode" {
            restore_opencode_standard_tools(&mut config, &mut config_changed);
        } else {
            restore_standard_tools(&mut config, &mut config_changed);
        }
    }

    // Step 4: Clean up Codex Apps cached tools (Codex only)
    if ide.flag == "codex" {
        clean_codex_apps_cache();
    }

    if config_changed {
        write_config(&ide.config_path, &config);
    }
}

/// Check if any Codex Apps cached tool files reference Keenable.
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

/// Remove cached Codex Apps tool files that reference Keenable.
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
        ui::sub_success(&format!("Removed {} Codex Apps cached tool file(s)", removed));
    } else {
        ui::sub_done("No Codex Apps cached tools to remove");
    }
}

fn restore_standard_tools(config: &mut serde_json::Value, changed: &mut bool) {
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

    let had_tools = CLAUDE_CODE_STANDARD_TOOLS
        .iter()
        .any(|tool| {
            config
                .pointer("/permissions/deny")
                .and_then(|v| v.as_array())
                .map_or(false, |arr| arr.iter().any(|v| v.as_str() == Some(tool)))
        });

    if had_tools {
        if filtered.is_empty() {
            // Remove the deny key entirely if empty
            if let Some(perms) = config.get_mut("permissions").and_then(|v| v.as_object_mut()) {
                perms.remove("deny");
                // Remove permissions object if now empty
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

fn restore_opencode_standard_tools(config: &mut serde_json::Value, changed: &mut bool) {
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

// ── Interactive confirmation ────────────────────────────────────────

fn confirm_reset(ide_names: &[&str]) -> bool {
    eprintln!();
    let target = if ide_names.len() == 1 {
        ide_names[0].to_string()
    } else {
        format!("{} clients", ide_names.len())
    };
    eprintln!(
        "   This will remove Keenable MCP from {} and re-enable\n   built-in search tools where applicable.",
        target.bold()
    );
    eprintln!();

    let choices = &[
        "Proceed",
        "Cancel",
    ];

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

// ── Main reset flow ─────────────────────────────────────────────────

pub fn reset(selected_flags: Vec<String>) {
    ui::header("keenable reset");

    let all = all_ides();
    let detected: Vec<&IDEDef> = all.iter().filter(|ide| is_detected(ide)).collect();

    // Find which detected clients have Keenable configured
    // (MCP entry in config OR Codex Apps cached tools)
    let configured: Vec<&IDEDef> = detected
        .iter()
        .filter(|ide| has_keenable_entry(ide) || (ide.flag == "codex" && has_codex_apps_cache()))
        .copied()
        .collect();

    if selected_flags.is_empty() {
        // ── Status mode: show which clients have Keenable configured ─
        show_reset_status(&configured);
    } else {
        // ── Reset mode: remove Keenable from selected clients ────────
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

        // Warn about flags that don't match
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
                        ui::warning(&format!("{} doesn't have Keenable configured", ide_name));
                    }
                }
            }
        }

        if targets.is_empty() {
            ui::info("No clients with Keenable configuration found to reset.");
            eprintln!();
            return;
        }

        let target_names: Vec<&str> = targets.iter().map(|ide| ide.name).collect();

        ui::save_cursor();
        if !confirm_reset(&target_names) {
            eprintln!();
            return;
        }
        ui::restore_and_clear();

        // Show current state before resetting
        ui::label("Your Clients");
        for ide in &targets {
            eprintln!("   {} {}", "✓".green(), ide.name.green());
        }

        for ide in &targets {
            ui::label(ide.name);
            reset_ide(ide);
        }

        eprintln!();
        ui::success("Reset complete");
    }

    eprintln!();
}

fn show_reset_status(configured: &[&IDEDef]) {
    ui::label("Your Clients");

    if configured.is_empty() {
        ui::info("No clients have Keenable configured. Nothing to reset.");
        return;
    }

    for ide in configured {
        eprintln!("   {} {}", "✓".green(), ide.name.green());
        eprintln!("      {}", format!("- Run keenable reset --{}", ide.flag).dimmed());
    }

    ui::hint(&format!(
        "Run {} to reset all at once",
        "keenable reset --all".cyan()
    ));
}
