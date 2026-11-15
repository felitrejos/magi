use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::ledger::{Counted, Ledger};
use super::pricing::{self, Usage};
use super::tail::{self, Lines, Tail};
use crate::proto::Tokens;

const ASSISTANT_MARK: &[u8] = b"\"type\":\"assistant\"";
const RESULT_MARK: &[u8] = b"\"tool_result\"";

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
}

struct OpenTool {
    id: String,
    name: String,
    subagent: Option<Subagent>,
}

#[derive(Clone)]
pub struct Subagent {
    pub description: String,
    pub subagent_type: String,
}

#[derive(Deserialize)]
struct ToolInput {
    #[serde(default)]
    description: Option<serde_json::Value>,
    #[serde(default)]
    subagent_type: Option<serde_json::Value>,
}

impl ToolInput {
    fn subagent(&self) -> Subagent {
        let text = |value: &Option<serde_json::Value>| {
            value
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Subagent {
            description: text(&self.description),
            subagent_type: text(&self.subagent_type),
        }
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

    pub fn running_subagents(&self) -> impl Iterator<Item = &Subagent> {
        self.state
            .open_tools
            .iter()
            .filter_map(|open| open.subagent.as_ref())
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
        if !tail::contains(line, ASSISTANT_MARK) && !tail::contains(line, RESULT_MARK) {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<Line>(line) else {
            return;
        };
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
                        let subagent = SUBAGENT_TOOLS
                            .contains(&name.as_str())
                            .then(|| block.input.as_ref().map(ToolInput::subagent))
                            .flatten();
                        state.open_tools.push(OpenTool {
                            id: id.clone(),
                            name: name.clone(),
                            subagent,
                        });
                    }
                }
                state.ended_turn = message.stop_reason.as_deref() == Some("end_turn");
                if state.ended_turn {
                    state.open_tools.clear();
                }
                state.count(&message, self.claims);
            }
            "user" => {
                state.ended_turn = false;
                for id in message
                    .content
                    .iter()
                    .filter(|block| block.kind == "tool_result")
                    .filter_map(|block| block.tool_use_id.as_ref())
                {
                    state.open_tools.retain(|open| open.id != *id);
                }
            }
            _ => {}
        }
    }
}

impl Summary {
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

    #[test]
    fn open_subagent_calls_are_the_running_subagents_until_their_results_arrive() {
        let path = temp("subagents");
        let explore = r#"{"type":"tool_use","id":"toolu_a","name":"Agent","input":{"description":"Find the parser","subagent_type":"Explore","prompt":"a long prompt that is not kept"}}"#;
        let legacy = r#"{"type":"tool_use","id":"toolu_t","name":"Task","input":{"description":"Run the tests"}}"#;
        let write = r#"{"type":"tool_use","id":"toolu_w","name":"Write","input":{"file_path":"/x","content":"…"}}"#;
        append(
            &path,
            &format!(
                "{}\n",
                assistant(
                    "msg_s",
                    &format!("{explore},{legacy},{write}"),
                    10,
                    "tool_use",
                    NOW
                )
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        let running: Vec<(String, String)> = transcript
            .running_subagents()
            .map(|subagent| (subagent.description.clone(), subagent.subagent_type.clone()))
            .collect();
        assert_eq!(
            running,
            [
                ("Find the parser".into(), "Explore".into()),
                ("Run the tests".into(), String::new())
            ]
        );
        append(&path, &format!("{}\n", result("toolu_a")));
        read(&mut transcript);
        assert_eq!(transcript.running_subagents().count(), 1);
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
        assert_eq!(transcript.running_subagents().count(), 0);
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
