use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::proto::{Agent, BOLD, DIM, Run, Shade};

const TAIL_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 40;
const MAX_PROMPT_LINES: usize = 6;
const MAX_REPLY_LINES: usize = 14;
const MAX_DETAIL_CHARS: usize = 120;
const DETAIL_KEYS: [&str; 8] = [
    "command",
    "file_path",
    "path",
    "pattern",
    "description",
    "url",
    "query",
    "prompt",
];

#[derive(Debug, PartialEq)]
enum Said {
    You(String),
    Reply(String),
    Tool { name: String, detail: String },
}

#[derive(Default)]
pub struct Reader {
    cached: Option<(PathBuf, u64, Vec<Vec<Run>>)>,
}

impl Reader {
    pub fn read(&mut self, path: &Path, agent: Agent) -> Result<Vec<Vec<Run>>, String> {
        let len = path
            .metadata()
            .map_err(|error| format!("can't read the conversation: {error}"))?
            .len();
        if let Some((cached, cached_len, rows)) = &self.cached
            && cached == path
            && *cached_len == len
        {
            return Ok(rows.clone());
        }
        let tail =
            tail(path, len).map_err(|error| format!("can't read the conversation: {error}"))?;
        let rows = rows(&said(&tail, agent));
        self.cached = Some((path.to_path_buf(), len, rows.clone()));
        Ok(rows)
    }
}

fn tail(path: &Path, len: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(if start > 0 {
        text.split_once('\n')
            .map_or(String::new(), |(_, rest)| rest.to_string())
    } else {
        text
    })
}

fn said(text: &str, agent: Agent) -> Vec<Said> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut cwd = String::new();
    let mut said = Vec::new();
    for value in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if let Some(seen) = value["cwd"]
            .as_str()
            .or_else(|| value["payload"]["cwd"].as_str())
        {
            cwd = seen.to_string();
        }
        let found = match agent {
            Agent::ClaudeCode => claude(&value),
            Agent::Codex => codex(&value),
        };
        said.extend(found.into_iter().map(|entry| match entry {
            Said::Tool { name, detail } => Said::Tool {
                name,
                detail: shorten(&detail, &cwd, &home),
            },
            other => other,
        }));
    }
    said.drain(..said.len().saturating_sub(MAX_ENTRIES));
    said
}

fn shorten(detail: &str, cwd: &str, home: &str) -> String {
    let inside = |root: &str| {
        (!root.is_empty())
            .then(|| detail.strip_prefix(root)?.strip_prefix('/'))
            .flatten()
    };
    if let Some(relative) = inside(cwd) {
        return relative.to_string();
    }
    match inside(home) {
        Some(relative) => format!("~/{relative}"),
        None => detail.replace(&format!("{home}/"), "~/"),
    }
}

fn claude(value: &Value) -> Vec<Said> {
    let message = &value["message"];
    match value["type"].as_str() {
        Some("user") if value.get("toolUseResult").is_none() && value["isMeta"] != true => {
            prompt(&message["content"])
                .map(Said::You)
                .into_iter()
                .collect()
        }
        Some("assistant") => message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| match block["type"].as_str() {
                Some("text") => text(block["text"].as_str()?).map(Said::Reply),
                Some("tool_use") => Some(Said::Tool {
                    name: block["name"].as_str()?.to_string(),
                    detail: detail(&block["input"]),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn codex(value: &Value) -> Vec<Said> {
    let payload = &value["payload"];
    if value["type"] != "response_item" {
        return Vec::new();
    }
    match (payload["type"].as_str(), payload["role"].as_str()) {
        (Some("message"), Some(role @ ("user" | "assistant"))) => {
            let joined: String = payload["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            text(&joined)
                .map(|text| {
                    if role == "user" {
                        Said::You(text)
                    } else {
                        Said::Reply(text)
                    }
                })
                .into_iter()
                .collect()
        }
        (Some("function_call" | "custom_tool_call"), _) => {
            let input = payload
                .get("arguments")
                .or_else(|| payload.get("input"))
                .cloned()
                .unwrap_or(Value::Null);
            let input = match &input {
                Value::String(raw) => serde_json::from_str(raw).unwrap_or(input),
                _ => input,
            };
            vec![Said::Tool {
                name: payload["name"].as_str().unwrap_or("tool").to_string(),
                detail: detail(&input),
            }]
        }
        _ => Vec::new(),
    }
}

fn prompt(content: &Value) -> Option<String> {
    match content {
        Value::String(raw) => text(raw),
        Value::Array(blocks) => text(
            &blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

fn text(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty() && !trimmed.starts_with('<')).then(|| trimmed.to_string())
}

fn detail(input: &Value) -> String {
    let found = match input {
        Value::String(raw) => Some(raw.as_str()),
        Value::Object(fields) => DETAIL_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(Value::as_str)),
        _ => None,
    };
    let line = found.unwrap_or_default().lines().next().unwrap_or_default();
    let mut detail: String = line.chars().take(MAX_DETAIL_CHARS).collect();
    if line.chars().count() > MAX_DETAIL_CHARS {
        detail.push('…');
    }
    detail
}

fn rows(said: &[Said]) -> Vec<Vec<Run>> {
    let plain = |text: String, flags| Run {
        text,
        fg: Shade::Default,
        bg: Shade::Default,
        flags,
    };
    let mut rows = Vec::new();
    for entry in said {
        match entry {
            Said::You(prompt) => {
                rows.push(Vec::new());
                for (index, line) in capped(prompt, MAX_PROMPT_LINES).into_iter().enumerate() {
                    let mark = if index == 0 { "› " } else { "  " };
                    rows.push(vec![plain(format!("{mark}{line}"), BOLD)]);
                }
                rows.push(Vec::new());
            }
            Said::Reply(reply) => {
                rows.extend(
                    capped(reply, MAX_REPLY_LINES)
                        .iter()
                        .map(|line| emphasis(line)),
                );
            }
            Said::Tool { name, detail } => {
                let mut row = vec![plain(format!("⏺ {name}"), BOLD | DIM)];
                if !detail.is_empty() {
                    row.push(plain(format!("  {detail}"), DIM));
                }
                rows.push(row);
            }
        }
    }
    rows
}

fn emphasis(line: &str) -> Vec<Run> {
    line.split("**")
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(index, part)| Run {
            text: part.to_string(),
            fg: Shade::Default,
            bg: Shade::Default,
            flags: if index % 2 == 1 { BOLD } else { 0 },
        })
        .collect()
}

fn capped(text: &str, max: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<String> = lines
        .iter()
        .take(max)
        .map(|line| (*line).to_string())
        .collect();
    if lines.len() > max {
        kept.push("…".into());
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_transcripts_become_prompts_replies_and_tool_calls() {
        let transcript = [
            r#"{"type":"user","message":{"role":"user","content":"run the tests"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Caveat: injected"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"Running them now."},{"type":"tool_use","name":"Bash","input":{"command":"cargo test -q\nsecond line","description":"Run tests"}}]}}"#,
            r#"{"type":"user","toolUseResult":{},"message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#,
            "not json",
        ]
        .join("\n");
        assert_eq!(
            said(&transcript, Agent::ClaudeCode),
            [
                Said::You("run the tests".into()),
                Said::Reply("Running them now.".into()),
                Said::Tool {
                    name: "Bash".into(),
                    detail: "cargo test -q".into()
                },
                Said::Tool {
                    name: "Read".into(),
                    detail: "/src/main.rs".into()
                },
            ]
        );
    }

    #[test]
    fn codex_rollouts_skip_developer_and_context_messages() {
        let rollout = [
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add rate limiting"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"rg limit\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#,
        ]
        .join("\n");
        assert_eq!(
            said(&rollout, Agent::Codex),
            [
                Said::You("add rate limiting".into()),
                Said::Tool {
                    name: "shell".into(),
                    detail: "rg limit".into()
                },
                Said::Reply("Done.".into()),
            ]
        );
    }

    #[test]
    fn prompts_stand_out_and_long_replies_are_cut() {
        let long = (0..20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let rows = rows(&[
            Said::You("first\nsecond".into()),
            Said::Reply(long),
            Said::Tool {
                name: "Bash".into(),
                detail: String::new(),
            },
        ]);
        let texts: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|run| run.text.as_str()).collect())
            .collect();
        assert_eq!(&texts[..4], ["", "› first", "  second", ""]);
        assert_eq!(texts[4 + MAX_REPLY_LINES], "…");
        assert_eq!(texts.last().unwrap(), "⏺ Bash");
        assert_eq!(rows[1][0].flags, BOLD);
    }

    #[test]
    fn only_the_end_of_a_long_transcript_is_read_and_unchanged_files_are_not_reread() {
        let path =
            std::env::temp_dir().join(format!("magi-conversation-{}.jsonl", std::process::id()));
        let filler = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old"}]}}"#;
        let mut body = String::new();
        while body.len() < (TAIL_BYTES as usize) * 2 {
            body.push_str(filler);
            body.push('\n');
        }
        body.push_str(r#"{"type":"user","message":{"role":"user","content":"latest"}}"#);
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let mut reader = Reader::default();
        let rows = reader.read(&path, Agent::ClaudeCode).unwrap();
        assert!(rows.iter().flatten().any(|run| run.text == "› latest"));
        assert!(rows.len() <= MAX_ENTRIES * 2 + 4);
        assert_eq!(reader.read(&path, Agent::ClaudeCode).unwrap(), rows);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn paths_read_relative_to_the_project_or_home_and_bold_is_bold() {
        assert_eq!(
            shorten("/home/me/web/src/a.rs", "/home/me/web", "/home/me"),
            "src/a.rs"
        );
        assert_eq!(
            shorten("/home/me/other/b.rs", "/home/me/web", "/home/me"),
            "~/other/b.rs"
        );
        assert_eq!(
            shorten("cd /home/me/web && cargo test", "/home/me/web", "/home/me"),
            "cd ~/web && cargo test"
        );
        assert_eq!(shorten("/etc/hosts", "", "/home/me"), "/etc/hosts");
        let runs = emphasis("- **Benchmarks:** report delays");
        let parts: Vec<(&str, u8)> = runs
            .iter()
            .map(|run| (run.text.as_str(), run.flags))
            .collect();
        assert_eq!(
            parts,
            [("- ", 0), ("Benchmarks:", BOLD), (" report delays", 0)]
        );
    }
}
