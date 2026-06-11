use std::fs;
use std::path::{Path, PathBuf};

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

/// Atomically replace `path` with `content`: write a uniquely-named temp file
/// in the same directory, then rename, so concurrent readers see either the
/// old or the new file, never a partial one. With `restrict`, the temp is
/// created 0600 (for files holding secrets); otherwise the destination's
/// existing permissions are preserved.
pub fn atomic_write(path: &Path, content: &str, restrict: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Unique temp name — concurrent writers must not clobber each other's temp.
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);

    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if restrict {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        {
            use std::io::Write;
            let mut f = opts.open(&tmp)?;
            f.write_all(content.as_bytes())?;
        }
        #[cfg(unix)]
        if !restrict {
            if let Ok(meta) = fs::metadata(path) {
                fs::set_permissions(&tmp, meta.permissions()).ok();
            }
        }
        let renamed = fs::rename(&tmp, path);
        // Windows refuses to replace a destination that's locked or
        // read-only; fall back to remove + rename (brief non-atomic window).
        #[cfg(windows)]
        let renamed = renamed.or_else(|_| {
            fs::remove_file(path)?;
            fs::rename(&tmp, path)
        });
        renamed
    })();
    if result.is_err() {
        fs::remove_file(&tmp).ok();
    }
    result
}

fn write_json(path: &PathBuf, data: &Value) {
    let content = serde_json::to_string_pretty(data).unwrap();
    // restrict: the config holds API keys — never world-readable, even briefly
    if let Err(e) = atomic_write(path, &content, true) {
        crate::ui::error(&format!("Failed to write {}: {}", path.display(), e));
        eprintln!();
        std::process::exit(1);
    }
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
