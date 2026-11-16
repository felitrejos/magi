use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use crate::bounded;
use crate::proto::OutputFile;

const GIT_LIMIT: Duration = Duration::from_secs(10);
const MAX_DIFF_BYTES: usize = 900 << 10;
const MAX_OUTPUTS: usize = 40;
const MAX_OUTPUT_FILE_BYTES: usize = 256 << 10;
const MAX_OUTPUT_BYTES: usize = 900 << 10;
const MAX_MEDIA_BYTES: usize = 700 << 10;
const MEDIA: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "pdf"];
const DOCUMENTS: [&str; 7] = ["md", "markdown", "mdx", "txt", "rst", "html", "csv"];
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

pub fn outputs(transcript: &Path, cwd: &str) -> Result<Vec<OutputFile>, String> {
    let tail =
        read_tail(transcript).map_err(|error| format!("can't read the transcript: {error}"))?;
    let mut plan = None;
    let mut touched: Vec<String> = Vec::new();
    let mut touch = |path: &str| {
        let path = if path.starts_with('/') || cwd.is_empty() {
            path.to_string()
        } else {
            format!("{cwd}/{path}")
        };
        touched.retain(|known| *known != path);
        touched.push(path);
    };
    for line in tail.lines() {
        if !line.contains("\"tool_use\"") && !line.contains("apply_patch") {
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
                    plan = input["plan"]
                        .as_str()
                        .filter(|plan| !plan.trim().is_empty())
                        .map(str::to_string)
                        .or(plan);
                }
                Some("Write" | "Edit" | "MultiEdit" | "NotebookEdit") => {
                    if let Some(path) = input["file_path"].as_str() {
                        touch(path);
                    }
                }
                _ => {}
            }
        }
        let payload = &value["payload"];
        if payload["name"] == "apply_patch" {
            let patch = payload["input"]
                .as_str()
                .or_else(|| payload["arguments"].as_str())
                .unwrap_or_default();
            for path in patch.lines().filter_map(|line| {
                line.strip_prefix("*** Add File: ")
                    .or_else(|| line.strip_prefix("*** Update File: "))
            }) {
                touch(path.trim());
            }
        }
    }
    let mut files: Vec<OutputFile> = plan
        .map(|text| {
            let bytes = capped_text(text).into_bytes();
            OutputFile {
                path: "Plan".into(),
                size: bytes.len() as u64,
                bytes,
            }
        })
        .into_iter()
        .collect();
    let mut total = files.iter().map(|file| file.bytes.len()).sum::<usize>();
    let inside = format!("{}/", cwd.trim_end_matches('/'));
    for path in touched.iter().rev() {
        if files.len() >= MAX_OUTPUTS || total >= MAX_OUTPUT_BYTES {
            break;
        }
        let Some(relative) = path.strip_prefix(&inside).filter(|_| !cwd.is_empty()) else {
            continue;
        };
        let extension = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let text = DOCUMENTS.contains(&extension.as_str());
        if !text && !MEDIA.contains(&extension.as_str()) {
            continue;
        }
        let Ok(size) = fs::metadata(path).map(|metadata| metadata.len()) else {
            continue;
        };
        let bytes = if text {
            fs::read_to_string(path).map(|text| capped_text(text).into_bytes())
        } else if size as usize <= MAX_MEDIA_BYTES.min(MAX_OUTPUT_BYTES - total) {
            fs::read(path)
        } else {
            Ok(Vec::new())
        };
        let Ok(bytes) = bytes else {
            continue;
        };
        total += bytes.len();
        files.push(OutputFile {
            path: relative.to_string(),
            bytes,
            size,
        });
    }
    if files.is_empty() {
        return Err("this session hasn't made a plan or written a document yet".into());
    }
    Ok(files)
}

fn capped_text(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_FILE_BYTES {
        let mut cut = MAX_OUTPUT_FILE_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n\n… the rest is too long to show here\n");
    }
    text
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
    fn outputs_are_the_plan_then_the_documents_it_touched_inside_its_folder_newest_first() {
        let dir = scratch("outputs");
        let cwd = dir.to_str().unwrap();
        for (name, text) in [
            ("NOTES.md", "# Notes"),
            ("README.md", "# Read me"),
            ("main.rs", "fn main() {}"),
        ] {
            fs::write(dir.join(name), text).unwrap();
        }
        fs::write(dir.join("shot.png"), [137, 80, 78, 71]).unwrap();
        fs::write(dir.join("huge.pdf"), vec![0u8; MAX_MEDIA_BYTES + 1]).unwrap();
        let outside = scratch("outputs-outside").join("ELSEWHERE.md");
        fs::write(&outside, "away").unwrap();
        let write = |path: &std::path::Path| {
            tool(
                "Write",
                &format!(r#"{{"file_path":"{}","content":"x"}}"#, path.display()),
            )
        };
        let transcript = dir.join("s.jsonl");
        fs::write(
            &transcript,
            [
                write(&dir.join("README.md")),
                tool("ExitPlanMode", r#"{"plan":"1. Add retries"}"#),
                write(&dir.join("NOTES.md")),
                write(&dir.join("shot.png")),
                write(&dir.join("huge.pdf")),
                write(&dir.join("main.rs")),
                write(&outside),
                write(&dir.join("README.md")),
                write(&dir.join("GONE.md")),
            ]
            .join("\n"),
        )
        .unwrap();
        let found = outputs(&transcript, cwd).unwrap();
        let paths: Vec<&str> = found.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            ["Plan", "README.md", "huge.pdf", "shot.png", "NOTES.md"]
        );
        assert!(found[2].bytes.is_empty());
        assert_eq!(found[2].size, MAX_MEDIA_BYTES as u64 + 1);
        assert_eq!(found[3].bytes, [137, 80, 78, 71]);
        assert_eq!(found[0].bytes, b"1. Add retries");
        assert_eq!(found[1].bytes, b"# Read me");
        fs::write(&transcript, tool("Bash", r#"{"command":"ls"}"#)).unwrap();
        assert!(outputs(&transcript, cwd).is_err());
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
