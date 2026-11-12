use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Home directory keenable resolves all of its file paths against.
///
/// Honors the `KEENABLE_HOME` environment variable when set, falling back to
/// the OS home directory otherwise. The override is what lets the e2e suite
/// isolate each run under a temp dir on every platform — notably Windows,
/// where `dirs::home_dir()` queries the real user profile and ignores `HOME`.
/// Every place that needs the user's home goes through here so the override is
/// honored consistently across config, daemon, and MCP client paths.
pub fn home_dir() -> Option<PathBuf> {
    match std::env::var_os("KEENABLE_HOME") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => dirs::home_dir(),
    }
}

/// The home directory, or exit 1 with the fix: a container or service
/// account can run with no `HOME` and no passwd entry, and that must be a
/// clear error, not a panic.
pub const NO_HOME: &str = "Cannot determine your home directory";
pub const NO_HOME_HINT: &str =
    "Set HOME, or set KEENABLE_HOME to the directory keenable should keep its .keenable folder in.";

pub fn require_home_dir() -> PathBuf {
    home_dir().unwrap_or_else(|| {
        crate::ui::error(NO_HOME);
        crate::ui::hint(NO_HOME_HINT);
        eprintln!();
        std::process::exit(1);
    })
}

fn config_dir() -> PathBuf {
    require_home_dir().join(".keenable")
}

/// Create `~/.keenable` if needed and keep it private to the user: it holds
/// the API key and the daemon socket. Best effort on the mode (a directory
/// owned by someone else can't be changed); a no-op for the mode on Windows.
pub fn ensure_private_config_dir() -> std::io::Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&dir)
            && meta.permissions().mode() & 0o077 != 0
        {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).ok();
        }
    }
    Ok(dir)
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

/// Parse the config for a read-modify-write. A missing or empty file is an
/// empty config; a file that exists but is not a JSON object is an error, so
/// a write never silently replaces it (and the API key in it) with `{}`.
fn parse_config_for_write(content: Option<&str>) -> Result<Value, String> {
    let content = match content {
        Some(c) if !c.trim().is_empty() => c,
        _ => return Ok(Value::Object(Default::default())),
    };
    match serde_json::from_str::<Value>(content) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("it is not a JSON object".to_string()),
        Err(e) => Err(format!("it is not valid JSON ({e})")),
    }
}

fn read_config_for_write() -> Value {
    let path = config_file();
    let content = match fs::read_to_string(&path) {
        Ok(c) => Some(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => exit_config_error(&path, &format!("it could not be read ({e})")),
    };
    match parse_config_for_write(content.as_deref()) {
        Ok(v) => v,
        Err(why) => exit_config_error(&path, &why),
    }
}

/// Exit 1 now if a later write to config.json would be refused, before a
/// command does anything that can't be undone (provisioning a key).
pub fn ensure_config_writable() {
    read_config_for_write();
}

fn exit_config_error(path: &Path, why: &str) -> ! {
    crate::ui::error(&format!("Not changing {}: {}", path.display(), why));
    crate::ui::hint("Fix the file, or delete it and run keenable login again.");
    eprintln!();
    std::process::exit(1);
}

/// The file a write to `path` should land in: the link's target when `path`
/// is a symlink (dotfile managers link client configs into a repo), so the
/// link survives and the real file is the one updated.
fn write_target(path: &Path) -> PathBuf {
    let is_link = fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_link {
        return path.to_path_buf();
    }
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    // Dangling link: write where it points, one hop.
    match fs::read_link(path) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => path.parent().unwrap_or(Path::new(".")).join(target),
        Err(_) => path.to_path_buf(),
    }
}

/// Atomically replace `path` with `content`: write a uniquely-named temp file
/// in the same directory, then rename, so concurrent readers see either the
/// old or the new file, never a partial one. With `restrict`, the temp is
/// created 0600 (for files holding secrets); otherwise the destination's
/// existing permissions are preserved.
pub fn atomic_write(path: &Path, content: &str, restrict: bool) -> std::io::Result<()> {
    // `restrict` only governs Unix file modes (0o600); Windows has no equivalent.
    #[cfg(not(unix))]
    let _ = restrict;
    let path = &write_target(path);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Unique temp name — concurrent writers must not clobber each other's temp.
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);

    let result = (|| {
        // create_new: never write through a file or symlink someone else
        // left at the temp name. A leftover from a crashed run with the same
        // pid is removed (remove_file does not follow links) and retried once.
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if restrict {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        {
            use std::io::Write;
            let mut f = match opts.open(&tmp) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::remove_file(&tmp)?;
                    opts.open(&tmp)?
                }
                other => other?,
            };
            f.write_all(content.as_bytes())?;
            f.sync_all().ok();
        }
        #[cfg(unix)]
        if !restrict && let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions()).ok();
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

fn write_json(path: &Path, data: &Value) {
    let content = serde_json::to_string_pretty(data).unwrap();
    // restrict: the config holds API keys — never world-readable, even briefly
    let written = ensure_private_config_dir().and_then(|_| atomic_write(path, &content, true));
    if let Err(e) = written {
        crate::ui::error(&format!("Failed to write {}: {}", path.display(), e));
        eprintln!();
        std::process::exit(1);
    }
}

pub fn get_config() -> Value {
    read_json(&config_file())
}

/// Read-modify-write of the config object. Refuses (exit 1) to touch a
/// config file that exists but can't be parsed.
fn update_config(change: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut config = read_config_for_write();
    if let Value::Object(map) = &mut config {
        change(map);
    }
    write_json(&config_file(), &config);
}

pub fn set_config_value(key: &str, value: Value) {
    update_config(|map| {
        map.insert(key.to_string(), value);
    });
}

pub fn remove_config_value(key: &str) {
    update_config(|map| {
        map.remove(key);
    });
}

const API_KEY: &str = "api_key";
/// The API base the stored key was saved for. Absent means production:
/// that is where every key saved before this field existed came from.
const API_KEY_BASE: &str = "api_key_base";

/// The stored API key, as seen from the API base this process talks to.
#[derive(Debug, PartialEq)]
pub enum StoredKey {
    None,
    Usable(String),
    /// A key is stored, but for another API base. It is never sent to this
    /// one: `KEENABLE_API_URL` can come from anything that sets environment
    /// variables, and must not be able to redirect the key to another host.
    OtherBase {
        saved_for: String,
    },
}

/// Pure for unit tests. Keys are trimmed on save AND read (read covers
/// configs written by older versions or by hand): stray whitespace breaks
/// HTTP header building.
fn stored_key_for(config: &Value, current_base: &str) -> StoredKey {
    let Some(key) = config[API_KEY]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return StoredKey::None;
    };
    let saved_for = config[API_KEY_BASE]
        .as_str()
        .map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::constants::DEFAULT_API_BASE_URL);
    if same_base(saved_for, current_base) {
        StoredKey::Usable(key.to_string())
    } else {
        StoredKey::OtherBase {
            saved_for: saved_for.to_string(),
        }
    }
}

/// The stored key if it was saved for `base`. configure-mcp writes the
/// production MCP URL whatever `KEENABLE_API_URL` says, so it asks for the
/// production key here rather than the current base's.
pub fn api_key_for(base: &str) -> Option<String> {
    match stored_key_for(&get_config(), base) {
        StoredKey::Usable(key) => Some(key),
        _ => None,
    }
}

/// Two API bases name the same API: equal scheme, host and port (so
/// `HTTPS://API.keenable.ai:443` is production) and the same path prefix.
fn same_base(a: &str, b: &str) -> bool {
    let parse = |s: &str| reqwest::Url::parse(s.trim().trim_end_matches('/')).ok();
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => {
            a.scheme() == b.scheme()
                && a.host_str() == b.host_str()
                && a.port_or_known_default() == b.port_or_known_default()
                && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
        }
        _ => a == b,
    }
}

pub fn stored_key() -> StoredKey {
    // An invalid KEENABLE_API_URL matches no saved base: nothing is usable.
    let base = crate::api::resolved_api_base().unwrap_or("");
    stored_key_for(&get_config(), base)
}

/// The stored key, only when it was saved for the current API base.
pub fn get_api_key() -> Option<String> {
    match stored_key() {
        StoredKey::Usable(key) => Some(key),
        _ => None,
    }
}

/// Save `key` together with the API base it belongs to.
pub fn set_api_key(key: &str) {
    let base = crate::api::api_base();
    update_config(|map| {
        map.insert(API_KEY.into(), Value::String(key.trim().to_string()));
        if base == crate::constants::DEFAULT_API_BASE_URL {
            map.remove(API_KEY_BASE);
        } else {
            map.insert(API_KEY_BASE.into(), Value::String(base.to_string()));
        }
    });
}

pub fn remove_api_key() {
    update_config(|map| {
        map.remove(API_KEY);
        map.remove(API_KEY_BASE);
    });
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

/// None without a home directory: the update check is optional and must
/// never end a command that doesn't need one.
pub fn update_check_file() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".keenable").join(".update_check"))
}

// Tools that configure-mcp itself added to Claude Code's deny list. Reset
// uses this to restore only what we changed, not denies the user set
// independently. None = no record (configs from older versions).

pub fn get_managed_deny_tools() -> Option<Vec<String>> {
    get_config()["managed_deny_tools"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    })
}

pub fn add_managed_deny_tools(tools: &[String]) {
    let mut all = get_managed_deny_tools().unwrap_or_default();
    for t in tools {
        if !all.contains(t) {
            all.push(t.clone());
        }
    }
    set_config_value("managed_deny_tools", Value::from(all));
}

pub fn clear_managed_deny_tools() {
    remove_config_value("managed_deny_tools");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_API_BASE_URL;
    use serde_json::json;

    #[test]
    fn key_without_a_saved_base_belongs_to_production() {
        let config = json!({"api_key": " keen_x \n"});
        assert_eq!(
            stored_key_for(&config, DEFAULT_API_BASE_URL),
            StoredKey::Usable("keen_x".into())
        );
        assert_eq!(
            stored_key_for(&config, "https://evil.example"),
            StoredKey::OtherBase {
                saved_for: DEFAULT_API_BASE_URL.into()
            }
        );
    }

    #[test]
    fn key_saved_for_an_override_is_usable_only_there() {
        let config = json!({"api_key": "keen_x", "api_key_base": "http://localhost:8080/"});
        assert_eq!(
            stored_key_for(&config, "http://localhost:8080"),
            StoredKey::Usable("keen_x".into())
        );
        assert_eq!(
            stored_key_for(&config, DEFAULT_API_BASE_URL),
            StoredKey::OtherBase {
                saved_for: "http://localhost:8080".into()
            }
        );
        assert_eq!(
            stored_key_for(&config, ""),
            StoredKey::OtherBase {
                saved_for: "http://localhost:8080".into()
            }
        );
    }

    #[test]
    fn the_same_base_spelled_differently_still_matches() {
        let config = json!({"api_key": "keen_x"});
        for base in [
            "HTTPS://API.keenable.ai",
            "https://api.keenable.ai:443",
            "https://api.keenable.ai/",
        ] {
            assert_eq!(
                stored_key_for(&config, base),
                StoredKey::Usable("keen_x".into()),
                "{base}"
            );
        }
        for base in [
            "https://api.keenable.ai:8443",
            "http://api.keenable.ai",
            "https://api.keenable.ai/gw",
        ] {
            assert!(
                matches!(stored_key_for(&config, base), StoredKey::OtherBase { .. }),
                "{base}"
            );
        }
    }

    #[test]
    fn no_key_is_none_whatever_the_base() {
        for config in [json!({}), json!({"api_key": "  "}), json!({"api_key": 7})] {
            assert_eq!(
                stored_key_for(&config, DEFAULT_API_BASE_URL),
                StoredKey::None
            );
        }
    }

    #[test]
    fn unparseable_config_is_refused_for_writes() {
        assert_eq!(parse_config_for_write(None), Ok(json!({})));
        assert_eq!(parse_config_for_write(Some("  \n")), Ok(json!({})));
        assert_eq!(
            parse_config_for_write(Some(r#"{"api_key":"k"}"#)),
            Ok(json!({"api_key": "k"}))
        );
        assert!(parse_config_for_write(Some(r#"{"api_key":"k""#)).is_err());
        assert!(parse_config_for_write(Some("[]")).is_err());
    }

    #[cfg(unix)]
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kn-config-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn writing_a_symlinked_file_updates_the_target_and_keeps_the_link() {
        let dir = scratch("symlink");
        let real = dir.join("real.json");
        let link = dir.join("link.json");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        atomic_write(&link, "new", false).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_temp_symlink_is_not_written_through() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("tmp-link");
        let victim = dir.join("victim");
        fs::write(&victim, "untouched").unwrap();
        let path = dir.join("config.json");
        let tmp = dir.join(format!("config.json.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();

        atomic_write(&path, "secret", true).unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched");
        assert_eq!(fs::read_to_string(&path).unwrap(), "secret");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_dir_all(&dir).ok();
    }
}
