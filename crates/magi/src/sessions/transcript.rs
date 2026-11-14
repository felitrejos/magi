use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::pricing::{self, Usage};
use crate::proto::Tokens;

const CHUNK_BYTES: usize = 256 * 1024;
const MAX_LINE_BYTES: usize = 16 << 20;
const ASSISTANT_MARK: &[u8] = b"\"type\":\"assistant\"";
const RESULT_MARK: &[u8] = b"\"tool_result\"";

pub struct Transcript {
    path: PathBuf,
    file_id: Option<(u64, u64)>,
    offset: u64,
    pending: Vec<u8>,
    skipping_line: bool,
    counted: HashMap<String, Counted>,
    copied: HashMap<String, Counted>,
    pub model: Option<String>,
    pub tokens: Tokens,
    pub cost_micros: u64,
    pub unpriced: bool,
    pub context_tokens: u64,
    open_tools: Vec<(String, String)>,
}

struct Counted {
    tokens: Tokens,
    cost: u64,
    priced: bool,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: String,
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
            path,
            file_id: None,
            offset: 0,
            pending: Vec::new(),
            skipping_line: false,
            counted: HashMap::new(),
            copied: HashMap::new(),
            model: None,
            tokens: Tokens::default(),
            cost_micros: 0,
            unpriced: false,
            context_tokens: 0,
            open_tools: Vec::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn running_tool(&self) -> Option<&str> {
        self.open_tools.last().map(|(_, name)| name.as_str())
    }

    pub fn release(&self, claims: &mut HashSet<String>) {
        for id in self.counted.keys() {
            claims.remove(id);
        }
    }

    pub fn adopt(&mut self, claims: &mut HashSet<String>) {
        for (id, counted) in std::mem::take(&mut self.copied) {
            if claims.insert(id.clone()) {
                self.add(&counted);
                self.counted.insert(id, counted);
            } else {
                self.copied.insert(id, counted);
            }
        }
    }

    fn add(&mut self, counted: &Counted) {
        self.tokens = self.tokens + counted.tokens;
        self.cost_micros += counted.cost;
        self.unpriced |= !counted.priced;
    }

    pub fn advance(&mut self, budget: &mut u64, claims: &mut HashSet<String>) -> io::Result<()> {
        let metadata = fs::metadata(&self.path)?;
        let file_id = Some((metadata.dev(), metadata.ino()));
        if metadata.len() < self.offset || (self.file_id.is_some() && self.file_id != file_id) {
            self.release(claims);
            *self = Transcript::new(self.path.clone());
        }
        self.file_id = file_id;
        if metadata.len() == self.offset || *budget == 0 {
            return Ok(());
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = vec![0; CHUNK_BYTES];
        let mut remaining = (metadata.len() - self.offset).min(*budget);
        while remaining > 0 {
            let wanted = remaining.min(CHUNK_BYTES as u64) as usize;
            let read = file.read(&mut chunk[..wanted])?;
            if read == 0 {
                break;
            }
            self.offset += read as u64;
            remaining -= read as u64;
            *budget -= read as u64;
            self.take(&chunk[..read], claims);
        }
        Ok(())
    }

    fn take(&mut self, bytes: &[u8], claims: &mut HashSet<String>) {
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            self.hold(bytes);
            return;
        };
        let mut complete = std::mem::take(&mut self.pending);
        complete.extend_from_slice(&bytes[..end]);
        let mut lines = complete.split(|byte| *byte == b'\n');
        if std::mem::take(&mut self.skipping_line) {
            lines.next();
        }
        for line in lines {
            self.read_line(line, claims);
        }
        self.hold(&bytes[end + 1..]);
    }

    fn hold(&mut self, bytes: &[u8]) {
        if self.skipping_line {
            return;
        }
        if self.pending.len() + bytes.len() > MAX_LINE_BYTES {
            self.pending = Vec::new();
            self.skipping_line = true;
            return;
        }
        self.pending.extend_from_slice(bytes);
    }

    fn read_line(&mut self, line: &[u8], claims: &mut HashSet<String>) {
        if !contains(line, ASSISTANT_MARK) && !contains(line, RESULT_MARK) {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<Line>(line) else {
            return;
        };
        let Some(message) = parsed.message else {
            return;
        };
        match parsed.kind.as_str() {
            "assistant" => {
                for block in message
                    .content
                    .iter()
                    .filter(|block| block.kind == "tool_use")
                {
                    if let (Some(id), Some(name)) = (&block.id, &block.name) {
                        self.open_tools.push((id.clone(), name.clone()));
                    }
                }
                if message.stop_reason.as_deref() == Some("end_turn") {
                    self.open_tools.clear();
                }
                self.count(&message, claims);
            }
            "user" => {
                for id in message
                    .content
                    .iter()
                    .filter(|block| block.kind == "tool_result")
                    .filter_map(|block| block.tool_use_id.as_ref())
                {
                    self.open_tools.retain(|(open, _)| open != id);
                }
            }
            _ => {}
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
        let seen = Counted {
            tokens: Tokens {
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read_input_tokens,
                cache_write: usage.cache_creation_input_tokens,
            },
            cost: price.unwrap_or(0),
            priced: price.is_some(),
        };
        if let Some(copy) = self.copied.get_mut(id) {
            copy.keep_larger(seen);
            return;
        }
        match self.counted.get_mut(id) {
            Some(counted) => {
                let before = (counted.tokens.output, counted.cost);
                counted.keep_larger(seen);
                self.tokens.output += counted.tokens.output - before.0;
                self.cost_micros += counted.cost - before.1;
                self.unpriced |= !counted.priced;
            }
            None if claims.insert(id.clone()) => {
                self.add(&seen);
                self.counted.insert(id.clone(), seen);
            }
            None => {
                self.copied.insert(id.clone(), seen);
            }
        }
    }
}

impl Counted {
    fn keep_larger(&mut self, seen: Counted) {
        if seen.tokens.output > self.tokens.output {
            self.tokens.output = seen.tokens.output;
            self.cost = self.cost.max(seen.cost);
            self.priced &= seen.priced;
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

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

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "magi-transcript-{}-{name}.jsonl",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    fn append(path: &Path, text: &str) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
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
            transcript.tokens,
            Tokens {
                input: 10,
                output: 140,
                cache_read: 1_000,
                cache_write: 0
            }
        );
        assert_eq!(transcript.cost_micros, 40 + 2_800 + 200);
        assert_eq!(transcript.context_tokens, 1_150);
        assert_eq!(transcript.running_tool(), Some("Bash"));

        append(&path, &format!("{}\n", result("toolu_1")));
        read(&mut transcript);
        assert_eq!(transcript.running_tool(), None);
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
        assert_eq!((first.tokens.output, second.tokens.output), (500, 7));
        second.adopt(&mut claims);
        assert_eq!(second.tokens.output, 7);
        first.release(&mut claims);
        assert!(!claims.contains("shared") && claims.contains("own"));
        second.adopt(&mut claims);
        assert_eq!(second.tokens.output, 507);
        assert!(claims.contains("shared"));
        let mut after_restart = Transcript::new(fork.clone());
        after_restart
            .advance(&mut u64::MAX.clone(), &mut HashSet::new())
            .unwrap();
        assert_eq!(after_restart.tokens.output, second.tokens.output);
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
    fn a_half_written_line_waits_for_its_end() {
        let path = temp("partial");
        let line = assistant(
            "msg_2",
            r#"{"type":"text","text":"hi"}"#,
            2,
            "end_turn",
            NOW,
        );
        let (head, tail) = line.split_at(line.len() / 2);
        append(&path, head);
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(transcript.tokens.output, 0);
        append(&path, &format!("{tail}\n"));
        read(&mut transcript);
        assert_eq!(transcript.tokens.output, 2);
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
        assert_eq!(transcript.tokens.output, 10);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn the_byte_budget_spreads_a_long_catch_up_over_several_polls() {
        let path = temp("budget");
        let line = assistant("x", "", 1, "end_turn", NOW);
        for index in 0..100 {
            append(
                &path,
                &format!(
                    "{}\n",
                    line.replace("\"id\":\"x\"", &format!("\"id\":\"m{index}\""))
                ),
            );
        }
        let mut transcript = Transcript::new(path.clone());
        let mut budget = (line.len() as u64 + 2) * 10;
        transcript
            .advance(&mut budget, &mut HashSet::new())
            .unwrap();
        assert!(
            transcript.tokens.output >= 9 && transcript.tokens.output <= 10,
            "{}",
            transcript.tokens.output
        );
        assert_eq!(budget, 0);
        read(&mut transcript);
        assert_eq!(transcript.tokens.output, 100);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_line_too_long_to_hold_is_skipped_and_reading_resumes_after_it() {
        let path = temp("huge");
        let huge =
            result("toolu_1").replace("\"ok\"", &format!("\"{}\"", "x".repeat(MAX_LINE_BYTES)));
        append(
            &path,
            &format!(
                "{}\n{huge}\n{}\n",
                assistant("before", "", 3, "end_turn", NOW),
                assistant("after", "", 4, "end_turn", NOW)
            ),
        );
        let mut transcript = Transcript::new(path.clone());
        read(&mut transcript);
        assert_eq!(transcript.tokens.output, 7);
        assert!(transcript.pending.is_empty() && !transcript.skipping_line);
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
        assert!(transcript.unpriced);
        assert_eq!(transcript.tokens.output, 1);
        fs::remove_file(path).unwrap();
    }
}
