use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::handoff;

const BROWSE: &str = r#"cd "$0" || exit 1
export MAGI_DELTA="$2"
"$3" --ansi --delimiter='	' --with-nth=1,2 --header="$1" --header-first \
  --layout=reverse --info=inline-right --prompt='filter files > ' --no-sort --exact \
  --keep-right --footer='↑↓ pick a file · esc close' \
  --preview='"$MAGI_DELTA" --paging=never --width=$FZF_PREVIEW_COLUMNS < {3}.diff' \
  --preview-window='right,62%,border-left,<60(down,65%,border-top)' \
  --bind=enter:ignore < files
cd / && rm -rf "$0""#;
const NEW_FILE: &str = "new file, not yet added: ";

#[derive(Debug, PartialEq)]
struct Section {
    path: String,
    added: u32,
    removed: u32,
    text: String,
}

fn sections(diff: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            let path = header
                .split_once(" b/")
                .map_or(header, |(_, path)| path)
                .to_string();
            sections.push(Section {
                path,
                added: 0,
                removed: 0,
                text: String::new(),
            });
        } else if let Some(path) = line.strip_prefix(NEW_FILE) {
            sections.push(Section {
                path: path.to_string(),
                added: 0,
                removed: 0,
                text: format!("{line}\n"),
            });
            continue;
        }
        let Some(section) = sections.last_mut() else {
            continue;
        };
        if line.starts_with('+') && !line.starts_with("+++") {
            section.added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            section.removed += 1;
        }
        section.text.push_str(line);
        section.text.push('\n');
    }
    sections
}

pub fn browser(diff: &str, title: &str, popups: &Path) -> io::Result<Option<Vec<String>>> {
    let dirs = handoff::search_dirs();
    let (Some(fzf), Some(delta)) = (handoff::find("fzf", &dirs), handoff::find("delta", &dirs))
    else {
        return Ok(None);
    };
    let sections = sections(diff);
    if sections.is_empty() {
        return Ok(None);
    }
    let dir = fresh_dir(popups)?;
    let mut list = String::new();
    for (index, section) in sections.iter().enumerate() {
        write_private(&dir.join(format!("{index}.diff")), &section.text)?;
        let path: String = section.path.chars().filter(|c| !c.is_control()).collect();
        list.push_str(&format!(
            "\x1b[32m+{}\x1b[0m \x1b[31m−{}\x1b[0m\t{path}\t{index}\n",
            section.added, section.removed
        ));
    }
    write_private(&dir.join("files"), &list)?;
    let header = format!(
        "{title} · {} {}, +{} −{}",
        sections.len(),
        if sections.len() == 1 { "file" } else { "files" },
        sections.iter().map(|section| section.added).sum::<u32>(),
        sections.iter().map(|section| section.removed).sum::<u32>()
    );
    Ok(Some(vec![
        "sh".into(),
        "-c".into(),
        BROWSE.into(),
        dir.to_string_lossy().into_owned(),
        header,
        delta.to_string_lossy().into_owned(),
        fzf.to_string_lossy().into_owned(),
    ]))
}

fn fresh_dir(popups: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(popups)?;
    let dir = popups.join(format!(
        "diff-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_diff_splits_into_one_section_per_file_with_its_counts_and_new_files_last() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\ndiff --git a/b.md b/b.md\n+line\n\nnew file, not yet added: notes.txt\n";
        let found = sections(diff);
        let summary: Vec<(&str, u32, u32)> = found
            .iter()
            .map(|section| (section.path.as_str(), section.added, section.removed))
            .collect();
        assert_eq!(
            summary,
            [("src/a.rs", 2, 1), ("b.md", 1, 0), ("notes.txt", 0, 0)]
        );
        assert!(found[0].text.starts_with("diff --git a/src/a.rs"));
        assert!(!found[0].text.contains("b.md"));
        assert_eq!(found[2].text, "new file, not yet added: notes.txt\n");
    }

    #[test]
    fn the_browser_writes_each_file_privately_and_removes_them_when_it_closes() {
        let popups = std::env::temp_dir().join(format!("magi-diff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&popups);
        let Some(command) = browser("diff --git a/x b/x\n+y\n", "fix", &popups).unwrap() else {
            return;
        };
        let dir = PathBuf::from(&command[3]);
        assert_eq!(command[4], "fix · 1 file, +1 −0");
        assert_eq!(
            fs::read_to_string(dir.join("0.diff")).unwrap(),
            "diff --git a/x b/x\n+y\n"
        );
        assert!(
            fs::read_to_string(dir.join("files"))
                .unwrap()
                .ends_with("\tx\t0\n")
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(command[2].ends_with(r#"rm -rf "$0""#));
        fs::remove_dir_all(popups).unwrap();
    }
}
