use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::proto::SharedProjects;

const MAX_NAME_CHARS: usize = 60;
const MAX_SHARED: usize = 2_000;

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
    #[serde(default)]
    folded: BTreeSet<String>,
    #[serde(default)]
    machines: BTreeMap<String, String>,
    #[serde(default)]
    zen: bool,
    #[serde(default)]
    updated_ms: u64,
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
        let edited = [
            &projects.repos,
            &projects.groups,
            &projects.suggested,
            &projects.machines,
        ]
        .iter()
        .any(|names| !names.is_empty());
        if projects.updated_ms == 0 && edited {
            projects.updated_ms = 1;
        }
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

    pub fn is_folded(&self, key: &str) -> bool {
        self.folded.contains(key)
    }

    pub fn toggle_fold(&mut self, key: &str) -> io::Result<()> {
        if !self.folded.remove(key) {
            self.folded.insert(key.to_string());
        }
        self.save()
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
        self.known
            .get_or_insert_default()
            .extend(asked.iter().map(|repo| repo.to_lowercase()));
        self.changed()
    }

    pub fn move_repo(&mut self, repo: &str, name: &str) -> io::Result<()> {
        let name = clean(name);
        let repo = repo.to_lowercase();
        if name.is_empty() {
            self.repos.remove(&repo);
        } else {
            self.repos.insert(repo, name);
        }
        self.changed()
    }

    pub fn rename(&mut self, key: &str, name: &str) -> io::Result<()> {
        let name = clean(name);
        if name.is_empty() {
            self.groups
                .retain(|group, named| group != key && named.to_lowercase() != key);
            self.repos.retain(|_, named| named.to_lowercase() != key);
            return self.changed();
        }
        for named in self.repos.values_mut().chain(self.groups.values_mut()) {
            if named.to_lowercase() == key {
                named.clone_from(&name);
            }
        }
        self.groups.insert(key.to_string(), name);
        self.changed()
    }

    pub fn zen(&self) -> bool {
        self.zen
    }

    pub fn set_zen(&mut self, zen: bool) -> io::Result<()> {
        self.zen = zen;
        self.save()
    }

    pub fn machine_label(&self, machine: &str) -> Option<&str> {
        self.machines
            .get(&machine.to_lowercase())
            .map(String::as_str)
    }

    pub fn label_machine(&mut self, machine: &str, label: &str) -> io::Result<()> {
        let label = clean(label);
        let machine = machine.to_lowercase();
        if label.is_empty() {
            self.machines.remove(&machine);
        } else {
            self.machines.insert(machine, label);
        }
        self.changed()
    }

    pub fn updated_ms(&self) -> u64 {
        self.updated_ms
    }

    pub fn shared(&self) -> SharedProjects {
        SharedProjects {
            updated_ms: self.updated_ms,
            repos: self.repos.clone(),
            groups: self.groups.clone(),
            suggested: self.suggested.clone(),
            machines: self.machines.clone(),
        }
    }

    pub fn adopt(&mut self, shared: SharedProjects) -> io::Result<bool> {
        let sizes = [
            shared.repos.len(),
            shared.groups.len(),
            shared.suggested.len(),
            shared.machines.len(),
        ];
        if shared.updated_ms <= self.updated_ms || sizes.iter().any(|size| *size > MAX_SHARED) {
            return Ok(false);
        }
        let tidy = |names: BTreeMap<String, String>| -> BTreeMap<String, String> {
            names
                .into_iter()
                .map(|(key, name)| (clean(&key).to_lowercase(), clean(&name)))
                .filter(|(key, name)| !key.is_empty() && !name.is_empty())
                .collect()
        };
        self.repos = tidy(shared.repos);
        self.groups = tidy(shared.groups);
        self.suggested = tidy(shared.suggested);
        self.machines = tidy(shared.machines);
        self.updated_ms = shared.updated_ms;
        self.save()?;
        Ok(true)
    }

    fn changed(&mut self) -> io::Result<()> {
        self.updated_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
            .max(self.updated_ms + 1);
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
    fn an_edit_is_stamped_to_share_and_only_a_newer_shared_set_replaces_ours() {
        let mut projects = Projects::default();
        projects.move_repo("ping", "Games").unwrap();
        projects.set_zen(true).unwrap();
        let mine = projects.shared();
        assert!(mine.updated_ms > 0);
        assert_eq!(mine.repos.get("ping").map(String::as_str), Some("Games"));
        let mut older = mine.clone();
        older.updated_ms -= 1;
        older.repos.clear();
        assert!(!projects.adopt(older).unwrap());
        let mut newer = mine.clone();
        newer.updated_ms += 10;
        newer
            .machines
            .insert("Homelab ".into(), " Server\u{7} ".into());
        newer.repos.clear();
        assert!(projects.adopt(newer).unwrap());
        assert_eq!(projects.machine_label("homelab"), Some("Server"));
        assert_eq!(resolved(&projects, "ping"), ("ping".into(), None));
        assert!(projects.zen());
        let mut flood = projects.shared();
        flood.updated_ms += 10;
        flood.repos = (0..=MAX_SHARED)
            .map(|n| (n.to_string(), "x".into()))
            .collect();
        assert!(!projects.adopt(flood).unwrap());
    }

    #[test]
    fn edits_saved_before_sharing_existed_are_shared_but_lose_to_any_new_edit() {
        let path =
            std::env::temp_dir().join(format!("magi-old-projects-{}.json", std::process::id()));
        fs::write(&path, r#"{"repos":{"ping":"Games"}}"#).unwrap();
        let projects = Projects::open(path.clone());
        assert_eq!(projects.updated_ms(), 1);
        fs::write(&path, "{}").unwrap();
        assert_eq!(Projects::open(path.clone()).updated_ms(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn edits_are_saved_privately_and_read_back() {
        let path = std::env::temp_dir().join(format!("magi-projects-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut projects = Projects::open(path.clone());
        projects.move_repo("ping", "Games\u{7}").unwrap();
        projects
            .label_machine("Felipes-MacBook-Pro", " MacBook ")
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reopened = Projects::open(path.clone());
        assert_eq!(
            resolved(&reopened, "ping"),
            ("games".into(), Some("Games".into()))
        );
        assert_eq!(
            reopened.machine_label("felipes-macbook-pro"),
            Some("MacBook")
        );
        fs::remove_file(path).unwrap();
    }
}
