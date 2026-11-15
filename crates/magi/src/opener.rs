use std::ffi::OsString;
use std::process::{Command, Output};
use std::time::Duration;

use serde::Deserialize;

use crate::bounded;
use crate::handoff::{find, search_dirs};

const LIST_LIMIT: Duration = Duration::from_secs(10);
const OPEN_LIMIT: Duration = Duration::from_secs(40);
const MAX_DIR_BYTES: usize = 4096;
const MAX_FOLDERS: usize = 500;

#[derive(Deserialize)]
struct Report {
    ok: bool,
    message: String,
}

pub fn folders() -> Result<Vec<String>, String> {
    let output = cielago(&["open", "--list", "--json"], LIST_LIMIT)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    let mut folders: Vec<String> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cielago listed folders in a form magi can't read: {error}"))?;
    folders.truncate(MAX_FOLDERS);
    Ok(folders)
}

pub fn open(dir: &str, codex: bool) -> Result<String, String> {
    check(dir)?;
    let mut args = vec!["open", "--json"];
    if codex {
        args.push("--codex");
    }
    args.push(dir);
    let output = cielago(&args, OPEN_LIMIT)?;
    report(&output)
}

fn report(output: &Output) -> Result<String, String> {
    let report = std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|text| text.lines().last())
        .and_then(|line| serde_json::from_str::<Report>(line).ok());
    match report {
        Some(report) if report.ok => Ok(report.message),
        Some(report) => Err(format!("couldn't open a session: {}", report.message)),
        None => Err(failure(output)),
    }
}

pub fn adopt(dir: &str, session: &str) -> Result<String, String> {
    check(dir)?;
    let output = cielago(&["open", "--json", "--resume", session, dir], OPEN_LIMIT)?;
    report(&output)
}

fn check(dir: &str) -> Result<(), String> {
    if !dir.starts_with('/') || dir.len() > MAX_DIR_BYTES || dir.chars().any(char::is_control) {
        return Err(format!("{dir:?} isn't a folder path"));
    }
    Ok(())
}

fn cielago(args: &[&str], limit: Duration) -> Result<Output, String> {
    let dirs = search_dirs();
    let cielago = find("cielago", &dirs)
        .ok_or("cielago isn't installed on that machine: see github.com/felitrejos/magi")?;
    let path: OsString = std::env::join_paths(&dirs).unwrap_or_default();
    bounded::output(Command::new(cielago).args(args).env("PATH", path), limit)
        .map_err(|error| format!("couldn't run cielago: {error}"))
}

fn failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("cielago failed")
        .trim_start_matches("cielago: ");
    format!("couldn't open a session: {reason}")
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
