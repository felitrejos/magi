use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MAX_META_BYTES: u64 = 16 * 1024;
const OUTPUT_TAIL_BYTES: u64 = 48 * 1024;
const OUTPUT_TAIL_LINES: usize = 200;
const TEMP_ROOTS: [&str; 2] = ["/tmp", "/private/tmp"];

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "toolUseId")]
    tool_use_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SubagentMeta {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub finished: bool,
}

pub fn subagent_meta(transcript: &Path) -> Option<SubagentMeta> {
    let name = transcript.file_stem()?.to_str()?;
    let path = transcript.with_file_name(format!("{name}.meta.json"));
    if fs::metadata(&path).ok()?.len() > MAX_META_BYTES {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub fn subagent_transcript(parent: &Path, tool_use_id: &str) -> Option<PathBuf> {
    let dir = parent.with_extension("").join("subagents");
    fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        let name = path.file_name()?.to_str()?;
        let agent = name.strip_suffix(".meta.json")?;
        if entry.metadata().ok()?.len() > MAX_META_BYTES {
            return None;
        }
        let meta: Meta = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
        (meta.tool_use_id.as_deref() == Some(tool_use_id))
            .then(|| path.with_file_name(format!("{agent}.jsonl")))
            .filter(|transcript| transcript.is_file())
    })
}

pub fn shell_output(session: &str, task: &str) -> Result<String, String> {
    if !safe(session) || !safe(task) {
        return Err("that command's output can't be found".into());
    }
    let user = format!("claude-{}", unsafe { libc::getuid() });
    let found = TEMP_ROOTS
        .iter()
        .filter_map(|root| fs::read_dir(Path::new(root).join(&user)).ok())
        .flat_map(|projects| projects.flatten())
        .map(|project| {
            project
                .path()
                .join(session)
                .join("tasks")
                .join(format!("{task}.output"))
        })
        .find(|path| path.is_file())
        .ok_or("its output isn't on this machine any more")?;
    tail(&found).map_err(|error| format!("can't read its output: {error}"))
}

fn safe(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn tail(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(OUTPUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(OUTPUT_TAIL_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let keep = lines.len().saturating_sub(OUTPUT_TAIL_LINES);
    Ok(lines[keep..].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-children-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_subagent_is_found_by_the_tool_call_that_started_it() {
        let dir = scratch("meta");
        let parent = dir.join("s1.jsonl");
        let subagents = dir.join("s1/subagents");
        fs::create_dir_all(&subagents).unwrap();
        fs::write(
            subagents.join("agent-a1.meta.json"),
            r#"{"toolUseId":"toolu_x","agentType":"Explore"}"#,
        )
        .unwrap();
        fs::write(subagents.join("agent-a1.jsonl"), "").unwrap();
        fs::write(
            subagents.join("agent-a2.meta.json"),
            r#"{"toolUseId":"toolu_y"}"#,
        )
        .unwrap();
        assert_eq!(
            subagent_transcript(&parent, "toolu_x"),
            Some(subagents.join("agent-a1.jsonl"))
        );
        assert_eq!(subagent_transcript(&parent, "toolu_y"), None);
        assert_eq!(subagent_transcript(&parent, "toolu_z"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_tail_of_long_output_starts_on_a_whole_line_and_odd_ids_are_refused() {
        let dir = scratch("tail");
        let path = dir.join("out");
        let long: String = (0..20_000).map(|n| format!("line {n}\n")).collect();
        fs::write(&path, long).unwrap();
        let tail = tail(&path).unwrap();
        assert_eq!(tail.lines().count(), OUTPUT_TAIL_LINES);
        assert!(tail.ends_with("line 19999"));
        assert!(tail.lines().all(|line| line.starts_with("line ")));
        assert!(shell_output("../etc", "passwd").is_err());
        assert!(shell_output("s1", "a/b").is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
