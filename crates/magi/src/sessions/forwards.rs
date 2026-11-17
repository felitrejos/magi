use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::proto::{Forward, ForwardState};

const MAX_RECORDS: usize = 64;
const MAX_RECORD_BYTES: u64 = 16 * 1024;

#[derive(Deserialize)]
struct Record {
    session: String,
    host: String,
    #[serde(default)]
    ssh: String,
    ports: Vec<u16>,
    #[serde(default)]
    state: String,
}

pub struct Forwards {
    dir: PathBuf,
}

impl Forwards {
    pub fn for_this_user() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        Some(Self::new(Path::new(&home).join(".magi").join("forwards")))
    }

    fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn read(&self) -> Vec<Forward> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut forwards: Vec<Forward> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .filter(|path| {
                path.metadata()
                    .is_ok_and(|metadata| metadata.len() <= MAX_RECORD_BYTES)
            })
            .take(MAX_RECORDS)
            .filter_map(|path| {
                let record: Record = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
                let state = match (runner_alive(&path), record.state.as_str()) {
                    (false, _) => ForwardState::Stopped,
                    (true, "connected") => ForwardState::Connected,
                    (true, _) => ForwardState::Waiting,
                };
                Some(Forward {
                    session: record.session,
                    host: record.host,
                    ssh: record.ssh,
                    ports: record.ports,
                    state,
                })
            })
            .collect();
        forwards.sort_by(|a, b| a.session.cmp(&b.session));
        forwards
    }
}

fn runner_alive(record: &Path) -> bool {
    let Ok(lock) = File::open(record.with_extension("lock")) else {
        return false;
    };
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forward_counts_as_running_only_while_its_runner_holds_the_lock() {
        let dir = std::env::temp_dir().join(format!("magi-forwards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.json"),
            r#"{"session":"a","host":"homelab","ssh":"homelab","ports":[3000],"pid":1,"state":"connected"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            r#"{"session":"b","host":"nas","ssh":"nas.local","ports":[8080],"pid":1,"state":"waiting"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("c.json"),
            r#"{"session":"c","host":"nas","ports":[5173],"pid":1,"state":"connected"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let held = |name: &str| {
            let lock = File::create(dir.join(name)).unwrap();
            assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
            lock
        };
        let _a = held("a.lock");
        let _b = held("b.lock");
        File::create(dir.join("c.lock")).unwrap();
        let states: Vec<(String, ForwardState)> = Forwards::new(dir.clone())
            .read()
            .into_iter()
            .map(|forward| (forward.session, forward.state))
            .collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            states,
            [
                ("a".into(), ForwardState::Connected),
                ("b".into(), ForwardState::Waiting),
                ("c".into(), ForwardState::Stopped),
            ]
        );
    }
}
