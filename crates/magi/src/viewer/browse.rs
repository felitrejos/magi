use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::handoff;
use crate::proto::OutputFile;

const BROWSE: &str = r#"cd "$0" || exit 1
export MAGI_TOOL="$2" MAGI_PALETTE="$5"
"$3" --ansi --delimiter='	' --with-nth=1,2 --header="$1" --header-first \
  --layout=reverse --info=inline-right --prompt='filter files > ' --no-sort --exact \
  --keep-right --footer='↑↓ pick a file · esc close' \
  --preview="$4" \
  --preview-window='right,62%,border-left,<60(down,65%,border-top)' \
  --bind=enter:ignore < files
cd / && rm -rf "$0""#;
const DELTA_PREVIEW: &str = r#""$MAGI_TOOL" --paging=never --width=$FZF_PREVIEW_COLUMNS < {3}"#;
const MAGI_PREVIEW: &str = r#""$MAGI_TOOL" md --width $FZF_PREVIEW_COLUMNS {3}"#;
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

struct Entry {
    left: String,
    label: String,
    file: String,
    text: String,
}

pub fn diff(diff: &str, title: &str, popups: &Path) -> io::Result<Option<Vec<String>>> {
    let dirs = handoff::search_dirs();
    let (Some(fzf), Some(delta)) = (handoff::find("fzf", &dirs), handoff::find("delta", &dirs))
    else {
        return Ok(None);
    };
    let sections = sections(diff);
    if sections.is_empty() {
        return Ok(None);
    }
    let header = format!(
        "{title} · {}, +{} −{}",
        files(sections.len()),
        sections.iter().map(|section| section.added).sum::<u32>(),
        sections.iter().map(|section| section.removed).sum::<u32>()
    );
    let entries = sections
        .into_iter()
        .enumerate()
        .map(|(index, section)| Entry {
            left: format!(
                "\x1b[32m+{}\x1b[0m \x1b[31m−{}\x1b[0m",
                section.added, section.removed
            ),
            label: section.path,
            file: format!("{index}.diff"),
            text: section.text,
        })
        .collect();
    let browser = Browser {
        fzf: &fzf,
        tool: &delta,
        preview: DELTA_PREVIEW,
        palette: "",
    };
    browser.open(entries, &header, popups).map(Some)
}

pub fn outputs(
    outputs: &[OutputFile],
    title: &str,
    palette: &str,
    popups: &Path,
) -> io::Result<Option<Vec<String>>> {
    let Some(fzf) = handoff::find("fzf", &handoff::search_dirs()) else {
        return Ok(None);
    };
    let magi = std::env::current_exe()?;
    let entries = outputs
        .iter()
        .enumerate()
        .map(|(index, output)| {
            let extension = Path::new(&output.path)
                .extension()
                .and_then(|extension| extension.to_str())
                .filter(|extension| extension.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("md");
            Entry {
                left: format!("\x1b[2m{:>5}\x1b[0m", lines(&output.text)),
                label: output.path.clone(),
                file: format!("{index}.{extension}"),
                text: output.text.clone(),
            }
        })
        .collect();
    let browser = Browser {
        fzf: &fzf,
        tool: &magi,
        preview: MAGI_PREVIEW,
        palette,
    };
    let header = format!("{title} · {}", files(outputs.len()));
    browser.open(entries, &header, popups).map(Some)
}

fn files(count: usize) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

fn lines(text: &str) -> String {
    let count = text.lines().count();
    if count == 1 {
        "1 line".into()
    } else {
        format!("{count} lines")
    }
}

struct Browser<'a> {
    fzf: &'a Path,
    tool: &'a Path,
    preview: &'a str,
    palette: &'a str,
}

impl Browser<'_> {
    fn open(&self, entries: Vec<Entry>, header: &str, popups: &Path) -> io::Result<Vec<String>> {
        let dir = fresh_dir(popups)?;
        let mut list = String::new();
        for entry in entries {
            write_private(&dir.join(&entry.file), &entry.text)?;
            let label: String = entry.label.chars().filter(|c| !c.is_control()).collect();
            list.push_str(&format!("{}\t{label}\t{}\n", entry.left, entry.file));
        }
        write_private(&dir.join("files"), &list)?;
        Ok(vec![
            "sh".into(),
            "-c".into(),
            BROWSE.into(),
            dir.to_string_lossy().into_owned(),
            header.to_string(),
            self.tool.to_string_lossy().into_owned(),
            self.fzf.to_string_lossy().into_owned(),
            self.preview.to_string(),
            self.palette.to_string(),
        ])
    }
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
    fn outputs_list_each_document_by_its_path_and_preview_it_with_magi() {
        let popups = std::env::temp_dir().join(format!("magi-outputs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&popups);
        let files = [
            OutputFile {
                path: "Plan".into(),
                text: "1. Ship it".into(),
            },
            OutputFile {
                path: "docs/notes.txt".into(),
                text: "a\nb".into(),
            },
        ];
        let Some(command) = outputs(&files, "Outputs", "palette", &popups).unwrap() else {
            return;
        };
        let dir = PathBuf::from(&command[3]);
        assert_eq!(command[4], "Outputs · 2 files");
        assert_eq!(command[7], MAGI_PREVIEW);
        assert_eq!(command[8], "palette");
        let list = fs::read_to_string(dir.join("files")).unwrap();
        assert!(list.contains("1 line\x1b[0m\tPlan\t0.md\n"), "{list:?}");
        assert!(list.contains("\tdocs/notes.txt\t1.txt\n"), "{list:?}");
        assert_eq!(fs::read_to_string(dir.join("1.txt")).unwrap(), "a\nb");
        fs::remove_dir_all(popups).unwrap();
    }

    #[test]
    fn the_browser_writes_each_file_privately_and_removes_them_when_it_closes() {
        let popups = std::env::temp_dir().join(format!("magi-diff-{}", std::process::id()));
        let _ = fs::remove_dir_all(&popups);
        let Some(command) = diff("diff --git a/x b/x\n+y\n", "fix", &popups).unwrap() else {
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
                .ends_with("\tx\t0.diff\n")
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
