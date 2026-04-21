use colored::Colorize;
use serde_json::json;

use crate::api::{api_url, bearer_client, handle_response};
use crate::config;
use crate::ui;

fn require_token() -> String {
    match config::get_access_token() {
        Some(token) => token,
        None => {
            ui::error(&format!("Not logged in. Run {} first.", "keenable login".cyan()));
            eprintln!();
            std::process::exit(1);
        }
    }
}

fn require_org_id() -> String {
    match config::get_org_id() {
        Some(id) => id,
        None => {
            ui::error(&format!("No organization found. Run {} first.", "keenable login".cyan()));
            eprintln!();
            std::process::exit(1);
        }
    }
}

pub async fn create(label: &str, save: bool) {
    let token = require_token();
    let org_id = require_org_id();
    let client = bearer_client(&token);

    ui::header("keenable keys create");

    let resp = client
        .post(api_url("/bff/v1/auth/api_keys"))
        .query(&[("org_id", &org_id)])
        .json(&json!({"label": label}))
        .send()
        .await;

    match resp {
        Ok(r) => match handle_response(r).await {
            Ok(data) => {
                let key = data["api_key"].as_str().unwrap_or("unknown");

                ui::success("API key created");
                ui::info(&format!("Key: {}", key.yellow().bold()));
                ui::hint("This key will only be shown once.");

                if save {
                    config::set_api_key(key);
                    ui::info("Key saved to local config.");
                }
                eprintln!();
            }
            Err(e) => {
                ui::error(&e);
                eprintln!();
                std::process::exit(1);
            }
        },
        Err(e) => {
            ui::error(&e.to_string());
            eprintln!();
            std::process::exit(1);
        }
    }
}
