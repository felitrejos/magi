use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::limits::percent;
use crate::proto::{self, CodexModel, Window};

const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const MAX_MODELS: usize = 12;
const HELLO: &str = concat!(
    r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"magi","version":"1"}}}"#,
    "\n",
    r#"{"method":"initialized"}"#,
    "\n",
);
const RATE_LIMITS: &str = r#"{"id":2,"method":"account/rateLimits/read"}"#;
const MODEL_LIST: &str = r#"{"id":2,"method":"model/list","params":{}}"#;

#[derive(Deserialize)]
struct Reply<T> {
    id: Option<u64>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct ModelPage {
    data: Vec<ListedModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedModel {
    id: String,
    display_name: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    is_default: bool,
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

pub fn limits(program: &Path) -> Option<Vec<Window>> {
    ask(program, RATE_LIMITS, windows)
}

pub fn models(program: &Path) -> Option<Vec<CodexModel>> {
    ask(program, MODEL_LIST, listed_models)
}

fn ask<T>(program: &Path, request: &str, answer: fn(&str) -> Option<T>) -> Option<T> {
    let mut child = Command::new(program)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let answered = converse(&mut child, request, answer);
    let _ = child.kill();
    let _ = child.wait();
    answered
}

fn converse<T>(child: &mut Child, request: &str, answer: fn(&str) -> Option<T>) -> Option<T> {
    let stdin = child.stdin.as_mut()?;
    stdin.write_all(HELLO.as_bytes()).ok()?;
    writeln!(stdin, "{request}").ok()?;
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
        if let Some(answered) = answer(&line) {
            return Some(answered);
        }
    }
}

fn listed_models(line: &str) -> Option<Vec<CodexModel>> {
    let reply: Reply<ModelPage> = serde_json::from_str(line).ok()?;
    if reply.id != Some(2) {
        return None;
    }
    Some(
        reply
            .result?
            .data
            .into_iter()
            .filter(|model| !model.hidden && proto::is_model_name(&model.id))
            .take(MAX_MODELS)
            .map(|model| CodexModel {
                id: model.id,
                name: model.display_name,
                default: model.is_default,
            })
            .collect(),
    )
}

fn windows(line: &str) -> Option<Vec<Window>> {
    let reply: Reply<Answer> = serde_json::from_str(line).ok()?;
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
                used: percent(window.used_percent),
                resets_at: window.resets_at,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_models_keep_visible_ones_with_safe_names_and_mark_the_default() {
        let answer = r#"{"id":2,"result":{"data":[{"id":"gpt-6-luna","displayName":"GPT-6-Luna","hidden":false,"isDefault":true},{"id":"secret","displayName":"S","hidden":true},{"id":"-x","displayName":"Bad","hidden":false}],"nextCursor":null}}"#;
        assert_eq!(
            listed_models(answer),
            Some(vec![CodexModel {
                id: "gpt-6-luna".into(),
                name: "GPT-6-Luna".into(),
                default: true,
            }])
        );
        assert_eq!(listed_models(r#"{"id":1,"result":{}}"#), None);
    }

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
