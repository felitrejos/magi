use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use similar::{ChangeTag, TextDiff};

use crate::proto::{Agent, Change, Said};

const TAIL_WINDOWS: [u64; 3] = [1 << 20, 4 << 20, 8 << 20];
const SKIM_OVER: usize = 64 * 1024;
const MAX_ENTRIES: usize = 80;
const DIFF_LIMIT: Duration = Duration::from_millis(20);
const MAX_TEXT_BYTES: usize = 6 * 1024;
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

#[derive(Default)]
pub struct Reader {
    cached: Option<(PathBuf, u64, Vec<Said>)>,
}

impl Reader {
    pub fn read(&mut self, path: &Path, agent: Agent) -> Result<Vec<Said>, String> {
        let len = path
            .metadata()
            .map_err(|error| format!("can't read the conversation: {error}"))?
            .len();
        if let Some((cached, cached_len, said)) = &self.cached
            && cached == path
            && *cached_len == len
        {
            return Ok(said.clone());
        }
        let mut said = Vec::new();
        for window in TAIL_WINDOWS {
            let tail = tail(path, len, window)
                .map_err(|error| format!("can't read the conversation: {error}"))?;
            said = self::said(&tail, agent);
            if said.len() >= MAX_ENTRIES || window >= len {
                break;
            }
        }
        self.cached = Some((path.to_path_buf(), len, said.clone()));
        Ok(said)
    }
}

fn tail(path: &Path, len: u64, window: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let start = len.saturating_sub(window);
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
        .filter(|line| line.len() < SKIM_OVER || !line.contains("\"toolUseResult\""))
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
            Said::Tool {
                name,
                detail,
                change,
            } => Said::Tool {
                name,
                detail: shorten(&detail, &cwd, &home),
                change,
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
                Some("tool_use") => {
                    let name = block["name"].as_str()?;
                    Some(Said::Tool {
                        name: name.to_string(),
                        detail: detail(&block["input"]),
                        change: change(name, &block["input"]),
                    })
                }
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
                change: patch_change(&input),
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
    (!trimmed.is_empty() && !trimmed.starts_with('<')).then(|| capped(trimmed))
}

fn detail(input: &Value) -> String {
    let found = match input {
        Value::String(raw) => Some(raw.clone()),
        Value::Object(fields) => DETAIL_KEYS.iter().find_map(|key| match fields.get(*key)? {
            Value::String(text) => Some(text.clone()),
            Value::Array(words) => Some(command_line(words)),
            _ => None,
        }),
        _ => None,
    };
    let found = found.unwrap_or_default();
    let patched = patched_files(&found);
    let line = if patched.is_empty() {
        found.lines().next().unwrap_or_default()
    } else {
        &patched
    };
    let mut detail: String = line.chars().take(MAX_DETAIL_CHARS).collect();
    if line.chars().count() > MAX_DETAIL_CHARS {
        detail.push('…');
    }
    detail
}

fn command_line(words: &[Value]) -> String {
    let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
    match words.as_slice() {
        [shell, flag, script] if ["bash", "sh", "zsh"].contains(shell) && flag.ends_with('c') => {
            script.to_string()
        }
        _ => words.join(" "),
    }
}

fn patched_files(text: &str) -> String {
    if !text.starts_with("*** Begin Patch") {
        return String::new();
    }
    text.lines()
        .filter_map(|line| {
            ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn capped(text: &str) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn replaced(edit: &Value) -> Option<(&str, &str)> {
    Some((edit["old_string"].as_str()?, edit["new_string"].as_str()?))
}

fn change(tool: &str, input: &Value) -> Option<Change> {
    let pairs: Vec<(&str, &str)> = match tool {
        "Edit" => vec![replaced(input)?],
        "MultiEdit" => input["edits"]
            .as_array()?
            .iter()
            .filter_map(replaced)
            .collect(),
        "Write" => {
            let written = input["content"].as_str()?;
            return Some(Change {
                added: written.lines().count() as u32,
                removed: 0,
            });
        }
        _ => return None,
    };
    let mut total = Change {
        added: 0,
        removed: 0,
    };
    for (old, new) in pairs {
        let diff = TextDiff::configure()
            .deadline(Instant::now() + DIFF_LIMIT)
            .diff_lines(old, new);
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => total.added += 1,
                ChangeTag::Delete => total.removed += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    Some(total)
}

fn patch_change(input: &Value) -> Option<Change> {
    let candidates = [input.as_str(), input["input"].as_str()]
        .into_iter()
        .flatten()
        .chain(
            input["command"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
    let patch = candidates
        .into_iter()
        .find(|text| text.contains("*** Begin Patch"))?;
    let mut total = Change {
        added: 0,
        removed: 0,
    };
    for line in patch.lines() {
        if line.starts_with('+') {
            total.added += 1;
        } else if line.starts_with('-') {
            total.removed += 1;
        }
    }
    Some(total)
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
                    detail: "cargo test -q".into(),
                    change: None
                },
                Said::Tool {
                    name: "Read".into(),
                    detail: "/src/main.rs".into(),
                    change: None
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
                    detail: "rg limit".into(),
                    change: None
                },
                Said::Reply("Done.".into()),
            ]
        );
    }

    #[test]
    fn edits_count_the_lines_they_add_and_remove() {
        let edit: Value = serde_json::from_str(
            r#"{"file_path":"/a.rs","old_string":"one\ntwo\nthree\n","new_string":"one\n2\nthree\nfour\n"}"#,
        )
        .unwrap();
        assert_eq!(
            change("Edit", &edit),
            Some(Change {
                added: 2,
                removed: 1
            })
        );
        let multi: Value = serde_json::from_str(
            r#"{"file_path":"/a.rs","edits":[{"old_string":"a\n","new_string":"b\n"},{"old_string":"c\n","new_string":""}]}"#,
        )
        .unwrap();
        assert_eq!(
            change("MultiEdit", &multi),
            Some(Change {
                added: 1,
                removed: 2
            })
        );
        let write: Value =
            serde_json::from_str(r#"{"file_path":"/new.rs","content":"a\nb\nc\n"}"#).unwrap();
        assert_eq!(
            change("Write", &write),
            Some(Change {
                added: 3,
                removed: 0
            })
        );
        let bash: Value = serde_json::from_str(r#"{"command":"ls"}"#).unwrap();
        assert_eq!(change("Bash", &bash), None);
        let message: Value = serde_json::from_str(r#"{"to":"a","content":"one\ntwo"}"#).unwrap();
        assert_eq!(change("SendMessage", &message), None);
        let patch = Value::String(
            "*** Begin Patch\n*** Update File: a.sql\n@@\n-old\n--- a removed comment\n+new\n+++counter\n*** End Patch".into(),
        );
        let expected = Some(Change {
            added: 2,
            removed: 2,
        });
        assert_eq!(patch_change(&patch), expected);
        let shell = serde_json::json!({ "command": ["apply_patch", patch] });
        assert_eq!(patch_change(&shell), expected);
    }

    #[test]
    fn very_long_replies_are_cut_on_a_character_boundary() {
        let long = format!("a{}", "é".repeat(MAX_TEXT_BYTES));
        let cut = capped(&long);
        assert_eq!(cut.len(), MAX_TEXT_BYTES - 1 + "…".len());
        assert!(cut.ends_with("é…"));
        let prompt = format!(
            r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"text":"{}"}}]}}}}"#,
            "x".repeat(MAX_TEXT_BYTES * 3)
        );
        let said = codex(&serde_json::from_str(&prompt).unwrap());
        let [Said::You(text)] = said.as_slice() else {
            panic!("{said:?}")
        };
        assert!(text.len() <= MAX_TEXT_BYTES + "…".len());
    }

    #[test]
    fn only_the_end_of_a_long_transcript_is_read_and_unchanged_files_are_not_reread() {
        let path =
            std::env::temp_dir().join(format!("magi-conversation-{}.jsonl", std::process::id()));
        let filler = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old"}]}}"#;
        let mut body = String::new();
        while body.len() < (TAIL_WINDOWS[2] as usize) * 2 {
            body.push_str(filler);
            body.push('\n');
        }
        body.push_str(r#"{"type":"user","message":{"role":"user","content":"latest"}}"#);
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let mut reader = Reader::default();
        let said = reader.read(&path, Agent::ClaudeCode).unwrap();
        assert_eq!(said.last(), Some(&Said::You("latest".into())));
        assert!(said.len() <= MAX_ENTRIES);
        assert_eq!(reader.read(&path, Agent::ClaudeCode).unwrap(), said);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn huge_tool_results_make_the_reader_look_further_back() {
        let path = std::env::temp_dir().join(format!(
            "magi-conversation-wide-{}.jsonl",
            std::process::id()
        ));
        let mut body = String::new();
        for n in 0..MAX_ENTRIES {
            body.push_str(&format!(
                r#"{{"type":"user","message":{{"role":"user","content":"early {n}"}}}}"#
            ));
            body.push('\n');
        }
        let image = "A".repeat(700 * 1024);
        for _ in 0..2 {
            body.push_str(&format!(
                r#"{{"type":"user","toolUseResult":{{}},"message":{{"content":[{{"type":"tool_result","content":"{image}"}}]}}}}"#
            ));
            body.push('\n');
        }
        body.push_str(r#"{"type":"user","message":{"role":"user","content":"latest"}}"#);
        body.push('\n');
        std::fs::write(&path, &body).unwrap();
        let said = Reader::default().read(&path, Agent::ClaudeCode).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(said.len(), MAX_ENTRIES);
        assert_eq!(said.last(), Some(&Said::You("latest".into())));
        assert_eq!(said[0], Said::You("early 1".into()));
    }

    #[test]
    fn patches_name_their_files_and_command_lists_read_as_one_line() {
        let patch = Value::String(
            "*** Begin Patch\n*** Update File: a.go\n@@\n-x\n+y\n*** Add File: b.go\n+z\n*** End Patch".into(),
        );
        assert_eq!(detail(&patch), "a.go, b.go");
        let shell: Value = serde_json::json!({ "command": ["bash", "-lc", "go test ./..."] });
        assert_eq!(detail(&shell), "go test ./...");
        let direct: Value = serde_json::json!({ "command": ["go", "vet", "./..."] });
        assert_eq!(detail(&direct), "go vet ./...");
    }

    #[test]
    fn paths_read_relative_to_the_project_or_home() {
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
    }

    #[test]
    #[ignore = "benchmark: MAGI_TRANSCRIPT=path cargo test --release -- --ignored --nocapture bench_tail"]
    fn bench_tail() {
        let path = std::env::var("MAGI_TRANSCRIPT").expect("MAGI_TRANSCRIPT names a transcript");
        let started = std::time::Instant::now();
        let said = Reader::default()
            .read(Path::new(&path), Agent::ClaudeCode)
            .unwrap();
        println!("read {} entries in {:?}", said.len(), started.elapsed());
    }
}
