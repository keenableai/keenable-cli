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

    // Write to a temp file created 0600 (the config holds API keys — it must
    // never exist world-readable, even briefly), then rename so concurrent
    // readers see either the old or the new file, never a partial one.
    let tmp = path.with_extension("json.tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        let mut f = opts.open(&tmp).expect("failed to write config file");
        f.write_all(content.as_bytes())
            .expect("failed to write config file");
    }
    fs::rename(&tmp, path).expect("failed to write config file");
}

pub fn get_config() -> Value {
    read_json(&config_file())
}

pub fn set_config_value(key: &str, value: Value) {
    let mut config = get_config();
    // A valid-JSON-but-not-object file (e.g. hand-edited to `[]`) would make
    // the index assignment panic; start over instead.
    if !config.is_object() {
        config = Value::Object(Default::default());
    }
    config[key] = value;
    write_json(&config_file(), &config);
}

pub fn remove_config_value(key: &str) {
    let mut config = get_config();
    if let Value::Object(ref mut map) = config {
        map.remove(key);
    }
    write_json(&config_file(), &config);
}

pub fn get_api_key() -> Option<String> {
    get_config()["api_key"].as_str().map(|s| s.to_string())
}

pub fn set_api_key(key: &str) {
    set_config_value("api_key", Value::String(key.to_string()));
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
