//! `configure-mcp` then `reset` per client: every file configure touched
//! comes back to its original content. Keyless, so no request is sent.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_home(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "kn-rt-{name}-{}-{}",
        std::process::id(),
        nanos % 1_000_000_000
    ));
    std::fs::create_dir_all(dir.join(".keenable")).unwrap();
    // A fresh update-check cache keeps the run off the network.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(
        dir.join(".keenable/.update_check"),
        json!({"last_check": now, "latest_version": "0.0.0"}).to_string(),
    )
    .unwrap();
    dir
}

fn keenable(home: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_keenable"))
        .args(args)
        .arg("--yes")
        .env("KEENABLE_HOME", home)
        .env("HOME", home)
        .env("KEENABLE_API_URL", "http://127.0.0.1:9")
        .env_remove("KEENABLE_API_KEY")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "keenable {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(home: &Path, rel: &str, content: &str) {
    let path = home.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_json(home: &Path, rel: &str, value: &Value) {
    write(home, rel, &serde_json::to_string_pretty(value).unwrap());
}

fn read(home: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(home.join(rel)).ok()
}

fn read_json(home: &Path, rel: &str) -> Option<Value> {
    read(home, rel).map(|s| serde_json::from_str(&s).unwrap())
}

/// Snapshot `files`, run configure then reset for `flag`, and check each
/// file is semantically what it was (absent stays absent). Returns the
/// post-configure contents so callers can check configure did change them.
fn round_trip(home: &Path, flag: &str, files: &[&str]) -> Vec<Option<String>> {
    let before: Vec<Option<Value>> = files.iter().map(|f| read_json(home, f)).collect();
    keenable(home, &["configure-mcp", flag]);
    let configured: Vec<Option<String>> = files.iter().map(|f| read(home, f)).collect();
    keenable(home, &["reset", flag]);
    for (file, want) in files.iter().zip(&before) {
        assert_eq!(&read_json(home, file), want, "{file} after reset");
    }
    let config = read_json(home, ".keenable/config.json").unwrap_or(json!({}));
    assert!(
        config.get("restore_records").is_none(),
        "records left after reset: {config}"
    );
    configured
}

#[test]
fn claude_code_restores_every_allow_list() {
    let home = temp_home("claude");
    let files = [
        ".claude.json",
        ".claude/settings.json",
        "dev/a/.claude/settings.local.json",
        "projects/b/.claude/settings.local.json",
        "src/c/.claude/settings.local.json",
    ];
    write_json(
        &home,
        files[0],
        &json!({
            "numStartups": 3,
            "mcpServers": {"other": {"type": "http", "url": "https://example.com/mcp"}},
            "permissions": {"allow": ["Bash(git:*)", "WebSearch"]}
        }),
    );
    write_json(
        &home,
        files[1],
        &json!({
            "model": "opus",
            "permissions": {"allow": ["WebFetch", "Read"], "deny": ["Bash(rm:*)"]}
        }),
    );
    // Only standard tools: configure drops `permissions` entirely.
    write_json(
        &home,
        files[2],
        &json!({"permissions": {"allow": ["WebSearch", "WebFetch"]}}),
    );
    write_json(
        &home,
        files[3],
        &json!({"permissions": {"allow": ["Bash(ls:*)", "WebFetch", "Edit"]}, "env": {"A": "1"}}),
    );
    // Never had them: reset must not add them.
    write_json(
        &home,
        files[4],
        &json!({"permissions": {"allow": ["Read"]}}),
    );

    let configured = round_trip(&home, "--claude-code", &files);
    for (file, text) in files.iter().zip(&configured).take(4) {
        let text = text.as_deref().unwrap();
        let allow = serde_json::from_str::<Value>(text).unwrap()["permissions"]["allow"].clone();
        assert!(
            !allow.to_string().contains("WebSearch") && !allow.to_string().contains("WebFetch"),
            "{file} still allows a standard tool after configure: {text}"
        );
    }

    // Order is kept too, not just membership.
    let b = read_json(&home, files[3]).unwrap();
    assert_eq!(
        b["permissions"]["allow"],
        json!(["Bash(ls:*)", "WebFetch", "Edit"])
    );
}

#[test]
fn claude_code_removes_files_and_keys_it_created() {
    let home = temp_home("claude-fresh");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let configured = round_trip(
        &home,
        "--claude-code",
        &[".claude.json", ".claude/settings.json"],
    );
    assert!(configured.iter().all(Option::is_some));
    assert!(!home.join(".claude.json").exists());
    assert!(!home.join(".claude/settings.json").exists());
}

#[test]
fn claude_code_reconfigure_keeps_the_original_record() {
    let home = temp_home("claude-twice");
    let local = "dev/a/.claude/settings.local.json";
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    write_json(
        &home,
        local,
        &json!({"permissions": {"allow": ["Read", "WebSearch"]}}),
    );
    keenable(&home, &["configure-mcp", "--claude-code"]);
    keenable(&home, &["configure-mcp", "--claude-code"]);
    keenable(&home, &["reset", "--claude-code"]);
    assert_eq!(
        read_json(&home, local).unwrap(),
        json!({"permissions": {"allow": ["Read", "WebSearch"]}})
    );
}

#[test]
fn opencode_restores_prior_permission_values() {
    let file = ".config/opencode/opencode.json";
    for (name, original) in [
        (
            "allow",
            json!({"mcp": {}, "permission": {"websearch": "allow", "bash": "ask"}}),
        ),
        ("absent", json!({"theme": "dark"})),
        ("string", json!({"mcp": {}, "permission": "ask"})),
        ("empty", json!({"mcp": {}, "permission": {}})),
    ] {
        let home = temp_home(&format!("opencode-{name}"));
        write_json(&home, file, &original);
        let configured = round_trip(&home, "--opencode", &[file]);
        let configured: Value = serde_json::from_str(configured[0].as_deref().unwrap()).unwrap();
        assert_eq!(configured["permission"]["websearch"], "deny", "{name}");
    }
}

#[test]
fn opencode_keeps_a_deny_the_user_set() {
    let home = temp_home("opencode-user-deny");
    let file = ".config/opencode/opencode.json";
    let original = json!({"mcp": {}, "permission": {"websearch": "deny"}});
    write_json(&home, file, &original);
    round_trip(&home, "--opencode", &[file]);
}

#[test]
fn json_clients_round_trip() {
    for (flag, file) in [
        ("--cursor", ".cursor/mcp.json"),
        ("--windsurf", ".codeium/windsurf/mcp_config.json"),
        ("--devin", ".config/devin/mcp_config.json"),
    ] {
        let home = temp_home(&flag[2..]);
        write_json(
            &home,
            file,
            &json!({"mcpServers": {"other": {"url": "https://example.com/mcp"}}, "x": 1}),
        );
        round_trip(&home, flag, &[file]);

        // No file before: none after.
        let home = temp_home(&format!("{}-fresh", &flag[2..]));
        std::fs::create_dir_all(home.join(file).parent().unwrap()).unwrap();
        let configured = round_trip(&home, flag, &[file]);
        assert!(configured[0].is_some(), "{flag} wrote nothing");
        assert!(!home.join(file).exists(), "{flag} left {file}");
    }
}

#[test]
fn codex_round_trips_byte_for_byte() {
    let home = temp_home("codex");
    let file = ".codex/config.toml";
    let original = "# my settings\nmodel = \"gpt-5\"\n\n[tools]\nweb_search = true # keep\n\n[mcp_servers.other]\nurl = \"https://example.com/mcp\"\n";
    write(&home, file, original);
    keenable(&home, &["configure-mcp", "--codex"]);
    assert!(read(&home, file).unwrap().contains("keenable"));
    keenable(&home, &["reset", "--codex"]);
    assert_eq!(read(&home, file).unwrap(), original);
}
