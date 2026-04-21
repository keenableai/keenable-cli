use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use colored::Colorize;
use rand::Rng;
use reqwest::Client;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::api::{api_url, bearer_client, handle_response};
use crate::config;
use crate::constants::*;

fn generate_pkce() -> (String, String) {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    let verifier = URL_SAFE_NO_PAD.encode(&bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

async fn discover_endpoints() -> Result<(String, String), String> {
    let client = Client::new();
    let resp = client
        .get(OAUTH_DISCOVERY_URL)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch OAuth discovery: {}", e))?;
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse discovery: {}", e))?;
    let auth_endpoint = data["authorization_endpoint"]
        .as_str()
        .ok_or("Missing authorization_endpoint")?
        .to_string();
    let token_endpoint = data["token_endpoint"]
        .as_str()
        .ok_or("Missing token_endpoint")?
        .to_string();
    Ok((auth_endpoint, token_endpoint))
}

fn wait_for_code(timeout_secs: u64) -> Result<String, String> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", CALLBACK_PORT))
        .map_err(|e| format!("Failed to bind callback port {}: {}", CALLBACK_PORT, e))?;
    listener
        .set_nonblocking(false)
        .map_err(|e| format!("Failed to set blocking: {}", e))?;

    let code: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);

    // Set a timeout on the listener
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Failed to set non-blocking: {}", e))?;

    loop {
        if std::time::Instant::now() > deadline {
            return Err("Timed out waiting for authentication callback.".to_string());
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 4096];
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();

                // Parse the GET request for the code parameter
                if let Some(line) = request.lines().next() {
                    if let Some(path) = line.split_whitespace().nth(1) {
                        if path.starts_with(CALLBACK_PATH) {
                            if let Some(query) = path.split('?').nth(1) {
                                for param in query.split('&') {
                                    let mut kv = param.splitn(2, '=');
                                    if kv.next() == Some("code") {
                                        if let Some(val) = kv.next() {
                                            *code.lock().unwrap() = Some(val.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Send response
                let body = if code.lock().unwrap().is_some() {
                    include_str!("../../assets/login_success.html")
                } else {
                    include_str!("../../assets/login_failure.html")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).ok();
                stream.flush().ok();

                if let Some(c) = code.lock().unwrap().clone() {
                    return Ok(c);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => {
                return Err(format!("Accept error: {}", e));
            }
        }
    }
}

async fn exchange_code(
    token_endpoint: &str,
    code: &str,
    verifier: &str,
) -> Result<serde_json::Value, String> {
    let client = Client::new();
    let resp = client
        .post(token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", OAUTH_CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Token exchange failed: {}", e))?;
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))
}

async fn fetch_user_and_org(access_token: &str) -> Result<(Option<String>, String), String> {
    let client = bearer_client(access_token);

    // Sign in
    client
        .post(api_url("/bff/v1/auth/signin"))
        .send()
        .await
        .map_err(|e| format!("Signin failed: {}", e))?;

    // Get user info
    let resp = client
        .get(api_url("/bff/v1/auth/user"))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch user: {}", e))?;
    let user = handle_response(resp).await?;
    let org_id = user["current_org_id"]
        .as_str()
        .map(|s| s.to_string());
    let email = user["email"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    Ok((org_id, email))
}

async fn auto_provision_key(access_token: &str, org_id: &str) -> Result<String, String> {
    let client = bearer_client(access_token);
    let resp = client
        .post(api_url("/bff/v1/auth/api_keys"))
        .query(&[("org_id", org_id)])
        .json(&serde_json::json!({"label": "cli"}))
        .send()
        .await
        .map_err(|e| format!("Failed to create API key: {}", e))?;
    let data = handle_response(resp).await?;
    data["api_key"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or("Missing api_key in response".to_string())
}

pub async fn login() {
    use crate::ui;

    ui::header("keenable login");

    // Step 1: Discover OAuth endpoints
    let (auth_endpoint, token_endpoint) = match discover_endpoints().await {
        Ok(endpoints) => endpoints,
        Err(e) => {
            ui::error(&format!("Discovery failed: {}", e));
            std::process::exit(1);
        }
    };
    ui::step_done("Discovered OAuth endpoints");

    // Step 2: Generate PKCE
    let (verifier, challenge) = generate_pkce();

    // Step 3: Build auth URL and open browser
    let state: String = {
        let mut rng = rand::thread_rng();
        let bytes: Vec<u8> = (0..16).map(|_| rng.gen::<u8>()).collect();
        URL_SAFE_NO_PAD.encode(&bytes)
    };

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
        auth_endpoint, OAUTH_CLIENT_ID, REDIRECT_URI, challenge, state
    );

    if open::that(&auth_url).is_err() {
        eprintln!("   Please open this URL in your browser:\n   {}", auth_url);
    }
    ui::step_done("Opened browser");

    // Step 4: Wait for callback
    let code = match wait_for_code(120) {
        Ok(code) => code,
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    };
    ui::step_done("Received callback");

    // Step 5: Exchange code for tokens
    let tokens = match exchange_code(&token_endpoint, &code, &verifier).await {
        Ok(t) => t,
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    };

    let access_token = tokens["access_token"]
        .as_str()
        .expect("Missing access_token");
    config::set_credentials(&tokens);
    ui::step_done("Exchanged tokens");

    // Step 6: Fetch user and org
    let (org_id, email) = match fetch_user_and_org(access_token).await {
        Ok(info) => info,
        Err(e) => {
            ui::error(&e);
            std::process::exit(1);
        }
    };
    if let Some(ref id) = org_id {
        config::set_org_id(id);
    }
    ui::success(&format!("Logged in as {}", email));

    // Step 7: Auto-provision API key if needed
    if config::get_api_key().is_none() {
        if let Some(ref id) = org_id {
            match auto_provision_key(access_token, id).await {
                Ok(key) => {
                    config::set_api_key(&key);
                    ui::success("API key created and saved");
                }
                Err(e) => {
                    ui::error(&format!("Failed to create API key: {}", e));
                }
            }
        } else {
            ui::hint("No organization found — API key not created. Join an org and run login again.");
        }
    }

    ui::hint(&format!("Next: {}", "keenable setup".cyan()));
    eprintln!();
}

pub fn logout() {
    use crate::ui;

    ui::header("keenable logout");

    config::clear_credentials();
    config::set_config_value("api_key", serde_json::Value::Null);

    ui::step_done("Cleared credentials");
    ui::step_done("Removed API key");
    ui::success("Logged out");
    eprintln!();
}
