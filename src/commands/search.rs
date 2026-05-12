use colored::Colorize;
use serde_json::{json, Value};

use crate::api::{api_key_client, api_url, handle_response};
use crate::config;
use crate::daemon::{self, DaemonRequest};
use crate::ui;

fn resolve_api_key(override_key: Option<&str>) -> String {
    if let Some(key) = override_key {
        return key.to_string();
    }
    match config::get_api_key() {
        Some(key) => key,
        None => {
            ui::error(&format!(
                "No API key found. Run {} or {} first.",
                "keenable login".cyan(),
                "keenable configure --api-key <KEY>".cyan()
            ));
            eprintln!();
            std::process::exit(1);
        }
    }
}

fn print_yaml(data: &Value) {
    match serde_yaml::to_string(data) {
        Ok(yaml) => print!("{}", yaml),
        Err(_) => println!("{}", serde_json::to_string_pretty(data).unwrap()),
    }
}

/// Execute a request. When an API key override is provided, go direct (skip daemon).
/// Otherwise try daemon first, fall back to direct HTTP.
async fn execute(req: &DaemonRequest, api_key_override: Option<&str>) -> Result<Value, String> {
    // If no override, try daemon first
    if api_key_override.is_none() {
        if daemon::ensure_daemon().is_ok() {
            match daemon::daemon_request(req).await {
                Ok(resp) if resp.ok => return Ok(resp.data.unwrap_or(Value::Null)),
                Ok(resp) => return Err(resp.error.unwrap_or("Unknown error".to_string())),
                Err(_) => {} // Fall through to direct
            }
        }
    }

    // Direct HTTP
    let api_key = resolve_api_key(api_key_override);
    let client = api_key_client(&api_key);

    match req.command.as_str() {
        "search" => {
            let query = req.query.as_deref().unwrap_or("");
            let resp = client
                .get(api_url("/v1/search"))
                .query(&[("query", query)])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            handle_response(resp).await
        }
        "fetch" => {
            let urls = req.urls.as_ref().ok_or("Missing urls")?;
            let resp = client
                .post(api_url("/v1/fetch"))
                .json(&json!({"urls": urls}))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            handle_response(resp).await
        }
        "feedback" => {
            let body = req.body.as_ref().ok_or("Missing body")?;
            let resp = client
                .post(api_url("/v1/feedback"))
                .json(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            handle_response(resp).await
        }
        _ => Err(format!("Unknown command: {}", req.command)),
    }
}

pub async fn search(query: &str, human: bool, api_key: Option<&str>) {
    let req = DaemonRequest {
        command: "search".to_string(),
        query: Some(query.to_string()),
        urls: None,
        body: None,
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header(&format!("keenable search \"{}\"", query));
                if let Some(results) = data["results"].as_array() {
                    if results.is_empty() {
                        ui::info("No results found.");
                        eprintln!();
                        return;
                    }
                    for (i, result) in results.iter().enumerate() {
                        let title = result["title"].as_str().unwrap_or("Untitled");
                        let url = result["url"].as_str().unwrap_or("");
                        let description = result["description"].as_str().unwrap_or("");
                        let desc_truncated: String = description.chars().take(200).collect();

                        let num = format!("{:>2}.", i + 1).dimmed();
                        eprintln!("   {} {}", num, title.bold());
                        eprintln!("      {}", url.cyan());
                        if !desc_truncated.is_empty() {
                            eprintln!("      {}", desc_truncated.dimmed());
                        }
                        eprintln!();
                    }
                } else {
                    ui::info("No results found.");
                    eprintln!();
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => {
            ui::error(&e);
            eprintln!();
            std::process::exit(1);
        }
    }
}

pub async fn fetch(urls: &[String], human: bool, api_key: Option<&str>) {
    let req = DaemonRequest {
        command: "fetch".to_string(),
        query: None,
        urls: Some(urls.to_vec()),
        body: None,
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header("keenable fetch");
                if let Some(results) = data["results"].as_array() {
                    for result in results {
                        let url = result["url"].as_str().unwrap_or("");
                        let content = result["content"].as_str().unwrap_or("");
                        eprintln!("   {}", url.cyan().bold());
                        eprintln!("   {}", "─".repeat(60).dimmed());
                        for line in content.lines() {
                            eprintln!("   {}", line);
                        }
                        eprintln!();
                    }
                }
                return;
            }
            print_yaml(&data);
        }
        Err(e) => {
            ui::error(&e);
            eprintln!();
            std::process::exit(1);
        }
    }
}

pub async fn feedback(query: &str, scores: &[String], human: bool, api_key: Option<&str>) {
    // Parse url=score=comment entries
    let mut relevance: Vec<Value> = Vec::new();
    for entry in scores {
        // Split as url=score or url=score=comment
        // URL may contain '=' (e.g. query params), so split from the right
        let parts: Vec<&str> = entry.rsplitn(3, '=').collect();
        if parts.len() < 2 {
            ui::error(&format!("Invalid format: {}. Expected url=score or url=score=comment.", entry));
            eprintln!();
            std::process::exit(1);
        }

        let (score_str, url, comment) = if parts.len() == 3 {
            // url=score=comment (rsplitn reverses: [comment, score, url])
            (parts[1], parts[2], parts[0])
        } else {
            // url=score (rsplitn reverses: [score, url])
            (parts[0], parts[1], "")
        };

        let score: u32 = match score_str.parse() {
            Ok(s) if s <= 5 => s,
            _ => {
                ui::error(&format!("Invalid score in '{}'. Must be 0-5.", entry));
                eprintln!();
                std::process::exit(1);
            }
        };
        relevance.push(json!({
            "url": url,
            "score": score,
            "comment": comment,
        }));
    }

    let body = json!({
        "query": query,
        "relevance": relevance,
    });

    let req = DaemonRequest {
        command: "feedback".to_string(),
        query: None,
        urls: None,
        body: Some(body),
    };

    match execute(&req, api_key).await {
        Ok(data) => {
            if human {
                ui::header("keenable feedback");
                ui::success("Feedback submitted");
                eprintln!();
                return;
            }
            print_yaml(&json!({"status": "ok", "message": "Feedback submitted", "data": data}));
        }
        Err(e) => {
            ui::error(&e);
            eprintln!();
            std::process::exit(1);
        }
    }
}
