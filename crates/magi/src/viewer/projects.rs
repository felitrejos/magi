use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_NAME_CHARS: usize = 60;

#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Projects {
    #[serde(default)]
    repos: BTreeMap<String, String>,
    #[serde(default)]
    groups: BTreeMap<String, String>,
    #[serde(default)]
    suggested: BTreeMap<String, String>,
    #[serde(default)]
    known: Option<BTreeSet<String>>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub key: String,
    pub name: Option<String>,
}

pub fn automatic_key(repo: &str) -> String {
    repo.split('-').next().unwrap_or(repo).to_lowercase()
}

pub fn clean(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Projects {
    pub fn open(path: PathBuf) -> Self {
        let mut projects: Self = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        projects.path = Some(path);
        projects
    }

    pub fn resolve(&self, repo: &str) -> Resolved {
        let named = |name: &String| Resolved {
            key: name.to_lowercase(),
            name: Some(name.clone()),
        };
        if let Some(name) = self.repos.get(&repo.to_lowercase()) {
            return named(name);
        }
        let key = automatic_key(repo);
        if let Some(name) = self.groups.get(&key) {
            return named(name);
        }
        match self.suggested.get(&repo.to_lowercase()) {
            Some(name) => match self.groups.get(&name.to_lowercase()) {
                Some(renamed) => named(renamed),
                None => named(name),
            },
            None => Resolved { key, name: None },
        }
    }

    pub fn has_seen_any(&self) -> bool {
        self.known.is_some()
    }

    pub fn is_new(&self, repo: &str) -> bool {
        let repo = repo.to_lowercase();
        self.known
            .as_ref()
            .is_some_and(|known| !known.contains(&repo))
            && !self.repos.contains_key(&repo)
    }

    pub fn learn<'a>(&mut self, repos: impl IntoIterator<Item = &'a str>) -> io::Result<()> {
        let known = self.known.get_or_insert_default();
        known.extend(repos.into_iter().map(str::to_lowercase));
        self.save()
    }

    pub fn suggest(&mut self, asked: &[String], names: BTreeMap<String, String>) -> io::Result<()> {
        for (repo, name) in names {
            self.suggested.insert(repo.to_lowercase(), clean(&name));
        }
        self.learn(asked.iter().map(String::as_str))
    }

    pub fn move_repo(&mut self, repo: &str, name: &str) -> io::Result<()> {
        let name = clean(name);
        let repo = repo.to_lowercase();
        if name.is_empty() {
            self.repos.remove(&repo);
        } else {
            self.repos.insert(repo, name);
        }
        self.save()
    }

    pub fn rename(&mut self, key: &str, name: &str) -> io::Result<()> {
        let name = clean(name);
        if name.is_empty() {
            self.groups
                .retain(|group, named| group != key && named.to_lowercase() != key);
            self.repos.retain(|_, named| named.to_lowercase() != key);
            return self.save();
        }
        for named in self.repos.values_mut().chain(self.groups.values_mut()) {
            if named.to_lowercase() == key {
                named.clone_from(&name);
            }
        }
        self.groups.insert(key.to_string(), name);
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, json)?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(projects: &Projects, repo: &str) -> (String, Option<String>) {
        let Resolved { key, name } = projects.resolve(repo);
        (key, name)
    }

    #[test]
    fn related_repos_share_a_project_until_one_is_moved_and_a_move_can_be_undone() {
        let mut projects = Projects::default();
        assert_eq!(resolved(&projects, "stampr-ops"), ("stampr".into(), None));
        projects.move_repo("stampr-ops", "Ops").unwrap();
        assert_eq!(
            resolved(&projects, "Stampr-Ops"),
            ("ops".into(), Some("Ops".into()))
        );
        assert_eq!(resolved(&projects, "stampr"), ("stampr".into(), None));
        projects.move_repo("stampr-ops", "  ").unwrap();
        assert_eq!(resolved(&projects, "stampr-ops"), ("stampr".into(), None));
    }

    #[test]
    fn renaming_a_project_names_its_repos_and_renaming_onto_another_merges_them() {
        let mut projects = Projects::default();
        projects.move_repo("notes", "stampr").unwrap();
        projects.rename("stampr", "Stampr").unwrap();
        for repo in ["stampr", "stampr-web", "notes"] {
            assert_eq!(
                resolved(&projects, repo),
                ("stampr".into(), Some("Stampr".into())),
                "{repo}"
            );
        }
        projects.rename("autoscan", "Stampr").unwrap();
        assert_eq!(resolved(&projects, "autOScan-engine").0, "stampr");
        projects.rename("stampr", "").unwrap();
        assert_eq!(resolved(&projects, "notes"), ("notes".into(), None));
        assert_eq!(resolved(&projects, "stampr"), ("stampr".into(), None));
    }

    #[test]
    fn suggestions_place_new_repos_below_your_own_edits() {
        let mut projects = Projects::default();
        assert!(!projects.is_new("ping"));
        projects.learn(["magi"]).unwrap();
        assert!(projects.is_new("Ping") && !projects.is_new("magi"));
        projects
            .suggest(
                &["ping".into(), "stampr-site".into()],
                BTreeMap::from([
                    ("ping".into(), "Network Pong".into()),
                    ("stampr-site".into(), "magi".into()),
                ]),
            )
            .unwrap();
        assert!(!projects.is_new("ping"));
        assert_eq!(
            resolved(&projects, "ping"),
            ("network pong".into(), Some("Network Pong".into()))
        );
        assert_eq!(
            resolved(&projects, "stampr-site"),
            ("magi".into(), Some("magi".into()))
        );
        projects.rename("magi", "Monitor").unwrap();
        assert_eq!(
            resolved(&projects, "stampr-site"),
            ("monitor".into(), Some("Monitor".into()))
        );
        projects.move_repo("ping", "Games").unwrap();
        assert_eq!(resolved(&projects, "ping").1, Some("Games".into()));
    }

    #[test]
    fn edits_are_saved_privately_and_read_back() {
        let path = std::env::temp_dir().join(format!("magi-projects-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut projects = Projects::open(path.clone());
        projects.move_repo("ping", "Games\u{7}").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reopened = Projects::open(path.clone());
        assert_eq!(
            resolved(&reopened, "ping"),
            ("games".into(), Some("Games".into()))
        );
        fs::remove_file(path).unwrap();
    }
}
