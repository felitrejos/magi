use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::bounded;

const GIT_LIMIT: Duration = Duration::from_secs(10);
const MAX_DIFF_BYTES: usize = 4 << 20;
const MAX_FILE_BYTES: u64 = 1 << 20;
const TRANSCRIPT_TAIL_BYTES: u64 = 16 << 20;
const MAX_UNTRACKED: usize = 50;

pub fn diff(cwd: &str) -> Result<String, String> {
    if cwd.is_empty() {
        return Err("magi doesn't know which folder this session works in".into());
    }
    let changes = git(
        cwd,
        &["--no-pager", "diff", "HEAD", "--no-color", "--no-ext-diff"],
    )?;
    let untracked = git(cwd, &["ls-files", "--others", "--exclude-standard"])?;
    let new_files: Vec<&str> = untracked.lines().take(MAX_UNTRACKED).collect();
    if changes.trim().is_empty() && new_files.is_empty() {
        return Err("no changes since the last commit".into());
    }
    let mut text = changes;
    if text.len() > MAX_DIFF_BYTES {
        let mut cut = MAX_DIFF_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… the rest of the diff is too long to show\n");
    }
    for file in new_files {
        text.push_str(&format!("\nnew file, not yet added: {file}\n"));
    }
    Ok(text)
}

fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let output = bounded::output(
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null()),
        GIT_LIMIT,
    )
    .map_err(|error| format!("can't run git: {error}"))?;
    if !output.status.success() {
        return Err(format!("{cwd} isn't a git repository with a commit yet"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn plan(transcript: &Path) -> Result<(String, String), String> {
    let tail =
        read_tail(transcript).map_err(|error| format!("can't read the transcript: {error}"))?;
    let mut found: Option<(String, String)> = None;
    for line in tail.lines() {
        if !line.contains("\"tool_use\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        for block in value["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_use" {
                continue;
            }
            let input = &block["input"];
            match block["name"].as_str() {
                Some("ExitPlanMode") => {
                    if let Some(plan) = input["plan"]
                        .as_str()
                        .filter(|plan| !plan.trim().is_empty())
                    {
                        found = Some(("Plan".into(), plan.to_string()));
                    }
                }
                Some("Write" | "Edit" | "MultiEdit") => {
                    if let Some(path) = input["file_path"]
                        .as_str()
                        .filter(|path| path.ends_with(".md") || path.ends_with(".markdown"))
                    {
                        found = Some((path.to_string(), String::new()));
                    }
                }
                _ => {}
            }
        }
    }
    match found {
        Some((title, text)) if !text.is_empty() => Ok((title, text)),
        Some((path, _)) => {
            let file = Path::new(&path);
            let size = file
                .metadata()
                .map_err(|_| format!("{path} isn't there any more"))?
                .len();
            if size > MAX_FILE_BYTES {
                return Err(format!("{path} is too large to show"));
            }
            let text =
                fs::read_to_string(file).map_err(|error| format!("can't read {path}: {error}"))?;
            let name = file
                .file_name()
                .map_or(path.clone(), |name| name.to_string_lossy().into_owned());
            Ok((name, text))
        }
        None => Err("this session hasn't made a plan or written a Markdown file".into()),
    }
}

fn read_tail(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-show-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tool(name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"t","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    #[test]
    fn the_latest_plan_or_markdown_file_is_shown() {
        let dir = scratch("plan");
        let transcript = dir.join("s.jsonl");
        let notes = dir.join("NOTES.md");
        fs::write(&notes, "# Notes\nwritten").unwrap();
        fs::write(
            &transcript,
            [
                tool("ExitPlanMode", r#"{"plan":"1. Add retries\n2. Test them"}"#),
                tool("Bash", r#"{"command":"ls"}"#),
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(
            plan(&transcript).unwrap(),
            ("Plan".into(), "1. Add retries\n2. Test them".into())
        );
        let write = tool(
            "Write",
            &format!(r#"{{"file_path":"{}","content":"x"}}"#, notes.display()),
        );
        fs::write(
            &transcript,
            [tool("ExitPlanMode", r#"{"plan":"old"}"#), write].join("\n"),
        )
        .unwrap();
        assert_eq!(
            plan(&transcript).unwrap(),
            ("NOTES.md".into(), "# Notes\nwritten".into())
        );
        fs::write(&transcript, tool("Bash", r#"{"command":"ls"}"#)).unwrap();
        assert!(plan(&transcript).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_diff_lists_changes_and_new_files_and_says_when_there_are_none() {
        let dir = scratch("diff");
        let cwd = dir.to_str().unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(cwd)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "a.txt"]);
        run(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "first",
        ]);
        assert_eq!(diff(cwd), Err("no changes since the last commit".into()));
        fs::write(dir.join("a.txt"), "two\n").unwrap();
        fs::write(dir.join("b.txt"), "new\n").unwrap();
        let shown = diff(cwd).unwrap();
        assert!(shown.contains("-one\n+two"), "{shown}");
        assert!(shown.contains("new file, not yet added: b.txt"), "{shown}");
        assert!(diff(dir.join("missing").to_str().unwrap()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
