use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::proto::SharedLabels;

const MAX_NAME_CHARS: usize = 60;
const MAX_SHARED: usize = 2_000;

#[derive(Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Labels {
    #[serde(default)]
    machines: BTreeMap<String, String>,
    #[serde(default)]
    updated_ms: u64,
    #[serde(skip)]
    path: Option<PathBuf>,
}

fn clean(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

impl Labels {
    pub fn open(path: PathBuf) -> Self {
        let mut labels: Self = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        labels.path = Some(path);
        labels
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
        self.updated_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
            .max(self.updated_ms + 1);
        self.save()
    }

    pub fn updated_ms(&self) -> u64 {
        self.updated_ms
    }

    pub fn shared(&self) -> SharedLabels {
        SharedLabels {
            updated_ms: self.updated_ms,
            machines: self.machines.clone(),
        }
    }

    pub fn adopt(&mut self, shared: SharedLabels) -> io::Result<bool> {
        if shared.updated_ms <= self.updated_ms || shared.machines.len() > MAX_SHARED {
            return Ok(false);
        }
        self.machines = shared
            .machines
            .into_iter()
            .map(|(machine, label)| (clean(&machine).to_lowercase(), clean(&label)))
            .filter(|(machine, label)| !machine.is_empty() && !label.is_empty())
            .collect();
        self.updated_ms = shared.updated_ms;
        self.save()?;
        Ok(true)
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

    #[test]
    fn a_label_is_stamped_to_share_and_only_a_newer_shared_set_replaces_ours() {
        let mut labels = Labels::default();
        labels.label_machine("Homelab", "Server").unwrap();
        let mine = labels.shared();
        assert!(mine.updated_ms > 0);
        let mut older = mine.clone();
        older.updated_ms -= 1;
        older.machines.clear();
        assert!(!labels.adopt(older).unwrap());
        let mut newer = mine.clone();
        newer.updated_ms += 10;
        newer.machines.clear();
        newer.machines.insert("Mac ".into(), " Studio\u{7} ".into());
        assert!(labels.adopt(newer).unwrap());
        assert_eq!(labels.machine_label("mac"), Some("Studio"));
        assert_eq!(labels.machine_label("homelab"), None);
        let mut flood = labels.shared();
        flood.updated_ms += 10;
        flood.machines = (0..=MAX_SHARED)
            .map(|n| (n.to_string(), "x".into()))
            .collect();
        assert!(!labels.adopt(flood).unwrap());
    }

    #[test]
    fn labels_are_saved_privately_and_read_back() {
        let path = std::env::temp_dir().join(format!("magi-labels-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut labels = Labels::open(path.clone());
        labels
            .label_machine("Felipes-MacBook-Pro", " MacBook ")
            .unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reopened = Labels::open(path.clone());
        assert_eq!(
            reopened.machine_label("felipes-macbook-pro"),
            Some("MacBook")
        );
        labels.label_machine("felipes-macbook-pro", "").unwrap();
        assert_eq!(
            Labels::open(path.clone()).machine_label("felipes-macbook-pro"),
            None
        );
        fs::remove_file(path).unwrap();
    }
}
