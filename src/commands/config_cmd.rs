use colored::Colorize;

use crate::config;
use crate::constants::SEARCH_MODES;
use crate::ui;

/// Known config keys and their allowed values.
const KNOWN_KEYS: &[(&str, &[&str])] = &[
    ("default_search_mode", SEARCH_MODES),
    ("forced_search_mode", SEARCH_MODES),
];

fn find_key(key: &str) -> Option<&'static (&'static str, &'static [&'static str])> {
    KNOWN_KEYS.iter().find(|(k, _)| *k == key)
}

pub fn config_view() {
    ui::header("keenable config");

    let cfg = config::get_config();
    let mut found = false;
    for (key, allowed) in KNOWN_KEYS {
        if let Some(val) = cfg[key].as_str() {
            eprintln!("   {} = {}", key.bold(), val.green());
            found = true;
        } else {
            eprintln!("   {} {}", key.dimmed(), format!("(not set, allowed: {})", allowed.join(", ")).dimmed());
        }
    }
    if !found {
        eprintln!();
        ui::hint("Set a value with: keenable config set <key> <value>");
    }
    eprintln!();
}

pub fn config_get(key: &str) {
    if find_key(key).is_none() {
        ui::error(&format!("Unknown config key \"{}\". Known keys: {}", key,
            KNOWN_KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")));
        eprintln!();
        std::process::exit(1);
    }

    let cfg = config::get_config();
    if let Some(val) = cfg[key].as_str() {
        println!("{}", val);
    }
}

pub fn config_set(key: &str, value: &str) {
    let entry = match find_key(key) {
        Some(e) => e,
        None => {
            ui::error(&format!("Unknown config key \"{}\". Known keys: {}", key,
                KNOWN_KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")));
            eprintln!();
            std::process::exit(1);
        }
    };

    let (_, allowed) = entry;
    if !allowed.contains(&value) {
        ui::error(&format!("Invalid value \"{}\" for {}. Allowed: {}", value, key, allowed.join(", ")));
        eprintln!();
        std::process::exit(1);
    }

    config::set_config_value(key, serde_json::Value::String(value.to_string()));
    ui::success(&format!("{} = {}", key, value));
    eprintln!();
}

pub fn config_unset(key: &str) {
    if find_key(key).is_none() {
        ui::error(&format!("Unknown config key \"{}\". Known keys: {}", key,
            KNOWN_KEYS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")));
        eprintln!();
        std::process::exit(1);
    }

    config::remove_config_value(key);
    ui::success(&format!("{} unset", key));
    eprintln!();
}
