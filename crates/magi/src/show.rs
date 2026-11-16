use std::process::{Command, Stdio};
use std::time::Duration;

use crate::bounded;

const GIT_LIMIT: Duration = Duration::from_secs(10);
const MAX_DIFF_BYTES: usize = 900 << 10;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-show-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
