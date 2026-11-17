use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MAX_META_BYTES: u64 = 16 * 1024;

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
}
