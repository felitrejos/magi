use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::rex::Location;
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

#[derive(Default)]
struct Shared {
    statuses: HashMap<String, Status>,
    asked: HashSet<String>,
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

    pub fn status(&self, location: &Location) -> Option<Status> {
        let first_time = {
            let mut shared = self.lock();
            if let Some(status) = shared.statuses.get(&location.block) {
                return Some(status.clone());
            }
            shared.asked.insert(location.block.clone())
        };
        if !first_time {
            return None;
        }
        let output = bounded::output(
            &mut location.rex(
                "1s",
                &[
                    "block",
                    "call",
                    "-s",
                    &location.session,
                    "-b",
                    &location.block,
                    "program_status",
                    "{}",
                ],
            ),
            ASK_WITHIN,
        )
        .ok()?;
        let reply: Value = serde_json::from_slice(&output.stdout).ok()?;
        let status = reply["records"].as_array()?.iter().find_map(|record| {
            status(&json!({"id": record["id"].as_str().unwrap_or(""), "record": record}))
        })?;
        let mut shared = self.lock();
        Some(
            shared
                .statuses
                .entry(location.block.clone())
                .or_insert(status)
                .clone(),
        )
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
            shared.asked.clear();
            drop(shared);
            thread::sleep(RECONNECT_AFTER);
        }
    }

    fn read(&self, line: &str) {
        let Some((block, name, payload)) = block_event(line) else {
            return;
        };
        if !WAKING.contains(&name.as_str()) {
            return;
        }
        let mut shared = self.lock();
        match name.as_str() {
            "program_status_changed" => {
                if let Some(status) = status(&payload) {
                    shared.statuses.insert(block, status);
                }
            }
            "program_status_removed" | "child_exited" => {
                shared.statuses.remove(&block);
            }
            _ => {}
        }
        shared.woken = true;
        drop(shared);
        self.shared.1.notify_all();
    }
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
            .map(|text| {
                text.chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_ACTIVITY_CHARS)
                    .collect::<String>()
            })
            .filter(|text| !text.trim().is_empty()),
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
