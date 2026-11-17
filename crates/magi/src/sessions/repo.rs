use std::fs;
use std::path::{Path, PathBuf};

const MAX_NAME_CHARS: usize = 80;

#[derive(Debug, Default, PartialEq)]
pub struct Repo {
    pub project: String,
    pub branch: Option<String>,
}

pub fn locate(cwd: &str) -> Repo {
    let cwd = Path::new(cwd);
    for dir in cwd.ancestors() {
        let git = dir.join(".git");
        if git.is_dir() {
            return Repo {
                project: name(dir),
                branch: branch(&git),
            };
        }
        if let Some(gitdir) = linked_gitdir(&git) {
            return match main_repo(&gitdir) {
                Some(main) => Repo {
                    project: name(main),
                    branch: branch(&gitdir),
                },
                None => Repo {
                    project: name(dir),
                    branch: branch(&gitdir),
                },
            };
        }
    }
    Repo {
        project: name(cwd),
        ..Repo::default()
    }
}

fn linked_gitdir(git: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(git).ok()?;
    let target = Path::new(text.trim().strip_prefix("gitdir:")?.trim());
    Some(match git.parent() {
        Some(dir) if target.is_relative() => dir.join(target),
        _ => target.to_path_buf(),
    })
}

fn main_repo(gitdir: &Path) -> Option<&Path> {
    let worktrees = gitdir.parent()?;
    let common = worktrees.parent()?;
    let is_worktree = worktrees.file_name()? == "worktrees" && common.file_name()? == ".git";
    is_worktree.then(|| common.parent()).flatten()
}

fn branch(gitdir: &Path) -> Option<String> {
    let head = fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref: refs/heads/")?;
    Some(name.chars().take(MAX_NAME_CHARS).collect())
}

fn name(dir: &Path) -> String {
    let name = dir
        .file_name()
        .map_or_else(|| dir.to_string_lossy(), |name| name.to_string_lossy());
    name.chars().take(MAX_NAME_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("magi-repo-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_folder_inside_a_repo_belongs_to_that_repo_and_its_branch() {
        let root = scratch("inside");
        let repo = root.join("stampr");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/fix/retries\n");
        fs::create_dir_all(repo.join("src/api")).unwrap();
        assert_eq!(
            locate(repo.join("src/api").to_str().unwrap()),
            Repo {
                project: "stampr".into(),
                branch: Some("fix/retries".into()),
            }
        );
    }

    #[test]
    fn a_worktree_belongs_to_its_main_repo() {
        let root = scratch("worktree");
        let repo = root.join("magi");
        let tree = repo.join(".claude/worktrees/quiet-fox");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(
            &repo.join(".git/worktrees/quiet-fox/HEAD"),
            "ref: refs/heads/claude/quiet-fox\n",
        );
        write(
            &tree.join(".git"),
            &format!(
                "gitdir: {}\n",
                repo.join(".git/worktrees/quiet-fox").display()
            ),
        );
        assert_eq!(
            locate(tree.to_str().unwrap()),
            Repo {
                project: "magi".into(),
                branch: Some("claude/quiet-fox".into()),
            }
        );
    }

    #[test]
    fn a_detached_head_or_no_repo_has_no_branch() {
        let root = scratch("detached");
        let repo = root.join("ping");
        write(
            &repo.join(".git/HEAD"),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904\n",
        );
        assert_eq!(locate(repo.to_str().unwrap()).branch, None);
        let loose = root.join("notes");
        fs::create_dir_all(&loose).unwrap();
        assert_eq!(
            locate(loose.to_str().unwrap()),
            Repo {
                project: "notes".into(),
                ..Repo::default()
            }
        );
    }
}
