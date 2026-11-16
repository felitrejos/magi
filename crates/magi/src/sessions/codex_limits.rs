use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::handoff;
use crate::proto::{BasisPoints, FULL, Window};

const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const REQUEST: &str = concat!(
    r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"magi","version":"1"}}}"#,
    "\n",
    r#"{"method":"initialized"}"#,
    "\n",
    r#"{"id":2,"method":"account/rateLimits/read"}"#,
    "\n",
);

type Reading = (u64, Vec<Window>);

pub struct Limits {
    program: Option<PathBuf>,
    asked_at: Option<Instant>,
    asking: Arc<AtomicBool>,
    answer: Arc<Mutex<Option<Reading>>>,
}

impl Limits {
    pub fn new() -> Self {
        Self {
            program: handoff::find("codex", &handoff::search_dirs()),
            asked_at: None,
            asking: Arc::new(AtomicBool::new(false)),
            answer: Arc::new(Mutex::new(None)),
        }
    }

    pub fn latest(&mut self, now: Instant, now_secs: u64) -> Option<Reading> {
        let due = self
            .asked_at
            .is_none_or(|at| now.duration_since(at) >= ASK_EVERY);
        if let Some(program) = self.program.clone().filter(|_| due)
            && !self.asking.swap(true, Ordering::AcqRel)
        {
            self.asked_at = Some(now);
            let (asking, answer) = (self.asking.clone(), self.answer.clone());
            thread::spawn(move || {
                if let Some(windows) = ask(&program) {
                    *answer.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((now_secs, windows));
                }
                asking.store(false, Ordering::Release);
            });
        }
        self.answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[derive(Deserialize)]
struct Reply {
    id: Option<u64>,
    result: Option<Answer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    rate_limits: RateLimits,
}

#[derive(Deserialize)]
struct RateLimits {
    primary: Option<LimitWindow>,
    secondary: Option<LimitWindow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    used_percent: f64,
    window_duration_mins: u32,
    resets_at: u64,
}

fn ask(program: &PathBuf) -> Option<Vec<Window>> {
    let mut child = Command::new(program)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let answer = converse(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    answer
}

fn converse(child: &mut Child) -> Option<Vec<Window>> {
    child.stdin.as_mut()?.write_all(REQUEST.as_bytes()).ok()?;
    let stdout = child.stdout.take()?;
    let (lines, received) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + ANSWER_WITHIN;
    loop {
        let line = received
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()?;
        if let Some(windows) = windows(&line) {
            return Some(windows);
        }
    }
}

fn windows(line: &str) -> Option<Vec<Window>> {
    let reply: Reply = serde_json::from_str(line).ok()?;
    if reply.id != Some(2) {
        return None;
    }
    let limits = reply.result?.rate_limits;
    Some(
        [limits.primary, limits.secondary]
            .into_iter()
            .flatten()
            .map(|window| Window {
                minutes: window.window_duration_mins,
                used: (window.used_percent.clamp(0.0, 100.0) * f64::from(FULL) / 100.0).round()
                    as BasisPoints,
                resets_at: window.resets_at,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_limit_answer_becomes_windows_and_other_lines_are_skipped() {
        assert_eq!(windows(r#"{"id":1,"result":{"userAgent":"codex"}}"#), None);
        assert_eq!(windows(r#"{"method":"account/updated","params":{}}"#), None);
        let answer = r#"{"id":2,"result":{"rateLimits":{"limitId":"codex","primary":{"usedPercent":86,"windowDurationMins":43200,"resetsAt":1795911895},"secondary":null,"planType":"go"}}}"#;
        assert_eq!(
            windows(answer),
            Some(vec![Window {
                minutes: 43_200,
                used: 8_600,
                resets_at: 1_795_911_895,
            }])
        );
    }
}
