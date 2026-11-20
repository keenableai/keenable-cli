//! `keenable monitors`: the Monitors API (`/v1/monitors`) from the shell.
//!
//! Monitors need an API key: there is no keyless endpoint. Every call goes
//! straight to the API, never through the daemon: the daemon retries a failed
//! round-trip directly, which is only safe for reads, and a create or delete
//! sent twice would act twice.

use colored::Colorize;
use reqwest::Method;
use serde_json::{Map, Value, json};

use crate::api::{ApiError, api_key_client, api_url, handle_response};
use crate::commands::search::{
    handle_api_error, key_override_or_exit, print_yaml, stored_key_not_used,
};
use crate::config;
use crate::net;
use crate::ui;

/// When a monitor runs: an interval, a cron expression or a single time.
#[derive(Debug, Default, Clone, PartialEq)]
pub enum ScheduleArg {
    #[default]
    Unchanged,
    Every(String),
    Cron {
        expr: String,
        timezone: Option<String>,
    },
    Once(String),
    Remove,
}

impl ScheduleArg {
    pub fn from_flags(
        every: Option<String>,
        cron: Option<String>,
        timezone: Option<String>,
        once: Option<String>,
        remove: bool,
    ) -> Self {
        if remove {
            return Self::Remove;
        }
        if let Some(v) = every {
            return Self::Every(v);
        }
        if let Some(expr) = cron {
            return Self::Cron { expr, timezone };
        }
        if let Some(v) = once {
            return Self::Once(v);
        }
        Self::Unchanged
    }

    fn to_json(&self) -> Option<Value> {
        match self {
            Self::Unchanged => None,
            Self::Remove => Some(Value::Null),
            Self::Every(v) => Some(json!({"type": "interval", "value": v})),
            Self::Cron { expr, timezone } => {
                let mut s = json!({"type": "cron", "value": expr});
                if let Some(tz) = timezone {
                    s["timezone"] = json!(tz);
                }
                Some(s)
            }
            Self::Once(v) if v.is_empty() || v == "now" => Some(json!({"type": "once"})),
            Self::Once(v) => Some(json!({"type": "once", "value": v})),
        }
    }
}

/// What `create` and `update` send. `None` leaves a field out.
#[derive(Debug, Default)]
pub struct MonitorFields {
    pub query: Option<String>,
    pub name: Option<String>,
    pub max_results: Option<u64>,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
    pub text: bool,
    pub schedule: ScheduleArg,
    pub webhook: Option<String>,
    pub events: Vec<String>,
    pub remove_webhook: bool,
    pub metadata: Vec<(String, String)>,
    pub status: Option<String>,
}

fn domains(raw: &[String]) -> Vec<String> {
    raw.iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect()
}

/// The JSON body of a create (`for_create`) or an update.
pub fn monitor_body(f: &MonitorFields, for_create: bool) -> Value {
    let mut body = Map::new();
    if let Some(name) = &f.name {
        body.insert("name".into(), json!(name));
    }
    if let Some(status) = &f.status {
        body.insert("status".into(), json!(status));
    }

    let mut search = Map::new();
    if let Some(q) = &f.query {
        search.insert("query".into(), json!(q));
    }
    if let Some(n) = f.max_results {
        search.insert("max_results".into(), json!(n));
    }
    let include = domains(&f.include_domains);
    if !include.is_empty() {
        search.insert("include_domains".into(), json!(include));
    }
    let exclude = domains(&f.exclude_domains);
    if !exclude.is_empty() {
        search.insert("exclude_domains".into(), json!(exclude));
    }
    if f.text {
        search.insert("content".into(), json!({"text": true}));
    }
    if for_create || !search.is_empty() {
        body.insert("search".into(), Value::Object(search));
    }

    if let Some(schedule) = f.schedule.to_json() {
        body.insert("schedule".into(), schedule);
    }

    if f.remove_webhook {
        body.insert("webhook".into(), Value::Null);
    } else if f.webhook.is_some() || !f.events.is_empty() {
        let mut hook = Map::new();
        if let Some(url) = &f.webhook {
            hook.insert("url".into(), json!(url));
        }
        let events = domains(&f.events);
        if !events.is_empty() {
            hook.insert("events".into(), json!(events));
        }
        body.insert("webhook".into(), Value::Object(hook));
    }

    if !f.metadata.is_empty() {
        let meta: Map<String, Value> = f
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        body.insert("metadata".into(), Value::Object(meta));
    }
    Value::Object(body)
}

/// `key=value` from `--metadata`.
pub fn parse_metadata(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((k, v)) if !k.trim().is_empty() => Ok((k.trim().to_string(), v.to_string())),
        _ => Err(format!("expected key=value, got {raw:?}")),
    }
}

/// One request to the Monitors API.
#[derive(Debug)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
    pub idempotency_key: Option<String>,
}

impl Call {
    fn new(method: Method, path: String) -> Self {
        Call {
            method,
            path,
            query: Vec::new(),
            body: None,
            idempotency_key: None,
        }
    }

    fn param(mut self, name: &str, value: Option<impl ToString>) -> Self {
        if let Some(v) = value {
            self.query.push((name.to_string(), v.to_string()));
        }
        self
    }
}

/// Percent-encodes everything but the unreserved characters, so a value
/// stays one path segment.
fn segment(raw: &str) -> String {
    raw.trim()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `/v1/monitors/<id>...`, with the id escaped so it stays one path segment.
pub fn monitor_path(id: &str, rest: &str) -> String {
    format!("/v1/monitors/{}{}", segment(id), rest)
}

fn key_required() -> ApiError {
    ApiError {
        status: 401,
        error: "API key required".into(),
        error_type: Some("authentication_error".into()),
        message: Some("Monitors need an API key; there is no keyless access.".into()),
        transport_hint: Some(
            "Run `keenable login`, or pass a key with --api-key or KEENABLE_API_KEY.".into(),
        ),
        ..Default::default()
    }
}

/// The key this command sends: the override, else the one stored for this
/// API base. Exits in the command's output format when there is none.
fn key_or_exit(api_key_override: Option<String>, human: bool) -> String {
    if let Some(key) = api_key_override {
        return key;
    }
    if config::home_dir().is_none() {
        let err = ApiError {
            error: config::NO_HOME.into(),
            transport_hint: Some(config::NO_HOME_HINT.into()),
            ..Default::default()
        };
        handle_api_error((err, false), human);
    }
    if let config::StoredKey::OtherBase { saved_for } = config::stored_key() {
        handle_api_error((stored_key_not_used(&saved_for), false), human);
    }
    match config::get_api_key() {
        Some(key) => key.trim().to_string(),
        None => handle_api_error((key_required(), false), human),
    }
}

async fn send(call: &Call, key: &str) -> Result<Value, Box<ApiError>> {
    net::check_cert_file().map_err(|e| Box::new(ApiError::from(e)))?;
    let client = api_key_client(key).map_err(|e| Box::new(ApiError::from(e)))?;
    let mut req = client
        .request(call.method.clone(), api_url(&call.path))
        .query(&call.query);
    if let Some(body) = &call.body {
        req = req.json(body);
    }
    if let Some(k) = &call.idempotency_key {
        req = req.header("Idempotency-Key", k);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Box::new(ApiError::from_transport(&e)))?;
    handle_response(resp).await
}

/// Which pretty view a response gets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Monitor,
    Monitors,
    Results,
    Run,
    Runs,
    Deliveries,
    Plain,
}

/// Run a call and print the answer: YAML on stdout, or a readable view on
/// stderr with `-p`.
pub async fn run(call: Call, view: View, human: bool, api_key: Option<&str>) {
    let key = key_or_exit(key_override_or_exit(api_key, human), human);
    match send(&call, &key).await {
        Ok(data) if human => print_pretty(view, &data),
        Ok(data) => print_yaml(&data),
        Err(e) => handle_api_error((*e, true), human),
    }
}

/// Run `create`.
pub async fn create(
    fields: MonitorFields,
    idempotency_key: Option<String>,
    human: bool,
    api_key: Option<&str>,
) {
    let mut call = Call::new(Method::POST, "/v1/monitors".into());
    call.body = Some(monitor_body(&fields, true));
    call.idempotency_key = idempotency_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    run(call, View::Monitor, human, api_key).await;
}

pub fn list_call(
    status: Option<String>,
    name: Option<String>,
    limit: Option<u64>,
    cursor: Option<String>,
) -> Call {
    Call::new(Method::GET, "/v1/monitors".into())
        .param("status", status)
        .param("name", name)
        .param("limit", limit)
        .param("cursor", cursor)
}

pub fn get_call(id: &str) -> Call {
    Call::new(Method::GET, monitor_path(id, ""))
}

pub fn update_call(id: &str, fields: &MonitorFields) -> Call {
    let mut call = Call::new(Method::PATCH, monitor_path(id, ""));
    call.body = Some(monitor_body(fields, false));
    call
}

pub fn status_call(id: &str, status: &str) -> Call {
    let mut call = Call::new(Method::PATCH, monitor_path(id, ""));
    call.body = Some(json!({"status": status}));
    call
}

pub fn delete_call(id: &str) -> Call {
    Call::new(Method::DELETE, monitor_path(id, ""))
}

pub fn trigger_call(id: &str) -> Call {
    let mut call = Call::new(Method::POST, monitor_path(id, "/trigger"));
    call.body = Some(json!({}));
    call
}

pub fn runs_call(id: &str, limit: Option<u64>, cursor: Option<String>) -> Call {
    Call::new(Method::GET, monitor_path(id, "/runs"))
        .param("limit", limit)
        .param("cursor", cursor)
}

pub fn run_call(id: &str, run_id: &str) -> Call {
    Call::new(
        Method::GET,
        monitor_path(id, &format!("/runs/{}", segment(run_id))),
    )
}

pub fn results_call(
    id: &str,
    limit: Option<u64>,
    cursor: Option<String>,
    min_importance: Option<u8>,
) -> Call {
    Call::new(Method::GET, monitor_path(id, "/results"))
        .param("limit", limit)
        .param("cursor", cursor)
        .param("min_importance", min_importance)
}

pub fn deliveries_call(id: &str, limit: Option<u64>, cursor: Option<String>) -> Call {
    Call::new(Method::GET, monitor_path(id, "/deliveries"))
        .param("limit", limit)
        .param("cursor", cursor)
}

pub fn stats_call() -> Call {
    Call::new(Method::GET, "/v1/monitors/stats".into())
}

/// The refusal for a delete that could not be confirmed.
fn confirmation_required(id: &str) -> ApiError {
    ApiError {
        error: "Confirmation required".into(),
        message: Some(format!(
            "Deleting {} removes the monitor and every result it found.",
            ui::one_line(id)
        )),
        transport_hint: Some(
            "Pass --yes to delete without asking, or pause it instead: keenable monitors pause <id>."
                .into(),
        ),
        ..Default::default()
    }
}

/// Run `delete`, after a confirmation unless `yes`. Without a terminal to
/// ask on (stdin and stderr), or when the prompt fails, it stops with exit 1
/// instead of deleting; only an answered "no" exits 0.
pub async fn delete(id: &str, yes: bool, human: bool, api_key: Option<&str>) {
    if !yes {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            handle_api_error((confirmation_required(id), true), human);
        }
        let answer = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Delete {} and every result it found? This cannot be undone",
                ui::one_line(id)
            ))
            .default(false)
            .interact();
        match answer {
            Ok(true) => {}
            Ok(false) => {
                ui::info("Nothing deleted.");
                return;
            }
            Err(_) => handle_api_error((confirmation_required(id), true), human),
        }
    }
    run(delete_call(id), View::Monitor, human, api_key).await;
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn schedule_line(schedule: &Value) -> String {
    match text(schedule, "type") {
        "interval" => format!("every {}", text(schedule, "value")),
        "cron" => {
            let tz = schedule["timezone"].as_str().unwrap_or("UTC");
            format!("cron {} ({tz})", text(schedule, "value"))
        }
        "once" => format!("once at {}", text(schedule, "value")),
        _ => "only when triggered".into(),
    }
}

fn print_monitor(m: &Value) {
    let name = m["name"]
        .as_str()
        .unwrap_or_else(|| text(&m["search"], "query"));
    eprintln!(
        "   {}  {}  {}",
        ui::one_line(name).bold(),
        ui::one_line(text(m, "id")).dimmed(),
        ui::one_line(text(m, "status")).cyan()
    );
    eprintln!("      Query: {}", ui::one_line(text(&m["search"], "query")));
    eprintln!(
        "      Schedule: {}",
        ui::one_line(&schedule_line(&m["schedule"]))
    );
    if let Some(next) = m["next_run_at"].as_str() {
        eprintln!("      Next run: {}", ui::one_line(next));
    }
    let search = &m["search"];
    let mut options = Vec::new();
    if let Some(n) = search["max_results"].as_u64() {
        options.push(format!("max {n} results a run"));
    }
    if search["content"]["text"] == true {
        options.push("with page text".into());
    }
    if !options.is_empty() {
        eprintln!("      Options: {}", ui::one_line(&options.join(", ")));
    }
    for (label, key) in [
        ("Only sites", "include_domains"),
        ("Never sites", "exclude_domains"),
    ] {
        if let Some(list) = joined(&search[key]) {
            eprintln!("      {label}: {}", ui::one_line(&list));
        }
    }
    if let Some(url) = m["webhook"]["url"].as_str() {
        eprintln!("      Webhook: {}", ui::one_line(url));
        if let Some(events) = joined(&m["webhook"]["events"]) {
            eprintln!("      Events: {}", ui::one_line(&events));
        }
    }
    if let Some(meta) = m["metadata"].as_object().filter(|o| !o.is_empty()) {
        let pairs: Vec<String> = meta
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
            .collect();
        eprintln!("      Metadata: {}", ui::one_line(&pairs.join(", ")));
    }
}

/// A JSON list of strings as "a, b", or None when it is missing or empty.
fn joined(list: &Value) -> Option<String> {
    let items: Vec<&str> = list.as_array()?.iter().filter_map(Value::as_str).collect();
    (!items.is_empty()).then(|| items.join(", "))
}

fn print_result(p: &Value) {
    let title = p["title"].as_str().unwrap_or("Untitled");
    eprintln!("   {}", ui::one_line(title).bold());
    eprintln!("      {}", ui::one_line(text(p, "url")).cyan());
    if let Some(summary) = p["summary"].as_str() {
        eprintln!("      {}", ui::one_line(summary));
    }
    let mut facts = Vec::new();
    if let Some(i) = p["importance"].as_u64() {
        facts.push(format!("importance {i}/5"));
    }
    if let Some(d) = p["published_at"].as_str() {
        facts.push(format!("published {d}"));
    }
    if let Some(n) = p["same_story"].as_u64().filter(|n| *n > 0) {
        facts.push(format!("{n} more on the same story"));
    }
    if !facts.is_empty() {
        eprintln!("      {}", ui::one_line(&facts.join("  ")).dimmed());
    }
}

fn print_run(r: &Value) {
    let count = r["results"].as_array().map(|a| a.len());
    eprintln!(
        "   {}  {}  {}{}",
        ui::one_line(text(r, "id")).bold(),
        ui::one_line(text(r, "status")).cyan(),
        ui::one_line(r["created_at"].as_str().unwrap_or("")).dimmed(),
        count.map(|n| format!("  {n} results")).unwrap_or_default()
    );
    if let Some(msg) = r["error"]["message"].as_str() {
        eprintln!("      {}", ui::one_line(msg).red());
    }
}

fn more_hint(data: &Value) {
    if data["has_more"] == true
        && let Some(cursor) = data["next_cursor"].as_str()
    {
        ui::hint(&format!("More: add --cursor {}", ui::one_line(cursor)));
    }
}

fn list_of(data: &Value) -> &[Value] {
    data["data"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn print_pretty(view: View, data: &Value) {
    ui::header("keenable monitors");
    match view {
        View::Monitor => {
            print_monitor(data);
            if let Some(secret) = data["webhook"]["secret"].as_str() {
                eprintln!();
                ui::warning(&format!(
                    "Webhook secret: {} (store it now: it is not shown again)",
                    ui::one_line(secret)
                ));
            }
        }
        View::Monitors => {
            let items = list_of(data);
            if items.is_empty() {
                ui::info("No monitors yet. Create one: keenable monitors create \"<topic>\"");
            }
            for m in items {
                print_monitor(m);
            }
            more_hint(data);
        }
        View::Results => {
            let items = list_of(data);
            if items.is_empty() {
                ui::info(
                    "No results yet. A new monitor only sees pages crawled after it was created.",
                );
            }
            for p in items {
                print_result(p);
            }
            more_hint(data);
        }
        View::Run => {
            print_run(data);
            for p in data["results"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                print_result(p);
            }
        }
        View::Runs => {
            let items = list_of(data);
            if items.is_empty() {
                ui::info("No runs yet.");
            }
            for r in items {
                print_run(r);
            }
            more_hint(data);
        }
        View::Deliveries => {
            for d in list_of(data) {
                let state = if d["delivered_at"].is_string() {
                    "delivered".green()
                } else {
                    "pending".yellow()
                };
                eprintln!(
                    "   {}  {}  {}  attempts {}",
                    ui::one_line(text(d, "type")).bold(),
                    state,
                    ui::one_line(text(d, "created_at")).dimmed(),
                    d["attempts"].as_u64().unwrap_or(0)
                );
                if let Some(e) = d["last_error"].as_str() {
                    eprintln!("      {}", ui::one_line(e).red());
                }
            }
            more_hint(data);
        }
        View::Plain => {
            let yaml = serde_yaml::to_string(data).unwrap_or_default();
            for line in ui::printable(&yaml).lines() {
                eprintln!("   {line}");
            }
        }
    }
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_carries_the_search_and_leaves_unset_fields_out() {
        let f = MonitorFields {
            query: Some("open-weights LLM releases".into()),
            ..Default::default()
        };
        assert_eq!(
            monitor_body(&f, true),
            json!({"search": {"query": "open-weights LLM releases"}})
        );
    }

    #[test]
    fn create_body_maps_every_flag() {
        let f = MonitorFields {
            query: Some("q".into()),
            name: Some("Releases".into()),
            max_results: Some(25),
            include_domains: vec!["a.com".into(), " ".into()],
            exclude_domains: vec![" b.com ".into()],
            text: true,
            schedule: ScheduleArg::Cron {
                expr: "0 9 * * 1-5".into(),
                timezone: Some("Europe/Paris".into()),
            },
            webhook: Some("https://example.com/hook".into()),
            events: vec!["monitor.result.found".into()],
            metadata: vec![("team".into(), "research".into())],
            ..Default::default()
        };
        assert_eq!(
            monitor_body(&f, true),
            json!({
                "name": "Releases",
                "search": {
                    "query": "q",
                    "max_results": 25,
                    "include_domains": ["a.com"],
                    "exclude_domains": ["b.com"],
                    "content": {"text": true}
                },
                "schedule": {"type": "cron", "value": "0 9 * * 1-5", "timezone": "Europe/Paris"},
                "webhook": {"url": "https://example.com/hook", "events": ["monitor.result.found"]},
                "metadata": {"team": "research"}
            })
        );
    }

    #[test]
    fn update_body_sends_only_what_changes() {
        let f = MonitorFields {
            name: Some("New name".into()),
            ..Default::default()
        };
        assert_eq!(monitor_body(&f, false), json!({"name": "New name"}));

        let f = MonitorFields {
            schedule: ScheduleArg::Remove,
            remove_webhook: true,
            ..Default::default()
        };
        assert_eq!(
            monitor_body(&f, false),
            json!({"schedule": null, "webhook": null})
        );
    }

    #[test]
    fn schedules_follow_the_api_shape() {
        let every = ScheduleArg::from_flags(Some("6h".into()), None, None, None, false);
        assert_eq!(
            every.to_json(),
            Some(json!({"type": "interval", "value": "6h"}))
        );
        let once = ScheduleArg::from_flags(None, None, None, Some("now".into()), false);
        assert_eq!(once.to_json(), Some(json!({"type": "once"})));
        let later = ScheduleArg::from_flags(None, None, None, Some("2h".into()), false);
        assert_eq!(
            later.to_json(),
            Some(json!({"type": "once", "value": "2h"}))
        );
        assert_eq!(
            ScheduleArg::from_flags(None, None, None, None, false).to_json(),
            None
        );
        assert_eq!(
            ScheduleArg::from_flags(Some("1d".into()), None, None, None, true).to_json(),
            Some(Value::Null)
        );
    }

    #[test]
    fn metadata_needs_a_key() {
        assert_eq!(
            parse_metadata("team=a=b"),
            Ok(("team".into(), "a=b".into()))
        );
        assert!(parse_metadata("=x").is_err());
        assert!(parse_metadata("novalue").is_err());
    }

    #[test]
    fn ids_stay_one_path_segment() {
        assert_eq!(monitor_path("mon_1", "/runs"), "/v1/monitors/mon_1/runs");
        assert_eq!(monitor_path("../stats", ""), "/v1/monitors/..%2Fstats");
        assert_eq!(monitor_path("a b", ""), "/v1/monitors/a%20b");
    }

    #[test]
    fn calls_put_only_set_options_in_the_query() {
        let c = results_call("mon_1", Some(5), None, Some(4));
        assert_eq!(c.method, Method::GET);
        assert_eq!(c.path, "/v1/monitors/mon_1/results");
        assert_eq!(
            c.query,
            vec![
                ("limit".into(), "5".into()),
                ("min_importance".into(), "4".into())
            ]
        );
        let c = list_call(Some("active".into()), None, None, Some("c1".into()));
        assert_eq!(
            c.query,
            vec![
                ("status".into(), "active".into()),
                ("cursor".into(), "c1".into())
            ]
        );
    }

    #[test]
    fn pause_and_resume_patch_the_status() {
        let c = status_call("mon_1", "paused");
        assert_eq!(c.method, Method::PATCH);
        assert_eq!(c.body, Some(json!({"status": "paused"})));
    }

    #[test]
    fn a_missing_key_is_an_authentication_error_with_a_login_hint() {
        let e = key_required();
        assert_eq!(e.exit_code(), crate::api::exit_code::AUTH);
        assert!(e.hint(false).unwrap().contains("keenable login"));
    }

    #[test]
    fn schedule_lines_read_naturally() {
        assert_eq!(
            schedule_line(&json!({"type": "interval", "value": "1d"})),
            "every 1d"
        );
        assert_eq!(
            schedule_line(&json!({"type": "cron", "value": "0 9 * * *", "timezone": null})),
            "cron 0 9 * * * (UTC)"
        );
        assert_eq!(schedule_line(&Value::Null), "only when triggered");
    }

    #[test]
    fn lists_join_or_stay_out() {
        assert_eq!(
            joined(&json!(["a.com", "b.com"])).as_deref(),
            Some("a.com, b.com")
        );
        assert_eq!(joined(&json!([])), None);
        assert_eq!(joined(&Value::Null), None);
    }
}
