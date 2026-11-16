use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::proto::{Child, ChildKind, ChildState};

const READ_EVERY: Duration = Duration::from_secs(2);
const START_TOLERANCE_SECS: i64 = 2;
const MAX_LABEL_CHARS: usize = 60;
const FINISHED: [&str; 2] = ["collected", "gone"];
const FORGET_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_RECORD_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
struct Record {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    state: String,
    caller_pid: Option<i32>,
    caller_started: Option<i64>,
}

pub struct Tasks {
    dir: PathBuf,
    read_at: Option<Instant>,
    by_caller: HashMap<(i32, i64), Vec<Child>>,
}

impl Tasks {
    pub fn for_this_user() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        Some(Self::new(Path::new(&home).join(".cielago").join("tasks")))
    }

    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            read_at: None,
            by_caller: HashMap::new(),
        }
    }

    pub fn started_by(&mut self, pid: i32, now: Instant) -> Vec<Child> {
        if self
            .read_at
            .is_none_or(|at| now.duration_since(at) >= READ_EVERY)
        {
            self.read_at = Some(now);
            self.by_caller = read(&self.dir);
        }
        let Some(started) = super::registry::process_start_secs(pid) else {
            return Vec::new();
        };
        self.by_caller
            .iter()
            .filter(|((caller, caller_started), _)| {
                *caller == pid && (caller_started - started).abs() <= START_TOLERANCE_SECS
            })
            .flat_map(|(_, children)| children.iter().cloned())
            .collect()
    }
}

fn read(dir: &Path) -> HashMap<(i32, i64), Vec<Child>> {
    let mut by_caller: HashMap<(i32, i64), Vec<Child>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return by_caller;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let recent = entry.metadata().is_ok_and(|meta| {
            meta.len() <= MAX_RECORD_BYTES
                && meta
                    .modified()
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age < FORGET_AFTER)
        });
        if !recent {
            continue;
        }
        let Some(record) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        else {
            continue;
        };
        let id = path
            .file_stem()
            .map(|stem| format!("task:{}", stem.to_string_lossy()))
            .unwrap_or_default();
        if let Some((caller, child)) = child(id, record) {
            by_caller.entry(caller).or_default().push(child);
        }
    }
    by_caller
}

fn child(id: String, record: Record) -> Option<((i32, i64), Child)> {
    let caller = (record.caller_pid?, record.caller_started?);
    if FINISHED.contains(&record.state.as_str()) {
        return None;
    }
    let what = [&record.name, &record.command, &record.prompt]
        .into_iter()
        .find(|text| !text.trim().is_empty())
        .map_or(record.kind.as_str(), String::as_str);
    let first_line = what.lines().next().unwrap_or_default().trim();
    let mut label: String = first_line.chars().take(MAX_LABEL_CHARS).collect();
    if first_line.chars().count() > MAX_LABEL_CHARS {
        label.push('…');
    }
    let state = match record.state.as_str() {
        "" => "running",
        state => state,
    };
    Some((
        caller,
        Child {
            id,
            kind: ChildKind::Task,
            label,
            detail: format!("on {} · {state}", record.host),
            state: ChildState::Running,
            secs: 0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(json: &str) -> Record {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_task_is_named_by_what_it_runs_and_says_where_and_how_it_is_doing() {
        let (caller, task) = child("task:t1".into(), record(
            r#"{"kind":"run","host":"homelab","name":"","command":"cargo test --workspace\nsecond","state":"running","caller_pid":42,"caller_started":1000}"#,
        ))
        .unwrap();
        assert_eq!(caller, (42, 1000));
        assert_eq!(
            task,
            Child {
                id: "task:t1".into(),
                kind: ChildKind::Task,
                label: "cargo test --workspace".into(),
                detail: "on homelab · running".into(),
                state: ChildState::Running,
                secs: 0,
            }
        );
        let (_, delegated) = child("task:t1".into(), record(
            r#"{"kind":"delegate","host":"homelab","prompt":"Fix the flaky login test","caller_pid":42,"caller_started":1000}"#,
        ))
        .unwrap();
        assert_eq!(delegated.label, "Fix the flaky login test");
        assert_eq!(delegated.detail, "on homelab · running");
    }

    #[test]
    fn finished_tasks_and_ones_started_from_the_command_line_are_not_nested() {
        assert!(
            child(
                "task:t1".into(),
                record(r#"{"state":"collected","caller_pid":1,"caller_started":1}"#)
            )
            .is_none()
        );
        assert!(
            child(
                "task:t1".into(),
                record(r#"{"state":"gone","caller_pid":1,"caller_started":1}"#)
            )
            .is_none()
        );
        assert!(
            child(
                "task:t1".into(),
                record(r#"{"state":"running","command":"make"}"#)
            )
            .is_none()
        );
    }

    #[test]
    fn tasks_attach_only_to_the_process_that_started_them() {
        let dir = std::env::temp_dir().join(format!("magi-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let me = std::process::id() as i32;
        let started = super::super::registry::process_start_secs(me).unwrap();
        let write = |name: &str, pid: i32, at: i64| {
            std::fs::write(
                dir.join(format!("{name}.json")),
                format!(r#"{{"kind":"run","host":"homelab","command":"{name}","state":"running","caller_pid":{pid},"caller_started":{at}}}"#),
            )
            .unwrap();
        };
        write("mine", me, started);
        write("reused-pid", me, started - 3600);
        write("someone-else", me + 1, started);
        std::fs::write(dir.join("mine.log"), "not a record").unwrap();
        let mut tasks = Tasks::new(dir.clone());
        let labels: Vec<String> = tasks
            .started_by(me, Instant::now())
            .into_iter()
            .map(|child| child.label)
            .collect();
        assert_eq!(labels, ["mine"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
