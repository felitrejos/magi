use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::proto::RexPane;

const REFRESH_EVERY: Duration = Duration::from_secs(10);
const RETRY_MISSING_AFTER: Duration = Duration::from_secs(5);
const REREAD_PROCESS_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
struct Location {
    session: String,
    block: String,
    server: Option<OsString>,
    bin_dir: PathBuf,
}

#[derive(Default)]
pub struct Panes {
    servers: HashMap<(Option<OsString>, String), Server>,
    processes: HashMap<i32, (Instant, Option<Location>)>,
}

struct Server {
    listed_at: Instant,
    windows: HashMap<String, String>,
}

#[derive(Deserialize)]
struct Listing {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    block_id: String,
    window_id: Option<String>,
}

impl Panes {
    pub fn find(&mut self, pid: i32, now: Instant) -> Option<RexPane> {
        let here = self.locate(pid, now)?;
        let key = (here.server.clone(), here.session.clone());
        let due = match self.servers.get(&key) {
            Some(server) if server.windows.contains_key(&here.block) => {
                now.duration_since(server.listed_at) >= REFRESH_EVERY
            }
            Some(server) => now.duration_since(server.listed_at) >= RETRY_MISSING_AFTER,
            None => true,
        };
        if due {
            let windows = list_windows(&here).unwrap_or_default();
            self.servers.insert(
                key.clone(),
                Server {
                    listed_at: now,
                    windows,
                },
            );
        }
        let window = self.servers.get(&key)?.windows.get(&here.block)?;
        Some(RexPane {
            session: here.session,
            window: window.clone(),
        })
    }

    fn locate(&mut self, pid: i32, now: Instant) -> Option<Location> {
        if let Some((at, found)) = self.processes.get(&pid)
            && now.duration_since(*at) < REREAD_PROCESS_AFTER
        {
            return found.clone();
        }
        self.processes
            .retain(|_, (at, _)| now.duration_since(*at) < REREAD_PROCESS_AFTER);
        let found = environment(pid).and_then(|environment| location(&environment));
        self.processes.insert(pid, (now, found.clone()));
        found
    }
}

fn location(environment: &[u8]) -> Option<Location> {
    let mut session = None;
    let mut block = None;
    let mut server = None;
    let mut bin_dir = None;
    for entry in environment.split(|byte| *byte == 0) {
        let Some(split) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (name, value) = (&entry[..split], &entry[split + 1..]);
        match name {
            b"REX_SESSION" => session = String::from_utf8(value.to_vec()).ok(),
            b"REX_BLOCK" => block = String::from_utf8(value.to_vec()).ok(),
            b"REX_SERVER" => server = Some(OsString::from_vec(value.to_vec())),
            b"REX_BIN_DIR" => bin_dir = Some(PathBuf::from(OsString::from_vec(value.to_vec()))),
            _ => {}
        }
    }
    Some(Location {
        session: session.filter(|id| !id.is_empty())?,
        block: block.filter(|id| !id.is_empty())?,
        server,
        bin_dir: bin_dir?,
    })
}

fn list_windows(here: &Location) -> Option<HashMap<String, String>> {
    let mut command = Command::new(here.bin_dir.join("rex"));
    command
        .args([
            "--autostart=false",
            "--timeout",
            "1s",
            "block",
            "ls",
            "--json",
            "-s",
            &here.session,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    match &here.server {
        Some(server) => command.env("REX_SERVER", server),
        None => command.env_remove("REX_SERVER"),
    };
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let listing: Listing = serde_json::from_slice(&output.stdout).ok()?;
    Some(
        listing
            .blocks
            .into_iter()
            .filter_map(|block| Some((block.block_id, block.window_id?)))
            .collect(),
    )
}

#[cfg(target_os = "linux")]
fn environment(pid: i32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/environ")).ok()
}

#[cfg(target_os = "macos")]
fn environment(pid: i32) -> Option<Vec<u8>> {
    let mut size = argument_limit()?;
    let mut buffer = vec![0u8; size];
    let mut name = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let found = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if found != 0 {
        return None;
    }
    buffer.truncate(size);
    environment_of(&buffer).map(<[u8]>::to_vec)
}

#[cfg(target_os = "macos")]
fn argument_limit() -> Option<usize> {
    let mut limit: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    let mut name = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let found = unsafe {
        libc::sysctl(
            name.as_mut_ptr(),
            name.len() as libc::c_uint,
            (&raw mut limit).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (found == 0).then(|| usize::try_from(limit).ok()).flatten()
}

#[cfg(any(target_os = "macos", test))]
fn environment_of(arguments: &[u8]) -> Option<&[u8]> {
    let (count, rest) = arguments.split_first_chunk::<4>()?;
    let count = usize::try_from(i32::from_ne_bytes(*count)).ok()?;
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let rest = &rest[path_end..];
    let mut rest = &rest[rest.iter().position(|byte| *byte != 0)?..];
    for _ in 0..count {
        let end = rest.iter().position(|byte| *byte == 0)?;
        rest = &rest[end + 1..];
    }
    if rest.first() == Some(&0) {
        return Some(&[]);
    }
    let end = rest
        .windows(2)
        .position(|pair| pair == [0, 0])
        .map_or(rest.len(), |end| end + 1);
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rex_pane_needs_its_session_block_and_rex_location() {
        let environment = b"PATH=/bin\0REX_SESSION=session:1\0API_KEY=hidden\0REX_BLOCK=block:2\0REX_BIN_DIR=/opt/rex\0";
        assert_eq!(
            location(environment),
            Some(Location {
                session: "session:1".into(),
                block: "block:2".into(),
                server: None,
                bin_dir: "/opt/rex".into(),
            })
        );
        let with_server = [&environment[..], b"REX_SERVER=unix:///tmp/rex.sock\0"].concat();
        assert_eq!(
            location(&with_server).and_then(|here| here.server),
            Some("unix:///tmp/rex.sock".into())
        );
        assert_eq!(
            location(b"REX_SESSION=session:1\0REX_BLOCK=block:2\0"),
            None
        );
        assert_eq!(location(b"PATH=/bin\0"), None);
    }

    #[test]
    fn the_environment_follows_the_path_and_arguments_in_process_arguments() {
        let mut arguments = 2i32.to_ne_bytes().to_vec();
        arguments.extend_from_slice(b"/usr/bin/claude\0\0\0\0claude\0REX_BLOCK=not-env\0");
        arguments.extend_from_slice(b"REX_BLOCK=block:9\0HOME=/home/me\0\0executable_path=x\0");
        assert_eq!(
            environment_of(&arguments),
            Some(&b"REX_BLOCK=block:9\0HOME=/home/me\0"[..])
        );
        assert_eq!(environment_of(&[1, 0]), None);
    }

    #[test]
    fn this_process_reads_its_own_environment() {
        let environment = environment(std::process::id() as i32).unwrap();
        let path = std::env::var("PATH").unwrap();
        assert!(
            environment
                .split(|byte| *byte == 0)
                .any(|entry| entry == format!("PATH={path}").as_bytes())
        );
    }
}
