use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::bounded;
use crate::proto::{AgentSession, Reply};

const INSTALL_DIRS: [&str; 3] = [".local/bin", "/opt/homebrew/bin", "/usr/local/bin"];

const MOVE_LIMIT: Duration = Duration::from_secs(15 * 60);
const WATCH_EVERY: Duration = Duration::from_millis(200);
const FINAL_EVENTS: [&str; 2] = ["moved", "failed"];

static MOVING: AtomicBool = AtomicBool::new(false);

struct Moving;

impl Drop for Moving {
    fn drop(&mut self) {
        MOVING.store(false, Ordering::SeqCst);
    }
}

pub fn start(
    session: &AgentSession,
    to: &str,
    reply: impl Fn(Reply) + Send + 'static,
) -> Result<String, String> {
    check(to)?;
    let remote = remote_program()?;
    if MOVING.swap(true, Ordering::SeqCst) {
        return Err("a move is already running on that machine".into());
    }
    let (id, title, to) = (session.id.clone(), session.title.clone(), to.to_string());
    let started = format!("Moving {title} to {to}…");
    let guard = Moving;
    thread::Builder::new()
        .name("move".into())
        .spawn(move || {
            let _guard = guard;
            run(&remote, &id, &title, &to, MOVE_LIMIT, &reply);
        })
        .map_err(|error| format!("couldn't start the move: {error}"))?;
    Ok(started)
}

fn check(to: &str) -> Result<(), String> {
    let valid = !to.is_empty()
        && to.len() <= 253
        && !to.starts_with('-')
        && to
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if !valid {
        return Err(format!("{to:?} isn't a machine name"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Event {
    event: String,
    ok: bool,
    message: String,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    seconds: Option<String>,
    #[serde(default)]
    ports: Vec<u16>,
}

fn said(event: &Event, title: &str, to: &str) -> String {
    match event.event.as_str() {
        "moving" => format!("Moving {title} to {to}…"),
        "step" => {
            let doing = match event.step.as_deref().unwrap_or_default() {
                "inspect" => "checking both machines",
                "sync" => "copying your work",
                "transcript" => "copying the conversation",
                "launch" | "start" => "starting Claude there",
                "panes" => "setting up the project",
                "tab" => "opening its tab",
                "ports" => "forwarding its ports",
                other => other,
            };
            format!("Moving {title} to {to} · {doing}…")
        }
        "waiting" => format!("{title} moves to {to} when its turn ends…"),
        "moved" => {
            let took = event
                .seconds
                .as_ref()
                .map(|seconds| format!(" in {seconds}s"))
                .unwrap_or_default();
            let ports: Vec<String> = event
                .ports
                .iter()
                .map(|port| format!("localhost:{port}"))
                .collect();
            let reach = if ports.is_empty() {
                String::new()
            } else {
                format!(" · {} reaches it there", ports.join(", "))
            };
            format!("Moved {title} to {to}{took}{reach}")
        }
        _ => format!("couldn't move {title}: {}", event.message),
    }
}

fn run(remote: &Path, id: &str, title: &str, to: &str, limit: Duration, reply: &dyn Fn(Reply)) {
    let deadline = Instant::now() + limit;
    let mut child =
        match bounded::spawn(Command::new(remote).args(["move", "--json", "--stay", id, to])) {
            Ok(child) => child,
            Err(error) => {
                return reply(Reply {
                    ok: false,
                    text: format!("couldn't run {REMOTE}: {error}"),
                });
            }
        };
    let stderr = bounded::drain(child.stderr.take());
    let finished = Arc::new(AtomicBool::new(false));
    let stopped = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let finished = Arc::clone(&finished);
        let stopped = Arc::clone(&stopped);
        let pid = child.id();
        thread::spawn(move || {
            while Instant::now() < deadline && !finished.load(Ordering::SeqCst) {
                thread::sleep(WATCH_EVERY);
            }
            if !finished.load(Ordering::SeqCst)
                && let Ok(pid) = libc::pid_t::try_from(pid)
            {
                stopped.store(true, Ordering::SeqCst);
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        })
    };
    let mut ended = false;
    let mut waiting = false;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Event>(&line) else {
                continue;
            };
            ended = FINAL_EVENTS.contains(&event.event.as_str());
            waiting = event.event == "waiting";
            reply(Reply {
                ok: event.ok,
                text: said(&event, title, to),
            });
            if ended {
                break;
            }
        }
    }
    let status = bounded::wait(&mut child, deadline);
    finished.store(true, Ordering::SeqCst);
    let _ = watchdog.join();
    let stderr = stderr.join().unwrap_or_default();
    if ended || (waiting && status.as_ref().is_ok_and(|status| status.success())) {
        return;
    }
    if stopped.load(Ordering::SeqCst) {
        return reply(Reply {
            ok: false,
            text: format!("couldn't move {title}: it took too long and was stopped"),
        });
    }
    reply(match status {
        Ok(status) if status.success() => Reply {
            ok: true,
            text: format!("{title} moved to {to}"),
        },
        Ok(status) => Reply {
            ok: false,
            text: format!(
                "couldn't move {title}: {}",
                last_line(&stderr).unwrap_or_else(|| format!("{REMOTE} exited with {status}"))
            ),
        },
        Err(error) => Reply {
            ok: false,
            text: format!("couldn't move {title}: {error}"),
        },
    });
}

fn last_line(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches("magi: ").to_string())
}

pub fn search_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(INSTALL_DIRS.iter().filter_map(|dir| {
            if dir.starts_with('/') {
                Some(PathBuf::from(dir))
            } else {
                home.as_ref().map(|home| home.join(dir))
            }
        }))
        .collect()
}

pub const REMOTE: &str = "magi-remote";

pub fn remote_program() -> Result<PathBuf, String> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let dirs: Vec<PathBuf> = beside.into_iter().chain(search_dirs()).collect();
    find(REMOTE, &dirs).ok_or_else(|| {
        format!("{REMOTE} is missing on this machine, so magi can't move work; reinstall magi from github.com/felitrejos/magi")
    })
}

pub fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    dirs.iter().map(|dir| dir.join(name)).find(|path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_moves_only_to_a_plain_machine_name() {
        assert_eq!(check("homelab"), Ok(()));
        assert_eq!(check("felipes-macbook-pro.tail"), Ok(()));
        for bad in ["", "-oProxyCommand=x", "home lab", "a;b", "$(x)"] {
            assert!(check(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn remote_is_found_in_the_first_folder_that_has_it_as_a_program() {
        let root = std::env::temp_dir().join(format!("magi-find-{}", std::process::id()));
        let (empty, plain, program) = (root.join("a"), root.join("b"), root.join("c"));
        for dir in [&empty, &plain, &program] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(plain.join("remote"), "").unwrap();
        std::fs::write(program.join("remote"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            program.join("remote"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_eq!(
            find("remote", &[empty.clone(), plain.clone(), program.clone()]),
            Some(program.join("remote"))
        );
        assert_eq!(find("remote", &[empty, plain]), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn fake_remote(name: &str, script: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("magi-remote-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("remote");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, program)
    }

    fn replies(program: &Path) -> Vec<Reply> {
        let seen = std::sync::Mutex::new(Vec::new());
        run(
            program,
            "0b7a3c1e",
            "fix login",
            "homelab",
            Duration::from_secs(10),
            &|reply| {
                seen.lock().unwrap().push(reply);
            },
        );
        seen.into_inner().unwrap()
    }

    #[test]
    fn each_step_remote_reports_reaches_the_viewer_as_it_happens() {
        let (dir, program) = fake_remote(
            "steps",
            r#"[ "$1 $2 $3 $4 $5" = "move --json --stay 0b7a3c1e homelab" ] || { echo "magi: wrong arguments: $*" >&2; exit 2; }
echo '{"event":"waiting","ok":true,"session":"0b7a3c1e","host":"homelab","message":"Session 0b7a3c1e is in the middle of a turn"}'
echo 'not json'
echo '{"event":"step","ok":true,"step":"sync","message":"Moving session 0b7a3c1e to homelab: sync"}'
echo '{"event":"step","ok":true,"step":"panes","message":"Moving session 0b7a3c1e to homelab: panes"}'
echo '{"event":"step","ok":true,"step":"ports","message":"Moving session 0b7a3c1e to homelab: ports"}'
echo '{"event":"moved","ok":true,"session":"0b7a3c1e","host":"homelab","message":"Moved session 0b7a3c1e to homelab in 4.3s (inspect 1.1s, sync 0.2s).","seconds":"4.3","ports":[3000]}'"#,
        );
        let texts: Vec<(bool, String)> = replies(&program)
            .into_iter()
            .map(|reply| (reply.ok, reply.text))
            .collect();
        assert_eq!(
            texts,
            [
                (
                    true,
                    "fix login moves to homelab when its turn ends…".into()
                ),
                (
                    true,
                    "Moving fix login to homelab · copying your work…".into()
                ),
                (
                    true,
                    "Moving fix login to homelab · setting up the project…".into()
                ),
                (
                    true,
                    "Moving fix login to homelab · forwarding its ports…".into()
                ),
                (
                    true,
                    "Moved fix login to homelab in 4.3s · localhost:3000 reaches it there".into()
                )
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_remote_that_fails_without_saying_why_on_stdout_is_explained_from_stderr() {
        let (dir, program) = fake_remote(
            "fails",
            "echo 'magi: homelab has work of its own' >&2; exit 1",
        );
        assert_eq!(
            replies(&program),
            [Reply {
                ok: false,
                text: "couldn't move fix login: homelab has work of its own".into()
            }]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_outcome_is_the_last_thing_remote_said() {
        assert_eq!(
            last_line(b"syncing\nmoved fix login to homelab\n\n"),
            Some("moved fix login to homelab".into())
        );
        assert_eq!(
            last_line(b"magi: homelab has work of its own\n"),
            Some("homelab has work of its own".into())
        );
        assert_eq!(last_line(b"\n  \n"), None);
    }

    #[test]
    fn a_remote_that_never_finishes_is_stopped_at_the_deadline_and_says_so() {
        let (dir, program) = fake_remote(
            "hangs",
            r#"echo '{"event":"moving","ok":true,"message":"Moving"}'; sleep 30"#,
        );
        let started = Instant::now();
        let seen = std::sync::Mutex::new(Vec::new());
        run(
            &program,
            "id",
            "fix login",
            "homelab",
            Duration::from_millis(400),
            &|reply| {
                seen.lock().unwrap().push(reply);
            },
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen[0].text, "Moving fix login to homelab…");
        assert!(!seen[1].ok);
        assert!(
            seen[1]
                .text
                .starts_with("couldn't move fix login: it took too long"),
            "{}",
            seen[1].text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_move_flag_clears_even_when_the_move_thread_panics() {
        MOVING.store(true, Ordering::SeqCst);
        let guard = Moving;
        let _ = thread::spawn(move || {
            let _guard = guard;
            panic!("boom");
        })
        .join();
        assert!(!MOVING.load(Ordering::SeqCst));
    }
}
