use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::rex::{Location, running, running_child};
use crate::proto::{SessionState, Waiting};
use crate::{bounded, handoff};

const RECONNECT_AFTER: Duration = Duration::from_secs(5);
const ASK_WITHIN: Duration = Duration::from_secs(2);
const LOOK_FOR_REX_AFTER: Duration = Duration::from_secs(60);
const AGENTS: [&str; 2] = ["claude-code", "codex"];
const MAX_ACTIVITY_CHARS: usize = 160;
const WAKING: [&str; 6] = [
    "program_status_changed",
    "program_status_removed",
    "title_changed",
    "bell",
    "desktop_notification",
    "child_exited",
];
const LAYOUT: [&str; 5] = [
    "session_view_changed",
    "block_created",
    "block_closed",
    "window_created",
    "window_closed",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    state: String,
    kind: Option<String>,
    message: Option<String>,
}

impl Status {
    pub fn activity(&self) -> Option<String> {
        matches!(self.state.as_str(), "working" | "blocked")
            .then(|| self.message.clone())
            .flatten()
    }

    pub fn apply(&self, state: SessionState) -> SessionState {
        match (self.state.as_str(), state) {
            ("blocked", waiting @ SessionState::Waiting(_)) => waiting,
            ("blocked", _) => SessionState::Waiting(match self.kind.as_deref() {
                Some("question") => Waiting::Input(self.message.clone().unwrap_or_default()),
                _ => Waiting::Permission(self.message.clone()),
            }),
            ("working", working @ SessionState::Working { .. }) => working,
            ("working", _) => SessionState::Working { tool: None },
            ("idle" | "done" | "error", _) => SessionState::Idle,
            (_, state) => state,
        }
    }
}

pub const REX_API: u64 = 16;
pub const REX_RELEASE: &str = "0.1.0-preview.20";

#[derive(Default)]
struct Shared {
    outdated: Option<String>,
    statuses: HashMap<String, Status>,
    children: HashMap<String, HashMap<String, String>>,
    asked: HashSet<String>,
    listed: HashSet<String>,
    rearranged: HashSet<String>,
    woken: bool,
}

#[derive(Clone, Default)]
pub struct Events {
    shared: Arc<(Mutex<Shared>, Condvar)>,
}

impl Events {
    pub fn listen() -> Self {
        let events = Self::default();
        let listener = events.clone();
        let _ = thread::Builder::new()
            .name("rex events".into())
            .spawn(move || listener.follow());
        events
    }

    #[cfg(test)]
    fn known(&self, block: &str) -> Option<Status> {
        self.lock().statuses.get(block).cloned()
    }

    pub fn outdated(&self) -> Option<String> {
        self.lock().outdated.clone()
    }

    pub fn status(&self, location: &Location) -> Option<Status> {
        if self.lock().asked.insert(location.block.clone()) {
            self.list(location);
        }
        self.lock().statuses.get(&location.block).cloned()
    }

    pub fn running_children(&self, location: &Location) -> Option<HashMap<String, String>> {
        let shared = self.lock();
        shared.listed.contains(&location.block).then(|| {
            shared
                .children
                .get(&location.block)
                .cloned()
                .unwrap_or_default()
        })
    }

    fn list(&self, location: &Location) {
        let Ok(records) = location.program_status() else {
            return;
        };
        let mut shared = self.lock();
        let block = &location.block;
        for record in &records {
            if let Some(child) = running_child(record) {
                shared
                    .children
                    .entry(block.clone())
                    .or_default()
                    .insert(child, doing(record));
            } else if let Some(status) =
                status(&json!({"id": record["id"].as_str().unwrap_or(""), "record": record}))
            {
                shared.statuses.entry(block.clone()).or_insert(status);
            }
        }
        shared.listed.insert(block.clone());
    }

    pub fn rearranged(&self) -> HashSet<String> {
        std::mem::take(&mut self.lock().rearranged)
    }

    pub fn wait(&self, longest: Duration) -> bool {
        let (lock, changed) = &*self.shared;
        let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut guard, _) = changed
            .wait_timeout_while(guard, longest, |shared| !shared.woken)
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut guard.woken)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn follow(&self) {
        loop {
            let Some(rex) = handoff::find("rex", &handoff::search_dirs()) else {
                thread::sleep(LOOK_FOR_REX_AFTER);
                continue;
            };
            let outdated = outdated(&rex);
            let stale = outdated.is_some();
            self.lock().outdated = outdated;
            if stale {
                thread::sleep(LOOK_FOR_REX_AFTER);
                continue;
            }
            let child = Command::new(rex)
                .args(["--autostart=false", "events", "--all", "--json"])
                .env_remove("REX_SERVER")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = child {
                if let Some(stdout) = child.stdout.take() {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        self.read(&line);
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            let mut shared = self.lock();
            shared.statuses.clear();
            shared.children.clear();
            shared.asked.clear();
            shared.listed.clear();
            drop(shared);
            thread::sleep(RECONNECT_AFTER);
        }
    }

    fn read(&self, line: &str) {
        if let Some(session) = layout_event(line) {
            self.lock().rearranged.insert(session);
            return;
        }
        let Some((block, name, payload)) = block_event(line) else {
            return;
        };
        if !WAKING.contains(&name.as_str()) {
            return;
        }
        let mut shared = self.lock();
        match name.as_str() {
            "program_status_changed" => {
                let id = payload["id"].as_str().unwrap_or_default().to_string();
                if id.is_empty() {
                    if let Some(status) = status(&payload) {
                        shared.statuses.insert(block, status);
                    }
                } else if running(&payload["record"]) {
                    let doing = doing(&payload["record"]);
                    shared.children.entry(block).or_default().insert(id, doing);
                } else if let Some(children) = shared.children.get_mut(&block) {
                    children.remove(&id);
                }
            }
            "program_status_removed" => {
                for id in payload["ids"].as_array().into_iter().flatten() {
                    match id.as_str() {
                        Some("") => {
                            shared.statuses.remove(&block);
                        }
                        Some(id) => {
                            if let Some(children) = shared.children.get_mut(&block) {
                                children.remove(id);
                            }
                        }
                        None => {}
                    }
                }
            }
            "child_exited" => {
                shared.statuses.remove(&block);
                shared.children.remove(&block);
            }
            _ => {}
        }
        shared.woken = true;
        drop(shared);
        self.shared.1.notify_all();
    }
}

fn outdated(rex: &std::path::Path) -> Option<String> {
    let output = bounded::output(
        Command::new(rex)
            .args(["--autostart=false", "api", "call", "server.status"])
            .env_remove("REX_SERVER")
            .stdin(Stdio::null()),
        ASK_WITHIN,
    )
    .ok()?;
    let status: Value = serde_json::from_slice(&output.stdout).ok()?;
    too_old(status["api_revision"].as_u64()?)
}

fn too_old(revision: u64) -> Option<String> {
    (revision < REX_API).then(|| {
        format!(
            "Rex on this machine is at API revision {revision}; magi needs Rex {REX_RELEASE} \
             (revision {REX_API}). Update Rex and restart its server."
        )
    })
}

fn layout_event(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let kind = value["payload"]["event"]["type"].as_str()?;
    (value["type"] == "session_event" && LAYOUT.contains(&kind))
        .then(|| value["payload"]["session_id"].as_str().map(str::to_string))?
}

fn block_event(line: &str) -> Option<(String, String, Value)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let event = &value["payload"]["event"];
    if value["type"] != "session_event" || event["type"] != "block_event" {
        return None;
    }
    let event = &event["payload"];
    Some((
        event["block_id"].as_str()?.to_string(),
        event["name"].as_str()?.to_string(),
        event["payload"].clone(),
    ))
}

fn doing(record: &Value) -> String {
    clean(record["msg"].as_str().unwrap_or_default())
}

fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_ACTIVITY_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn status(payload: &Value) -> Option<Status> {
    let record = &payload["record"];
    let app = record["effective_app"].as_str()?;
    if payload["id"] != "" || !AGENTS.contains(&app) {
        return None;
    }
    Some(Status {
        state: record["state"].as_str()?.to_string(),
        kind: record["kind"].as_str().map(str::to_string),
        message: record["msg"]
            .as_str()
            .map(clean)
            .filter(|text| !text.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, payload: &str) -> String {
        format!(
            r#"{{"type":"session_event","payload":{{"session_id":"s","event":{{"type":"block_event","payload":{{"block_id":"b1","creator_name":"com.superlogical.terminal","name":"{name}","payload":{payload}}}}}}}}}"#
        )
    }

    #[test]
    fn an_agents_own_status_report_is_kept_per_pane_and_wakes_the_reader() {
        let events = Events::default();
        events.read(&event(
            "program_status_changed",
            r#"{"id":"","record":{"state":"blocked","kind":"permission","app":"claude-code","effective_app":"claude-code","msg":"run cargo test?","updated_at":"x"},"reason":"report"}"#,
        ));
        assert!(events.wait(Duration::ZERO));
        assert!(!events.wait(Duration::ZERO));
        let status = events.known("b1").unwrap();
        assert_eq!(
            status.apply(SessionState::Idle),
            SessionState::Waiting(Waiting::Permission(Some("run cargo test?".into())))
        );
        assert_eq!(status.activity().as_deref(), Some("run cargo test?"));
        events.read(&event(
            "program_status_changed",
            r#"{"id":"build","record":{"state":"working","effective_app":"cargo","updated_at":"x"},"reason":"report"}"#,
        ));
        assert_eq!(events.known("b1"), Some(status));
        events.read(&event(
            "program_status_removed",
            r#"{"ids":[""],"reason":"prompt"}"#,
        ));
        assert_eq!(events.known("b1"), None);
    }

    #[test]
    fn a_panes_running_subagents_follow_its_child_records() {
        let events = Events::default();
        let pane = Location::in_test("s", "b1");
        assert_eq!(events.running_children(&pane), None);
        events.lock().listed.insert("b1".into());
        events.read(&event(
            "program_status_changed",
            r#"{"id":"a1","record":{"state":"working","effective_app":"claude-code","msg":"Read(src/main.rs)"},"reason":"report"}"#,
        ));
        events.read(&event(
            "program_status_changed",
            r#"{"id":"a2","record":{"state":"working","effective_app":"claude-code"},"reason":"report"}"#,
        ));
        events.read(&event(
            "program_status_changed",
            r#"{"id":"a2","record":{"state":"done","effective_app":"claude-code"},"reason":"report"}"#,
        ));
        events.read(&event(
            "program_status_changed",
            r#"{"id":"","record":{"state":"working","effective_app":"claude-code"},"reason":"report"}"#,
        ));
        assert_eq!(
            events.running_children(&pane),
            Some(HashMap::from([(
                "a1".to_string(),
                "Read(src/main.rs)".to_string()
            )]))
        );
        events.read(&event(
            "program_status_removed",
            r#"{"ids":["a1"],"reason":"report"}"#,
        ));
        assert_eq!(events.running_children(&pane), Some(HashMap::new()));
        assert!(events.known("b1").is_some());
    }

    #[test]
    fn a_layout_change_marks_its_session_for_a_fresh_pane_listing() {
        let events = Events::default();
        events.read(
            r#"{"type":"session_event","payload":{"session_id":"s9","event":{"type":"session_view_changed","payload":{}}}}"#,
        );
        events.read(&event("title_changed", "{}"));
        assert_eq!(events.rearranged(), HashSet::from(["s9".to_string()]));
        assert!(events.rearranged().is_empty());
    }

    #[test]
    fn an_older_rex_than_the_one_magi_needs_is_a_notice() {
        assert!(too_old(REX_API - 1).unwrap().contains(REX_RELEASE));
        assert_eq!(too_old(REX_API), None);
    }

    #[test]
    fn working_and_idle_reports_settle_the_state_but_keep_what_the_transcript_knows() {
        let report = |state: &str| Status {
            state: state.into(),
            kind: None,
            message: Some("reading main.rs".into()),
        };
        let reading = SessionState::Working {
            tool: Some("Read".into()),
        };
        assert_eq!(report("working").apply(reading.clone()), reading);
        assert_eq!(
            report("working").apply(SessionState::Idle),
            SessionState::Working { tool: None }
        );
        assert_eq!(report("idle").apply(reading), SessionState::Idle);
        assert_eq!(report("idle").activity(), None);
        let question = SessionState::Waiting(Waiting::Input("1. yes 2. no".into()));
        assert_eq!(report("blocked").apply(question.clone()), question);
    }
}
