use std::ffi::OsString;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::bounded;
use crate::handoff::{REMOTE, remote_program, search_dirs};
use crate::proto::OpenIn;

const LIST_LIMIT: Duration = Duration::from_secs(10);
const OPEN_LIMIT: Duration = Duration::from_secs(40);
const CLOSE_WITHIN: Duration = Duration::from_secs(15);
const MAX_DIR_BYTES: usize = 4096;
const MAX_FOLDERS: usize = 500;

#[derive(Deserialize)]
struct Report {
    ok: bool,
    message: String,
    #[serde(default)]
    pane: Option<OpenedPane>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct OpenedPane {
    pub session: String,
    pub window: String,
    pub block: String,
}

pub struct Opened {
    pub message: String,
    pub pane: Option<OpenedPane>,
}

pub fn folders() -> Result<Vec<String>, String> {
    let output = remote(&["open", "--list", "--json"], LIST_LIMIT)?;
    if !output.status.success() {
        return Err(failure(&output, "couldn't open a session"));
    }
    let mut folders: Vec<String> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{REMOTE} listed folders in a form magi can't read: {error}"))?;
    folders.truncate(MAX_FOLDERS);
    Ok(folders)
}

static OPENING: AtomicBool = AtomicBool::new(false);

struct Opening;

impl Opening {
    fn start() -> Result<Self, String> {
        if OPENING.swap(true, Ordering::SeqCst) {
            return Err("a session is already being opened on that machine".into());
        }
        Ok(Self)
    }
}

impl Drop for Opening {
    fn drop(&mut self) {
        OPENING.store(false, Ordering::SeqCst);
    }
}

pub fn open(place: &OpenIn, codex: bool, model: Option<&str>) -> Result<Opened, String> {
    if let OpenIn::Folder(dir) = place {
        check(dir)?;
    }
    if let Some(model) = model.filter(|model| !crate::proto::is_model_name(model)) {
        return Err(format!("{model:?} isn't a model name"));
    }
    let _opening = Opening::start()?;
    let mut args = vec!["open", "--json"];
    if codex {
        args.push("--codex");
    }
    if let Some(model) = model {
        args.extend(["--model", model]);
    }
    match place {
        OpenIn::Folder(dir) => args.push(dir),
        OpenIn::Chats => args.push("--chat"),
    }
    let output = remote(&args, OPEN_LIMIT)?;
    report(&output)
}

fn report(output: &Output) -> Result<Opened, String> {
    let report = std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|text| text.lines().last())
        .and_then(|line| serde_json::from_str::<Report>(line).ok());
    match report {
        Some(report) if report.ok => Ok(Opened {
            message: report.message,
            pane: report.pane,
        }),
        Some(report) => Err(format!("couldn't open a session: {}", report.message)),
        None => Err(failure(output, "couldn't open a session")),
    }
}

pub fn adopt(dir: &str, session: &str) -> Result<String, String> {
    check(dir)?;
    let _opening = Opening::start()?;
    let output = remote(&["open", "--json", "--resume", session, dir], OPEN_LIMIT)?;
    report(&output).map(|opened| opened.message)
}

fn check(dir: &str) -> Result<(), String> {
    if !dir.starts_with('/') || dir.len() > MAX_DIR_BYTES || dir.chars().any(char::is_control) {
        return Err(format!("{dir:?} isn't a folder path"));
    }
    Ok(())
}

pub fn close_forward(session: &str) -> Result<String, String> {
    let output = remote(&["forwards", "close", session], CLOSE_WITHIN)?;
    if output.status.success() {
        return Ok(format!(
            "Closed the forward for session {session}, which moved on"
        ));
    }
    Err(failure(&output, "couldn't close a forward"))
}

fn remote(args: &[&str], limit: Duration) -> Result<Output, String> {
    let program = remote_program()?;
    let path: OsString = std::env::join_paths(search_dirs()).unwrap_or_default();
    bounded::output(Command::new(program).args(args).env("PATH", path), limit)
        .map_err(|error| format!("couldn't run {REMOTE}: {error}"))
}

fn failure(output: &Output, what: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("it failed")
        .trim_start_matches("magi: ");
    format!("{what}: {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_single_line_paths_are_opened() {
        assert!(check("/home/me/code/api").is_ok());
        for bad in ["relative/path", "/home/me\n/evil", ""] {
            assert!(check(bad).is_err(), "{bad:?}");
        }
        assert!(check(&format!("/{}", "a".repeat(MAX_DIR_BYTES))).is_err());
    }
}
