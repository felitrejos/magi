use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::{base64, view};
use crate::children;
use crate::conversation::Reader;
use crate::proto::{Agent, Said, StepState};
use crate::sessions::rex::Location;

const EVERY: Duration = Duration::from_millis(500);
const ASK_REX_EVERY: Duration = Duration::from_secs(2);
const REPORTED_WITHIN: Duration = Duration::from_secs(15);
const LINGER: Duration = Duration::from_secs(10);
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const PROMPT: &str = "\x1b[48;5;237m";
const RESET: &str = "\x1b[0m";
const APP: &str = "claude-code";
const MAX_TITLE_CHARS: usize = 40;
const MAX_MSG_CHARS: usize = 300;

pub fn follow(transcript: &Path, session: &str, block: &str) -> io::Result<()> {
    let parent = Location::here(session, block)
        .ok_or_else(|| io::Error::other("magi follow needs the rex command"))?;
    run(transcript, &parent)
}

fn run(transcript: &Path, parent: &Location) -> io::Result<()> {
    let id = transcript
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("agent-"))
        .unwrap_or_default()
        .to_string();
    let started = Instant::now();
    let mut alive = Alive::default();
    let mut reader = Reader::default();
    let mut printed = 0;
    let mut title = None;
    let mut reported = String::new();
    let mut out = io::stdout().lock();
    loop {
        let meta = children::subagent_meta(transcript);
        if let Some(meta) = &meta
            && title.is_none()
        {
            write!(out, "{BOLD}↳ {}{RESET}\n\n", meta.description)?;
            title = Some(meta.description.clone());
        }
        let said = reader
            .read(transcript, Agent::ClaudeCode)
            .unwrap_or_default();
        for item in said.iter().skip(printed) {
            out.write_all(shown(item, width()).as_bytes())?;
        }
        printed = said.len();
        let finished =
            meta.is_some_and(|meta| meta.finished) || !alive.check(parent, &id, started.elapsed());
        let report = status(
            if finished { "done" } else { "working" },
            title.as_deref().unwrap_or_default(),
            said.iter().rev().find_map(latest).unwrap_or_default(),
        );
        if report != reported {
            out.write_all(report.as_bytes())?;
            reported = report;
        }
        out.flush()?;
        if finished {
            write!(out, "\n{DIM}done · this tab closes in a moment{RESET}\n")?;
            out.flush()?;
            thread::sleep(LINGER);
            return Ok(());
        }
        thread::sleep(EVERY);
    }
}

#[derive(Default)]
struct Alive {
    seen: bool,
    asked: Option<Instant>,
    running: bool,
}

impl Alive {
    fn check(&mut self, parent: &Location, id: &str, waited: Duration) -> bool {
        if self
            .asked
            .is_some_and(|asked| asked.elapsed() < ASK_REX_EVERY)
        {
            return self.running;
        }
        self.asked = Some(Instant::now());
        self.running = match parent.running_children() {
            Ok(ids) => still_running(&ids, id, &mut self.seen, waited),
            Err(_) => false,
        };
        self.running
    }
}

fn still_running(reported: &[String], id: &str, seen: &mut bool, waited: Duration) -> bool {
    if reported.iter().any(|known| known == id) {
        *seen = true;
        return true;
    }
    !*seen && waited < REPORTED_WITHIN
}

fn latest(said: &Said) -> Option<&str> {
    match said {
        Said::Tool { detail, .. } => detail.lines().next(),
        Said::Reply(text) => text.lines().find(|line| !line.trim().is_empty()),
        Said::You(_) | Said::Plan(_) => None,
    }
}

fn status(state: &str, title: &str, msg: &str) -> String {
    let field = |text: &str, chars: usize| {
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(chars)
            .collect();
        base64::encode(clean.trim().as_bytes())
    };
    format!(
        "\x1b]7501;state={state}:app={APP}:title={}:msg={}\x1b\\",
        field(title, MAX_TITLE_CHARS),
        field(msg, MAX_MSG_CHARS)
    )
}

fn shown(said: &Said, width: usize) -> String {
    match said {
        Said::You(text) => {
            let lines: Vec<String> = text
                .trim()
                .lines()
                .map(|line| format!("{PROMPT} > {line} {RESET}"))
                .collect();
            format!("{}\n\n", lines.join("\n"))
        }
        Said::Reply(text) => {
            let body = view::markdown_ansi(text, width.saturating_sub(2));
            let mut lines = body.lines();
            let first = lines.next().unwrap_or_default();
            let rest: String = lines.map(|line| format!("\n  {line}")).collect();
            format!("⏺ {first}{rest}\n\n")
        }
        Said::Tool {
            name,
            detail,
            change,
            ..
        } => {
            let detail = detail.lines().next().unwrap_or_default();
            let mut shown = format!("{GREEN}⏺{RESET} {BOLD}{name}{RESET}({detail})\n");
            if let Some(change) = change {
                shown.push_str(&format!(
                    "  {DIM}⎿  +{} −{}{RESET}\n",
                    change.added, change.removed
                ));
            }
            shown.push('\n');
            shown
        }
        Said::Plan(steps) => {
            let lines: String = steps
                .iter()
                .map(|step| {
                    let mark = match step.state {
                        StepState::Done => "☒",
                        StepState::Doing => "◼",
                        StepState::Todo => "☐",
                    };
                    format!("  {DIM}{mark}{RESET} {}\n", step.text)
                })
                .collect();
            format!("⏺ {BOLD}Plan{RESET}\n{lines}\n")
        }
    }
}

fn width() -> usize {
    ratatui::crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_reports_itself_to_rex_as_claude_code_with_its_latest_step() {
        let working = status("working", "Explore: find auth", "rg \"login\" src");
        assert!(working.starts_with("\x1b]7501;state=working:app=claude-code:title="));
        assert!(working.ends_with("\x1b\\"));
        let msg = working
            .split(":msg=")
            .nth(1)
            .unwrap()
            .trim_end_matches("\x1b\\");
        assert_eq!(msg, base64::encode("rg \"login\" src".as_bytes()));
        let long = status("done", &"t".repeat(500), &"m".repeat(5000));
        assert!(long.len() < 4096, "{}", long.len());
        let tool = Said::Tool {
            name: "Bash".into(),
            detail: "cargo test\nsecond".into(),
            change: None,
            running_since: None,
        };
        assert_eq!(latest(&tool), Some("cargo test"));
    }

    #[test]
    fn a_subagent_counts_as_running_while_its_parent_reports_it() {
        let mut seen = false;
        let none: Vec<String> = Vec::new();
        assert!(still_running(&none, "a1", &mut seen, Duration::ZERO));
        assert!(!still_running(&none, "a1", &mut seen, REPORTED_WITHIN));
        assert!(still_running(
            &["a1".into()],
            "a1",
            &mut seen,
            REPORTED_WITHIN
        ));
        assert!(!still_running(&none, "a1", &mut seen, Duration::ZERO));
    }

    #[test]
    fn a_mirror_draws_tools_and_replies_the_way_claude_code_does() {
        let tool = shown(
            &Said::Tool {
                name: "Read".into(),
                detail: "src/main.rs".into(),
                change: None,
                running_since: None,
            },
            80,
        );
        assert!(tool.contains("⏺") && tool.contains("Read") && tool.contains("(src/main.rs)"));
        let reply = shown(&Said::Reply("Found it.".into()), 80);
        assert!(reply.starts_with("⏺ "), "{reply:?}");
        let prompt = shown(&Said::You("find the parser".into()), 80);
        assert!(prompt.contains(" > find the parser "), "{prompt:?}");
    }
}
