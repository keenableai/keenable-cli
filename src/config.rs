use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".keenable")
}

fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

fn credentials_file() -> PathBuf {
    config_dir().join("credentials.json")
}

fn read_json(path: &PathBuf) -> Value {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or(Value::Object(Default::default())),
        Err(_) => Value::Object(Default::default()),
    }
}

fn write_json(path: &PathBuf, data: &Value) {
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir).expect("failed to create config directory");
    let content = serde_json::to_string_pretty(data).unwrap();
    fs::write(path, &content).expect("failed to write config file");

    // Restrict permissions — config files contain API keys
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok();
    }
}

pub fn get_config() -> Value {
    read_json(&config_file())
}

pub fn set_config_value(key: &str, value: Value) {
    let mut config = get_config();
    config[key] = value;
    write_json(&config_file(), &config);
}

pub fn get_api_key() -> Option<String> {
    get_config()["api_key"].as_str().map(|s| s.to_string())
}

pub fn set_api_key(key: &str) {
    set_config_value("api_key", Value::String(key.to_string()));
}

pub fn get_org_id() -> Option<String> {
    get_config()["org_id"].as_str().map(|s| s.to_string())
}

pub fn set_org_id(org_id: &str) {
    set_config_value("org_id", Value::String(org_id.to_string()));
}

pub fn get_credentials() -> Value {
    read_json(&credentials_file())
}

pub fn set_credentials(data: &Value) {
    write_json(&credentials_file(), data);
}

pub fn get_access_token() -> Option<String> {
    get_credentials()["access_token"]
        .as_str()
        .map(|s| s.to_string())
}

pub fn clear_credentials() {
    let path = credentials_file();
    if path.exists() {
        fs::remove_file(path).ok();
    }
}

pub fn get_skip_setup_confirmation() -> bool {
    get_config()["skip_setup_confirmation"]
        .as_bool()
        .unwrap_or(false)
}

pub fn set_skip_setup_confirmation(value: bool) {
    set_config_value("skip_setup_confirmation", Value::Bool(value));
}

pub fn update_check_file() -> PathBuf {
    config_dir().join(".update_check")
}
