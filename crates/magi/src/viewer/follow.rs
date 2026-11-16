use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use super::view;
use crate::children;
use crate::conversation::Reader;
use crate::proto::{Agent, Said};

const EVERY: Duration = Duration::from_millis(500);
const LINGER: Duration = Duration::from_secs(10);
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

pub fn run(transcript: &Path) -> io::Result<()> {
    let mut reader = Reader::default();
    let mut printed = 0;
    let mut titled = false;
    let mut out = io::stdout().lock();
    loop {
        let meta = children::subagent_meta(transcript);
        if let Some(meta) = &meta
            && !titled
        {
            titled = true;
            write!(out, "{BOLD}↳ {}{RESET}\n\n", meta.description)?;
        }
        if let Ok(said) = reader.read(transcript, Agent::ClaudeCode) {
            for item in said.iter().skip(printed) {
                out.write_all(shown(item, width()).as_bytes())?;
            }
            printed = said.len();
        }
        out.flush()?;
        if meta.is_some_and(|meta| meta.finished) {
            write!(out, "\n{DIM}done · this tab closes in a moment{RESET}\n")?;
            out.flush()?;
            thread::sleep(LINGER);
            return Ok(());
        }
        thread::sleep(EVERY);
    }
}

fn shown(said: &Said, width: usize) -> String {
    match said {
        Said::You(text) => format!("{DIM}› {}{RESET}\n\n", text.trim()),
        Said::Reply(text) => format!("{}\n", view::markdown_ansi(text, width)),
        Said::Tool {
            name,
            detail,
            change,
            ..
        } => {
            let change = change.map_or_else(String::new, |change| {
                format!("  +{} −{}", change.added, change.removed)
            });
            format!(
                "{DIM}⏺ {name}  {}{change}{RESET}\n",
                detail.lines().next().unwrap_or_default()
            )
        }
        Said::Plan(_) => String::new(),
    }
}

fn width() -> usize {
    ratatui::crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns))
}
