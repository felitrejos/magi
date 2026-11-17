use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const START_TOLERANCE_SECS: i64 = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Busy,
    Waiting(String),
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    pub pid: i32,
    pub session_id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub remote: Option<String>,
    pub status: Status,
    pub status_since_ms: u64,
    pub started_ms: u64,
    proc_start: Option<String>,
}

#[derive(Deserialize)]
struct RegistryJson {
    pid: i32,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "bridgeSessionId", default)]
    remote: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "waitingFor", default)]
    waiting_for: Option<String>,
    #[serde(rename = "statusUpdatedAt", default)]
    status_updated_at: u64,
    #[serde(rename = "startedAt", default)]
    started_at: u64,
    #[serde(rename = "procStart", default)]
    proc_start: Option<String>,
    #[serde(default)]
    spare: bool,
}

#[derive(Default)]
pub struct Registry {
    known: HashMap<PathBuf, Registered>,
}

pub struct Scan {
    pub sessions: Vec<Registered>,
    pub unknown: Vec<Option<String>>,
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Session(Registered),
    Spare,
    UnknownShape {
        pid: Option<i32>,
        session_id: Option<String>,
    },
    Unreadable,
}

impl Registry {
    pub fn scan(&mut self, dir: &Path) -> io::Result<Scan> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Scan {
                    sessions: Vec::new(),
                    unknown: Vec::new(),
                });
            }
            Err(error) => return Err(error),
        };
        let mut seen = HashMap::new();
        let mut unknown = Vec::new();
        for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        }) {
            let parsed = fs::read(&path).map_or(Parsed::Unreadable, |text| parse(&text));
            let entry = match parsed {
                Parsed::Session(session) => Some(session),
                Parsed::Spare => None,
                Parsed::UnknownShape { pid, session_id } => {
                    if pid.is_none_or(is_alive) {
                        unknown.push(session_id);
                    }
                    None
                }
                Parsed::Unreadable => self.known.get(&path).cloned(),
            };
            if let Some(session) = entry.filter(is_running) {
                seen.insert(path, session);
            }
        }
        self.known = seen;
        let mut sessions: Vec<Registered> = self.known.values().cloned().collect();
        sessions.sort_by_key(|session| session.started_ms);
        Ok(Scan { sessions, unknown })
    }
}

fn parse(text: &[u8]) -> Parsed {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(text) else {
        return Parsed::Unreadable;
    };
    let unknown = |value: &serde_json::Value| Parsed::UnknownShape {
        pid: value["pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok()),
        session_id: value["sessionId"].as_str().map(str::to_string),
    };
    let Ok(json) = serde_json::from_value::<RegistryJson>(value.clone()) else {
        return unknown(&value);
    };
    if json.spare {
        return Parsed::Spare;
    }
    let status = match json.status.as_deref().unwrap_or_default() {
        "busy" => Status::Busy,
        "waiting" => Status::Waiting(json.waiting_for.unwrap_or_default()),
        "idle" | "shell" => Status::Idle,
        _ => return unknown(&value),
    };
    Parsed::Session(Registered {
        pid: json.pid,
        session_id: json.session_id,
        cwd: json.cwd,
        name: json.name.filter(|name| !name.is_empty()),
        remote: json.remote.filter(|id| is_remote_id(id)),
        status,
        status_since_ms: json.status_updated_at,
        started_ms: json.started_at,
        proc_start: json.proc_start,
    })
}

fn is_remote_id(id: &str) -> bool {
    id.strip_prefix("session_").is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 64 && rest.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

pub fn transcript(projects: &Path, session_id: &str) -> Option<PathBuf> {
    let file = format!("{session_id}.jsonl");
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&file))
        .find(|path| path.is_file())
}

fn is_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

fn is_running(session: &Registered) -> bool {
    if !is_alive(session.pid) {
        return false;
    }
    match session.proc_start.as_deref().and_then(parse_utc_ctime) {
        Some(recorded) => process_start_secs(session.pid)
            .is_some_and(|actual| (actual - recorded).abs() <= START_TOLERANCE_SECS),
        None => true,
    }
}

fn parse_utc_ctime(text: &str) -> Option<i64> {
    let text = CString::new(text).ok()?;
    let mut tm = MaybeUninit::<libc::tm>::zeroed();
    let end = unsafe {
        libc::strptime(
            text.as_ptr(),
            c"%a %b %d %H:%M:%S %Y".as_ptr(),
            tm.as_mut_ptr(),
        )
    };
    if end.is_null() {
        return None;
    }
    let mut tm = unsafe { tm.assume_init() };
    Some(unsafe { libc::timegm(&mut tm) } as i64)
}

#[cfg(target_os = "macos")]
pub fn process_start_secs(pid: i32) -> Option<i64> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    Some(unsafe { info.assume_init() }.pbi_start_tvsec as i64)
}

#[cfg(target_os = "linux")]
pub fn process_start_secs(pid: i32) -> Option<i64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let ticks: i64 = stat
        .rsplit_once(')')?
        .1
        .split_ascii_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    let boot: i64 = fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    (per_second > 0).then(|| boot + ticks / per_second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(status: &str, extra: &str) -> Vec<u8> {
        format!(r#"{{"pid":18278,"sessionId":"4749","cwd":"/Users/me/magi","startedAt":1794619474650,"procStart":"Sat Nov 14 01:24:34 2026","kind":"interactive","name":"fix login","status":"{status}","statusUpdatedAt":1794619480000{extra}}}"#).into_bytes()
    }

    fn session(parsed: Parsed) -> Registered {
        match parsed {
            Parsed::Session(session) => session,
            other => panic!("not a session: {other:?}"),
        }
    }

    #[test]
    fn a_remote_control_session_keeps_its_id_and_a_strange_one_is_dropped() {
        let remote = session(parse(&entry(
            "idle",
            r#","bridgeSessionId":"session_01PZAiQ7TuGT2SFskGqJrbd4""#,
        )));
        assert_eq!(
            remote.remote.as_deref(),
            Some("session_01PZAiQ7TuGT2SFskGqJrbd4")
        );
        for odd in ["session_", "session_../x", "https://evil.example", ""] {
            let parsed = session(parse(&entry(
                "idle",
                &format!(r#","bridgeSessionId":"{odd}""#),
            )));
            assert_eq!(parsed.remote, None, "{odd}");
        }
        assert_eq!(session(parse(&entry("idle", ""))).remote, None);
    }

    #[test]
    fn parses_busy_waiting_and_idle() {
        let busy = session(parse(&entry("busy", "")));
        assert_eq!(
            (busy.status, busy.status_since_ms, busy.name.as_deref()),
            (Status::Busy, 1794619480000, Some("fix login"))
        );
        let waiting = session(parse(&entry(
            "waiting",
            r#","waitingFor":"permission prompt""#,
        )));
        assert_eq!(waiting.status, Status::Waiting("permission prompt".into()));
        assert_eq!(session(parse(&entry("idle", ""))).status, Status::Idle);
        assert_eq!(
            session(parse(&entry("shell", ""))).status,
            Status::Idle,
            "Claude Code 2.1.326 writes shell when its turn is over but a background shell runs"
        );
    }

    #[test]
    fn spare_sessions_are_skipped_and_a_new_shape_is_told_apart_from_a_half_written_file() {
        assert_eq!(parse(&entry("idle", r#","spare":true"#)), Parsed::Spare);
        assert_eq!(parse(b"{\"pid\":"), Parsed::Unreadable);
        assert_eq!(
            parse(&entry("thinking", "")),
            Parsed::UnknownShape {
                pid: Some(18278),
                session_id: Some("4749".into())
            }
        );
        assert_eq!(
            parse(br#"{"processId":1,"session":"a","state":"busy"}"#),
            Parsed::UnknownShape {
                pid: None,
                session_id: None
            }
        );
    }

    #[test]
    fn proc_start_is_utc_ctime() {
        assert_eq!(
            parse_utc_ctime("Sat Nov 14 01:24:34 2026"),
            Some(1_794_619_474)
        );
        assert_eq!(parse_utc_ctime("garbage"), None);
    }

    #[test]
    fn this_process_matches_its_own_start_time_and_a_reused_pid_does_not() {
        let me = std::process::id() as i32;
        let started = process_start_secs(me).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(started <= now && now - started < 3_600);
        let utc = |secs: i64| {
            let mut tm = MaybeUninit::<libc::tm>::zeroed();
            unsafe { libc::gmtime_r(&secs, tm.as_mut_ptr()) };
            let tm = unsafe { tm.assume_init() };
            let mut buf = [0u8; 64];
            let len = unsafe {
                libc::strftime(
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    c"%a %b %d %H:%M:%S %Y".as_ptr(),
                    &tm,
                )
            };
            String::from_utf8_lossy(&buf[..len]).into_owned()
        };
        let session = |proc_start: String| Registered {
            pid: me,
            session_id: "s".into(),
            cwd: String::new(),
            name: None,
            remote: None,
            status: Status::Idle,
            status_since_ms: 0,
            started_ms: 0,
            proc_start: Some(proc_start),
        };
        assert!(is_running(&session(utc(started))));
        assert!(!is_running(&session(utc(started - 3_600))));
        assert!(!is_running(&Registered {
            pid: i32::MAX,
            ..session(utc(started))
        }));
    }

    #[test]
    fn a_half_written_file_keeps_the_last_good_reading() {
        let dir = std::env::temp_dir().join(format!("magi-registry-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let me = std::process::id();
        let file = dir.join(format!("{me}.json"));
        fs::write(&file, format!(r#"{{"pid":{me},"sessionId":"live","status":"busy","statusUpdatedAt":1,"startedAt":2}}"#)).unwrap();
        let mut registry = Registry::default();
        assert_eq!(registry.scan(&dir).unwrap().sessions.len(), 1);
        fs::write(&file, r#"{"pid":"#).unwrap();
        let half_written = registry.scan(&dir).unwrap();
        assert_eq!(half_written.sessions[0].session_id, "live");
        assert!(half_written.unknown.is_empty());
        fs::write(
            &file,
            format!(r#"{{"pid":{me},"sessionId":"live","status":"compacting"}}"#),
        )
        .unwrap();
        let changed = registry.scan(&dir).unwrap();
        assert!(changed.sessions.is_empty());
        assert_eq!(changed.unknown, vec![Some("live".to_string())]);
        fs::write(
            &file,
            r#"{"pid":2147483000,"sessionId":"gone","status":"compacting"}"#,
        )
        .unwrap();
        assert!(registry.scan(&dir).unwrap().unknown.is_empty());
        fs::remove_file(&file).unwrap();
        assert!(registry.scan(&dir).unwrap().sessions.is_empty());
        fs::remove_dir_all(&dir).unwrap();
        assert!(
            Registry::default()
                .scan(Path::new("/nonexistent/magi"))
                .unwrap()
                .sessions
                .is_empty()
        );
    }
}
