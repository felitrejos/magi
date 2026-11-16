use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::claude_limits::unix_secs;
use super::ledger::{Counted, Ledger};
use super::pricing::{self, Usage};
use super::tail::{self, Lines, Tail};
use crate::proto::{Child, ChildKind, ChildState, Tokens};

const ASSISTANT_MARK: &[u8] = b"\"type\":\"assistant\"";
const RESULT_MARK: &[u8] = b"\"tool_result\"";
const NOTICE_MARK: &[u8] = b"<task-notification>";
const STOP_TOOLS: [&str; 3] = ["TaskStop", "KillShell", "KillBash"];
const MAX_LABEL_CHARS: usize = 60;
const KEEP_FINISHED_SECS: u64 = 10 * 60;
const MAX_TRACKED: usize = 40;

pub struct Transcript {
    tail: Tail,
    pub state: Summary,
}

const SUBAGENT_TOOLS: [&str; 2] = ["Agent", "Task"];

#[derive(Default)]
pub struct Summary {
    pub ledger: Ledger,
    pub model: Option<String>,
    pub context_tokens: u64,
    pub ended_turn: bool,
    pub cwd: Option<String>,
    open_tools: Vec<OpenTool>,
    tracked: Vec<Tracked>,
}

struct Tracked {
    id: String,
    task: Option<String>,
    kind: ChildKind,
    label: String,
    detail: String,
    state: ChildState,
    at: u64,
}

struct OpenTool {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct ToolInput {
    #[serde(default)]
    description: Option<serde_json::Value>,
    #[serde(default)]
    subagent_type: Option<serde_json::Value>,
    #[serde(default)]
    command: Option<serde_json::Value>,
    #[serde(default)]
    run_in_background: Option<serde_json::Value>,
    #[serde(default)]
    task_id: Option<serde_json::Value>,
    #[serde(default)]
    shell_id: Option<serde_json::Value>,
}

fn text(value: &Option<serde_json::Value>) -> &str {
    value
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

fn label(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    let mut label: String = first.chars().take(MAX_LABEL_CHARS).collect();
    if first.chars().count() > MAX_LABEL_CHARS {
        label.push('…');
    }
    label
}

impl ToolInput {
    fn tracked(&self, id: &str, name: &str, at: u64) -> Option<Tracked> {
        let (kind, label, detail) = if SUBAGENT_TOOLS.contains(&name) {
            let description = text(&self.description);
            let what = if description.is_empty() {
                "subagent"
            } else {
                description
            };
            (
                ChildKind::Subagent,
                label(what),
                text(&self.subagent_type).to_string(),
            )
        } else if name == "Bash"
            && self
                .run_in_background
                .as_ref()
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            let description = text(&self.description);
            let what = if description.is_empty() {
                text(&self.command)
            } else {
                description
            };
            (ChildKind::Shell, label(what), String::new())
        } else {
            return None;
        };
        Some(Tracked {
            id: id.to_string(),
            task: None,
            kind,
            label,
            detail,
            state: ChildState::Running,
            at,
        })
    }

    fn stopped_task(&self) -> Option<&str> {
        [&self.task_id, &self.shell_id]
            .into_iter()
            .map(text)
            .find(|id| !id.is_empty())
    }
}

#[derive(Default)]
struct ResultInfo {
    task: Option<String>,
    is_async: bool,
}

fn result_info<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ResultInfo, D::Error> {
    deserializer.deserialize_any(ResultInfoVisitor)
}

struct ResultInfoVisitor;

impl<'de> Visitor<'de> for ResultInfoVisitor {
    type Value = ResultInfo;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a tool result")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ResultInfo, A::Error> {
        let mut info = ResultInfo::default();
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            match key.as_ref() {
                "backgroundTaskId" | "agentId" => {
                    if let Some(id) = map.next_value::<Option<String>>()? {
                        info.task.get_or_insert(id);
                    }
                }
                "isAsync" => info.is_async = map.next_value::<Option<bool>>()?.unwrap_or(false),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(info)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<ResultInfo, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(ResultInfo::default())
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<ResultInfo, E> {
        Ok(ResultInfo::default())
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<ResultInfo, E> {
        Ok(ResultInfo::default())
    }

    fn visit_unit<E: de::Error>(self) -> Result<ResultInfo, E> {
        Ok(ResultInfo::default())
    }
}

struct Feed<'a> {
    state: &'a mut Summary,
    claims: &'a mut HashSet<String>,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default, rename = "toolUseResult", deserialize_with = "result_info")]
    result: ResultInfo,
    #[serde(default)]
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<UsageJson>,
    #[serde(default, deserialize_with = "blocks_or_none")]
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    input: Option<ToolInput>,
}

#[derive(Deserialize, Default)]
struct UsageJson {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
    #[serde(default)]
    speed: Option<String>,
}

#[derive(Deserialize, Default)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

fn blocks_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Block>, D::Error> {
    deserializer.deserialize_any(BlocksOrNone)
}

struct BlocksOrNone;

impl<'de> Visitor<'de> for BlocksOrNone {
    type Value = Vec<Block>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("message content")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<Block>, A::Error> {
        let mut blocks = Vec::new();
        while let Some(block) = seq.next_element()? {
            blocks.push(block);
        }
        Ok(blocks)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<Block>, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Vec::new())
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Vec<Block>, E> {
        Ok(Vec::new())
    }

    fn visit_unit<E: de::Error>(self) -> Result<Vec<Block>, E> {
        Ok(Vec::new())
    }
}

impl Transcript {
    pub fn new(path: PathBuf) -> Self {
        Self {
            tail: Tail::new(path),
            state: Summary::default(),
        }
    }

    pub fn path(&self) -> &Path {
        self.tail.path()
    }

    pub fn running_tool(&self) -> Option<&str> {
        self.state.open_tools.last().map(|open| open.name.as_str())
    }

    pub fn children(&self, now_secs: u64) -> Vec<Child> {
        self.state
            .tracked
            .iter()
            .filter(|tracked| {
                tracked.state == ChildState::Running
                    || now_secs.saturating_sub(tracked.at) < KEEP_FINISHED_SECS
            })
            .map(|tracked| Child {
                id: tracked.id.clone(),
                kind: tracked.kind,
                label: tracked.label.clone(),
                detail: tracked.detail.clone(),
                state: tracked.state,
                secs: u32::try_from(now_secs.saturating_sub(tracked.at)).unwrap_or(u32::MAX),
            })
            .collect()
    }

    pub fn release(&self, claims: &mut HashSet<String>) {
        self.state.ledger.release(claims);
    }

    pub fn adopt(&mut self, claims: &mut HashSet<String>) {
        self.state.ledger.adopt(claims);
    }

    pub fn advance(&mut self, budget: &mut u64, claims: &mut HashSet<String>) -> io::Result<()> {
        self.tail.advance(
            budget,
            &mut Feed {
                state: &mut self.state,
                claims,
            },
        )
    }
}

impl Lines for Feed<'_> {
    fn restart(&mut self) {
        self.state.ledger.release(self.claims);
        *self.state = Summary::default();
    }

    fn line(&mut self, line: &[u8]) {
        if tail::contains(line, NOTICE_MARK) {
            return self.state.notice(line);
        }
        if !tail::contains(line, ASSISTANT_MARK) && !tail::contains(line, RESULT_MARK) {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<Line>(line) else {
            return;
        };
        let at = parsed
            .timestamp
            .as_deref()
            .and_then(unix_secs)
            .unwrap_or_default();
        let Some(message) = parsed.message else {
            return;
        };
        let state = &mut *self.state;
        if parsed.cwd.is_some() {
            state.cwd = parsed.cwd;
        }
        match parsed.kind.as_str() {
            "assistant" => {
                for block in message
                    .content
                    .iter()
                    .filter(|block| block.kind == "tool_use")
                {
                    if let (Some(id), Some(name)) = (&block.id, &block.name) {
                        state.open_tools.push(OpenTool {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        let Some(input) = &block.input else {
                            continue;
                        };
                        if STOP_TOOLS.contains(&name.as_str())
                            && let Some(task) = input.stopped_task()
                        {
                            state.finish_task(task, ChildState::Stopped, at);
                        }
                        if let Some(tracked) = input.tracked(id, name, at) {
                            state.track(tracked);
                        }
                    }
                }
                state.ended_turn = message.stop_reason.as_deref() == Some("end_turn");
                if state.ended_turn {
                    state.open_tools.clear();
                    for tracked in &mut state.tracked {
                        if tracked.state == ChildState::Running && tracked.task.is_none() {
                            tracked.state = ChildState::Stopped;
                            tracked.at = at;
                        }
                    }
                }
                state.count(&message, self.claims);
            }
            "user" => {
                state.ended_turn = false;
                for block in message
                    .content
                    .iter()
                    .filter(|block| block.kind == "tool_result")
                {
                    let Some(id) = &block.tool_use_id else {
                        continue;
                    };
                    state.open_tools.retain(|open| open.id != *id);
                    let Some(tracked) = state.tracked.iter_mut().find(|tracked| tracked.id == *id)
                    else {
                        continue;
                    };
                    let failed = block.is_error == Some(true);
                    match (&parsed.result.task, failed) {
                        (Some(task), false)
                            if tracked.kind == ChildKind::Shell || parsed.result.is_async =>
                        {
                            tracked.task = Some(task.clone());
                        }
                        _ => {
                            tracked.state = if failed {
                                ChildState::Failed
                            } else {
                                ChildState::Done
                            };
                            tracked.at = at;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Summary {
    fn track(&mut self, tracked: Tracked) {
        self.tracked.retain(|known| known.id != tracked.id);
        self.tracked.push(tracked);
        if self.tracked.len() > MAX_TRACKED {
            let finished = self
                .tracked
                .iter()
                .position(|tracked| tracked.state != ChildState::Running)
                .unwrap_or(0);
            self.tracked.remove(finished);
        }
    }

    fn finish_task(&mut self, task: &str, state: ChildState, at: u64) {
        if let Some(tracked) = self.tracked.iter_mut().find(|tracked| {
            tracked.task.as_deref() == Some(task) && tracked.state == ChildState::Running
        }) {
            tracked.state = state;
            tracked.at = at;
        }
    }

    fn notice(&mut self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        let tag = |name: &str| {
            let open = format!("<{name}>");
            let start = text.find(&open)? + open.len();
            let end = start + text[start..].find(&format!("</{name}>"))?;
            Some(text[start..end].to_string())
        };
        let (Some(id), Some(status)) = (tag("tool-use-id"), tag("status")) else {
            return;
        };
        let state = match status.as_str() {
            "completed" => ChildState::Done,
            "failed" => ChildState::Failed,
            "killed" | "stopped" => ChildState::Stopped,
            _ => return,
        };
        let at = notice_time(&text);
        if let Some(tracked) = self
            .tracked
            .iter_mut()
            .find(|tracked| tracked.id == id && tracked.state == ChildState::Running)
        {
            tracked.state = state;
            tracked.at = at.unwrap_or(tracked.at);
        }
    }

    fn count(&mut self, message: &Message, claims: &mut HashSet<String>) {
        let (Some(id), Some(usage), Some(model)) = (&message.id, &message.usage, &message.model)
        else {
            return;
        };
        if model.starts_with('<') {
            return;
        }
        let (write_5m, write_1h) = match &usage.cache_creation {
            Some(split) => (
                split.ephemeral_5m_input_tokens,
                split.ephemeral_1h_input_tokens,
            ),
            None => (usage.cache_creation_input_tokens, 0),
        };
        let price = pricing::cost_micros(
            model,
            &Usage {
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read_input_tokens,
                cache_write_5m: write_5m,
                cache_write_1h: write_1h,
                fast: usage.speed.as_deref() == Some("fast"),
            },
        );
        self.context_tokens = usage.input_tokens
            + usage.cache_read_input_tokens
            + usage.cache_creation_input_tokens
            + usage.output_tokens;
        self.model = Some(model.clone());
        self.ledger.count(
            id,
            Counted {
                tokens: Tokens {
                    input: usage.input_tokens,
                    output: usage.output_tokens,
                    cache_read: usage.cache_read_input_tokens,
                    cache_write: usage.cache_creation_input_tokens,
                },
                cost: price.unwrap_or(0),
                priced: price.is_some(),
            },
            claims,
        );
    }
}

fn notice_time(line: &str) -> Option<u64> {
    let start = line.find("\"timestamp\":\"")? + "\"timestamp\":\"".len();
    let end = start + line[start..].find('"')?;
    unix_secs(&line[start..end])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::tail::tests::{append, temp};
    use super::*;

    const NOW: &str = "2026-11-14T01:00:00.000Z";
    const TOOL: &str = r#"{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}"#;

    fn assistant(id: &str, blocks: &str, output: u64, stop: &str, timestamp: &str) -> String {
        format!(
            r#"{{"parentUuid":"x","type":"assistant","timestamp":"{timestamp}","message":{{"model":"claude-opus-5-5","id":"{id}","type":"message","role":"assistant","stop_reason":"{stop}","content":[{blocks}],"usage":{{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":1000,"output_tokens":{output},"cache_creation":{{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}},"speed":"standard"}}}}}}"#
        )
    }

    fn result(tool_use_id: &str) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{NOW}","message":{{"role":"user","content":[{{"tool_use_id":"{tool_use_id}","type":"tool_result","content":"ok"}}]}}}}"#
        )
    }

    fn read(transcript: &mut Transcript) {
        transcript
            .advance(&mut u64::MAX.clone(), &mut HashSet::new())
            .unwrap();
    }

    #[test]
    fn a_response_counts_once_at_its_largest_output() {
        let path = temp("counts");
        let thinking = r#"{"type":"thinking","thinking":""}"#;
        append(
            &path,
            &format!(
                "{}\n{}\n",
                assistant("msg_1", thinking, 100, "tool_use", NOW),
                assistant("msg_1", TOOL, 140, "tool_use", NOW)
            ),
        );
        append(&path, "{\"type\":\"custom-title\",\"title\":\"ignored\"}\n");
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);

        assert_eq!(
            transcript.state.ledger.tokens,
            Tokens {
                input: 10,
                output: 140,
                cache_read: 1_000,
                cache_write: 0
            }
        );
        assert_eq!(transcript.state.ledger.cost_micros, 40 + 2_800 + 200);
        assert_eq!(transcript.state.context_tokens, 1_150);
        assert_eq!(transcript.running_tool(), Some("Bash"));

        append(&path, &format!("{}\n", result("toolu_1")));
        read(&mut transcript);
        assert_eq!(transcript.running_tool(), None);
        fs::remove_file(path).unwrap();
    }

    fn children(transcript: &Transcript, now: u64) -> Vec<(String, ChildKind, String, ChildState)> {
        transcript
            .children(now)
            .into_iter()
            .map(|child| (child.id, child.kind, child.label, child.state))
            .collect()
    }

    const T0: u64 = 1_794_787_200;

    #[test]
    fn subagents_and_background_shells_are_followed_until_they_finish() {
        let path = temp("children");
        let explore = r#"{"type":"tool_use","id":"toolu_a","name":"Agent","input":{"description":"Find the parser","subagent_type":"Explore","prompt":"kept out"}}"#;
        let shell = r#"{"type":"tool_use","id":"toolu_b","name":"Bash","input":{"command":"pnpm dev\nmore","run_in_background":true}}"#;
        let quick = r#"{"type":"tool_use","id":"toolu_c","name":"Bash","input":{"command":"ls"}}"#;
        let started = "2026-11-16T00:00:00.000Z";
        append(
            &path,
            &format!(
                "{}\n",
                assistant(
                    "msg_1",
                    &format!("{explore},{shell},{quick}"),
                    10,
                    "tool_use",
                    started
                )
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(
            children(&transcript, T0),
            [
                (
                    "toolu_a".into(),
                    ChildKind::Subagent,
                    "Find the parser".into(),
                    ChildState::Running
                ),
                (
                    "toolu_b".into(),
                    ChildKind::Shell,
                    "pnpm dev".into(),
                    ChildState::Running
                ),
            ]
        );
        append(
            &path,
            &format!(
                "{}\n{}\n",
                r#"{"type":"user","timestamp":"2026-11-16T00:01:00.000Z","toolUseResult":{"stdout":"","backgroundTaskId":"bash_1"},"message":{"role":"user","content":[{"tool_use_id":"toolu_b","type":"tool_result","content":"started"}]}}"#,
                r#"{"type":"user","timestamp":"2026-11-16T00:02:00.000Z","toolUseResult":{"status":"completed"},"message":{"role":"user","content":[{"tool_use_id":"toolu_a","type":"tool_result","content":"found it"}]}}"#
            ),
        );
        read(&mut transcript);
        let states: Vec<ChildState> = children(&transcript, T0 + 120)
            .into_iter()
            .map(|child| child.3)
            .collect();
        assert_eq!(states, [ChildState::Done, ChildState::Running]);
        append(
            &path,
            r#"{"type":"queue-operation","timestamp":"2026-11-16T00:03:00.000Z","content":"<task-notification>\n<task-id>bash_1</task-id>\n<tool-use-id>toolu_b</tool-use-id>\n<status>failed</status>\n</task-notification>"}
"#,
        );
        read(&mut transcript);
        assert_eq!(children(&transcript, T0 + 180)[1].3, ChildState::Failed);
        assert!(children(&transcript, T0 + 180 + 601).is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_background_subagent_runs_until_stopped_and_an_interrupted_turn_stops_the_rest() {
        let path = temp("async-children");
        let background = r#"{"type":"tool_use","id":"toolu_x","name":"Agent","input":{"description":"Review the diff","run_in_background":true}}"#;
        let inline = r#"{"type":"tool_use","id":"toolu_y","name":"Task","input":{"description":"Run the tests"}}"#;
        append(
            &path,
            &format!(
                "{}\n{}\n",
                assistant(
                    "msg_2",
                    &format!("{background},{inline}"),
                    10,
                    "tool_use",
                    "2026-11-16T00:00:00.000Z"
                ),
                r#"{"type":"user","timestamp":"2026-11-16T00:00:01.000Z","toolUseResult":{"isAsync":true,"agentId":"agent_9","status":"async_launched"},"message":{"role":"user","content":[{"tool_use_id":"toolu_x","type":"tool_result","content":"launched"}]}}"#
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        append(
            &path,
            &format!(
                "{}\n",
                assistant("msg_3", "", 5, "end_turn", "2026-11-16T00:00:05.000Z")
            ),
        );
        read(&mut transcript);
        let states: Vec<ChildState> = children(&transcript, T0 + 5)
            .into_iter()
            .map(|child| child.3)
            .collect();
        assert_eq!(states, [ChildState::Running, ChildState::Stopped]);
        let stop =
            r#"{"type":"tool_use","id":"toolu_z","name":"TaskStop","input":{"task_id":"agent_9"}}"#;
        append(
            &path,
            &format!(
                "{}\n",
                assistant("msg_4", stop, 5, "tool_use", "2026-11-16T00:00:09.000Z")
            ),
        );
        read(&mut transcript);
        assert_eq!(children(&transcript, T0 + 9)[0].3, ChildState::Stopped);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_subagent_that_errors_reads_as_failed() {
        let path = temp("failed-child");
        let agent = r#"{"type":"tool_use","id":"toolu_f","name":"Agent","input":{}}"#;
        append(
            &path,
            &format!(
                "{}\n{}\n",
                assistant("msg_5", agent, 10, "tool_use", "2026-11-16T00:00:00.000Z"),
                r#"{"type":"user","timestamp":"2026-11-16T00:00:02.000Z","toolUseResult":"Error: boom","message":{"role":"user","content":[{"tool_use_id":"toolu_f","type":"tool_result","is_error":true,"content":"boom"}]}}"#
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(
            children(&transcript, T0 + 2),
            [(
                "toolu_f".into(),
                ChildKind::Subagent,
                "subagent".into(),
                ChildState::Failed
            )]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_tool_call_with_an_unusual_description_still_counts_its_message() {
        let path = temp("odd-description");
        let odd = r#"{"type":"tool_use","id":"toolu_m","name":"mcp__x__y","input":{"description":{"nested":true},"subagent_type":null}}"#;
        append(
            &path,
            &format!("{}\n", assistant("msg_o", odd, 70, "tool_use", NOW)),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(transcript.state.ledger.tokens.output, 70);
        assert_eq!(transcript.running_tool(), Some("mcp__x__y"));
        assert!(transcript.children(0).is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_response_copied_into_a_forked_session_counts_only_once() {
        let original = temp("original");
        let fork = temp("fork");
        append(
            &original,
            &format!("{}\n", assistant("shared", "", 500, "end_turn", NOW)),
        );
        append(
            &fork,
            &format!(
                "{}\n{}\n",
                assistant("shared", "", 500, "end_turn", NOW),
                assistant("own", "", 7, "end_turn", NOW)
            ),
        );
        let mut claims = HashSet::new();
        let mut first = Transcript::new(original.clone());
        let mut second = Transcript::new(fork.clone());
        first.advance(&mut u64::MAX.clone(), &mut claims).unwrap();
        second.advance(&mut u64::MAX.clone(), &mut claims).unwrap();
        assert_eq!(
            (
                first.state.ledger.tokens.output,
                second.state.ledger.tokens.output
            ),
            (500, 7)
        );
        second.adopt(&mut claims);
        assert_eq!(second.state.ledger.tokens.output, 7);
        first.release(&mut claims);
        assert!(!claims.contains("shared") && claims.contains("own"));
        second.adopt(&mut claims);
        assert_eq!(second.state.ledger.tokens.output, 507);
        assert!(claims.contains("shared"));
        let mut after_restart = Transcript::new(fork.clone());
        after_restart
            .advance(&mut u64::MAX.clone(), &mut HashSet::new())
            .unwrap();
        assert_eq!(
            after_restart.state.ledger.tokens.output,
            second.state.ledger.tokens.output
        );
        fs::remove_file(original).unwrap();
        fs::remove_file(fork).unwrap();
    }

    #[test]
    fn a_finished_turn_clears_tool_calls_that_never_got_a_result() {
        let path = temp("interrupted");
        append(
            &path,
            &format!(
                "{}\n{}\n",
                assistant("a", TOOL, 1, "tool_use", NOW),
                assistant("b", r#"{"type":"text","text":"done"}"#, 1, "end_turn", NOW)
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(transcript.running_tool(), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_replaced_file_is_read_again_from_the_start() {
        let path = temp("rewritten");
        append(
            &path,
            &format!("{}\n", assistant("msg_a", "", 1, "end_turn", NOW)),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        let replacement = temp("replacement");
        append(
            &replacement,
            &format!(
                "{}\n{}\n",
                assistant("msg_b", "", 5, "end_turn", NOW),
                assistant("msg_c", "", 5, "end_turn", NOW)
            ),
        );
        fs::rename(&replacement, &path).unwrap();
        read(&mut transcript);
        assert_eq!(transcript.state.ledger.tokens.output, 10);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn content_of_any_shape_is_accepted_without_keeping_it() {
        let line = |content: &str| {
            format!(r#"{{"type":"user","message":{{"role":"user","content":{content}}}}}"#)
        };
        for content in [
            r#""a prompt""#,
            "null",
            r#"{"odd":[1,2]}"#,
            r#"[{"type":"text","text":"hi","extra":{"deep":[1]}}]"#,
        ] {
            let parsed: Line = serde_json::from_str(&line(content)).unwrap();
            assert!(parsed.message.unwrap().content.len() <= 1);
        }
    }

    #[test]
    fn unknown_models_count_tokens_but_flag_the_cost() {
        let path = temp("unknown");
        append(
            &path,
            &format!(
                "{}\n",
                assistant("msg_x", "", 1, "end_turn", NOW)
                    .replace("claude-opus-5-5", "claude-future-9")
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert!(transcript.state.ledger.unpriced);
        assert_eq!(transcript.state.ledger.tokens.output, 1);
        fs::remove_file(path).unwrap();
    }
}
