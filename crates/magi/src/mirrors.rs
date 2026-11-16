use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::bounded;
use crate::children;
use crate::proto::{Agent, ChildKind, ChildState};
use crate::sessions::Snapshot;
use crate::sessions::rex::Location;

const REX_LIMIT: Duration = Duration::from_secs(3);
const MAX_LABEL_CHARS: usize = 60;
const SHELL: &str = "com.superlogical.terminal.shell";

pub struct Mirror {
    pub key: (String, String),
    pane: Location,
    parent_window: String,
    transcript: PathBuf,
    label: String,
}

pub fn wanted(snapshot: &Snapshot, opened: &mut HashSet<(String, String)>) -> Vec<Mirror> {
    let listed: HashSet<(String, String)> = snapshot
        .sessions
        .iter()
        .flat_map(|session| {
            session
                .children
                .iter()
                .map(|child| (session.id.clone(), child.id.clone()))
        })
        .collect();
    opened.retain(|key| listed.contains(key));
    let mut wanted = Vec::new();
    for session in snapshot
        .sessions
        .iter()
        .filter(|session| session.agent == Agent::ClaudeCode)
    {
        let (Some(pane), Some(rex), Some((_, parent))) = (
            &session.pane,
            snapshot.panes.get(&session.id),
            snapshot.transcripts.get(&session.id),
        ) else {
            continue;
        };
        for child in session
            .children
            .iter()
            .filter(|child| child.kind == ChildKind::Subagent && child.state == ChildState::Running)
        {
            let key = (session.id.clone(), child.id.clone());
            if opened.contains(&key) {
                continue;
            }
            let Some(transcript) = children::subagent_transcript(parent, &child.id) else {
                continue;
            };
            opened.insert(key.clone());
            wanted.push(Mirror {
                key,
                pane: rex.clone(),
                parent_window: pane.window.clone(),
                transcript,
                label: label(&child.label),
            });
        }
    }
    wanted
}

fn label(child: &str) -> String {
    let mut label: String = format!("↳ {}", child.trim())
        .chars()
        .take(MAX_LABEL_CHARS)
        .collect();
    if child.trim().chars().count() + 2 > MAX_LABEL_CHARS {
        label.push('…');
    }
    label
}

#[derive(Deserialize)]
struct Windows {
    windows: Vec<Window>,
}

#[derive(Deserialize)]
struct Window {
    window_id: String,
    #[serde(default)]
    label: String,
}

#[derive(Deserialize)]
struct Created {
    window_id: Option<String>,
}

pub fn open(mirror: &Mirror, magi: &Path) -> Result<(), String> {
    let pane = &mirror.pane;
    let listing = rex(pane, &["window", "ls", "-s", &pane.session, "--json"])?;
    let windows: Windows = serde_json::from_slice(&listing)
        .map_err(|error| format!("Rex listed its tabs in a form magi can't read: {error}"))?;
    if windows
        .windows
        .iter()
        .any(|window| window.label == mirror.label)
    {
        return Ok(());
    }
    let after_parent = windows
        .windows
        .iter()
        .skip_while(|window| window.window_id != mirror.parent_window)
        .nth(1)
        .map(|window| window.window_id.clone());
    let request = json!({
        "focus": false,
        "window_label": mirror.label,
        "layout": {"block": {
            "flavor": SHELL,
            "label": mirror.label,
            "options": {"command": [magi.to_string_lossy(), "follow", mirror.transcript.to_string_lossy()]},
        }},
    });
    let created = rex(
        pane,
        &[
            "api",
            "call",
            "-s",
            &pane.session,
            "session.new_window",
            &request.to_string(),
        ],
    )?;
    let created: Created = serde_json::from_slice(&created)
        .map_err(|error| format!("Rex answered in a form magi can't read: {error}"))?;
    if let (Some(window), Some(before)) = (created.window_id, after_parent) {
        let request = json!({"window_id": window, "before_window_id": before});
        rex(
            pane,
            &[
                "api",
                "call",
                "-s",
                &pane.session,
                "session.move_window",
                &request.to_string(),
            ],
        )?;
    }
    Ok(())
}

fn rex(pane: &Location, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = bounded::output(&mut pane.rex("2s", args), REX_LIMIT)
        .map_err(|error| format!("can't run rex: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_tab_is_labelled_after_its_subagent_and_kept_short() {
        assert_eq!(
            label("Explore: find auth code"),
            "↳ Explore: find auth code"
        );
        let long = "x".repeat(80);
        let shown = label(&long);
        assert_eq!(shown.chars().count(), MAX_LABEL_CHARS + 1);
        assert!(shown.ends_with('…'));
    }
}
