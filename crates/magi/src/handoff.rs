use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use serde::Deserialize;

use crate::proto::{Agent, AgentSession, Reply};

const INSTALL_DIRS: [&str; 4] = [
    ".local/bin",
    "go/bin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
];

static MOVING: AtomicBool = AtomicBool::new(false);

pub fn start(
    session: &AgentSession,
    to: &str,
    reply: impl Fn(Reply) + Send + 'static,
) -> Result<String, String> {
    check(session, to)?;
    let cielago = find("cielago", &search_dirs())
        .ok_or("cielago isn't installed on that machine: see github.com/felitrejos/cielago")?;
    if MOVING.swap(true, Ordering::SeqCst) {
        return Err("a move is already running on that machine".into());
    }
    let (id, title, to) = (session.id.clone(), session.title.clone(), to.to_string());
    let started = format!("asking cielago to move {title} to {to}…");
    thread::Builder::new()
        .name("move".into())
        .spawn(move || {
            run(&cielago, &id, &title, &to, &reply);
            MOVING.store(false, Ordering::SeqCst);
        })
        .map_err(|error| {
            MOVING.store(false, Ordering::SeqCst);
            format!("couldn't start the move: {error}")
        })?;
    Ok(started)
}

fn check(session: &AgentSession, to: &str) -> Result<(), String> {
    if session.agent != Agent::ClaudeCode {
        return Err(format!(
            "{} is a {} session; cielago moves Claude Code sessions",
            session.title,
            session.agent.name()
        ));
    }
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
}

fn run(cielago: &Path, id: &str, title: &str, to: &str, reply: &dyn Fn(Reply)) {
    let child = Command::new(cielago)
        .args(["move", "--json", id, to])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return reply(Reply {
                ok: false,
                text: format!("couldn't run cielago: {error}"),
            });
        }
    };
    let mut finished = false;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Event>(&line) else {
                continue;
            };
            finished |= matches!(event.event.as_str(), "moved" | "failed");
            reply(Reply {
                ok: event.ok,
                text: event.message,
            });
        }
    }
    let output = child.wait_with_output();
    if finished {
        return;
    }
    reply(match output {
        Ok(output) if output.status.success() => Reply {
            ok: true,
            text: format!("{title} moved to {to}"),
        },
        Ok(output) => Reply {
            ok: false,
            text: format!(
                "couldn't move {title}: {}",
                last_line(&output.stderr)
                    .unwrap_or_else(|| format!("cielago exited with {}", output.status))
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
        .map(|line| line.trim_start_matches("cielago: ").to_string())
}

fn search_dirs() -> Vec<PathBuf> {
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

fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    dirs.iter().map(|dir| dir.join(name)).find(|path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{SessionState, Tokens};

    fn session(agent: Agent) -> AgentSession {
        AgentSession {
            agent,
            id: "0b7a3c1e".into(),
            title: "fix login".into(),
            project: "web".into(),
            state: SessionState::Idle,
            state_secs: 0,
            model: String::new(),
            context_tokens: 0,
            context_used: None,
            tokens: Tokens::default(),
            cost_micros: 0,
            cost_complete: true,
            age_secs: 0,
            pane: None,
            usage: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn only_claude_code_sessions_move_and_only_to_plain_machine_names() {
        let claude = session(Agent::ClaudeCode);
        assert_eq!(check(&claude, "homelab"), Ok(()));
        assert_eq!(check(&claude, "felipes-macbook-pro.tail"), Ok(()));
        for bad in ["", "-oProxyCommand=x", "home lab", "a;b", "$(x)"] {
            assert!(check(&claude, bad).is_err(), "{bad}");
        }
        assert_eq!(
            check(&session(Agent::Codex), "homelab"),
            Err("fix login is a Codex session; cielago moves Claude Code sessions".into())
        );
    }

    #[test]
    fn cielago_is_found_in_the_first_folder_that_has_it_as_a_program() {
        let root = std::env::temp_dir().join(format!("magi-find-{}", std::process::id()));
        let (empty, plain, program) = (root.join("a"), root.join("b"), root.join("c"));
        for dir in [&empty, &plain, &program] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(plain.join("cielago"), "").unwrap();
        std::fs::write(program.join("cielago"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            program.join("cielago"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_eq!(
            find("cielago", &[empty.clone(), plain.clone(), program.clone()]),
            Some(program.join("cielago"))
        );
        assert_eq!(find("cielago", &[empty, plain]), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn fake_cielago(name: &str, script: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("magi-cielago-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("cielago");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, program)
    }

    fn replies(program: &Path) -> Vec<Reply> {
        let seen = std::sync::Mutex::new(Vec::new());
        run(program, "0b7a3c1e", "fix login", "homelab", &|reply| {
            seen.lock().unwrap().push(reply);
        });
        seen.into_inner().unwrap()
    }

    #[test]
    fn each_step_cielago_reports_reaches_the_viewer_as_it_happens() {
        let (dir, program) = fake_cielago(
            "steps",
            r#"[ "$1 $2 $3 $4" = "move --json 0b7a3c1e homelab" ] || { echo "cielago: wrong arguments: $*" >&2; exit 2; }
echo '{"event":"waiting","ok":true,"session":"0b7a3c1e","host":"homelab","message":"Session 0b7a3c1e is in the middle of a turn"}'
echo 'not json'
echo '{"event":"moved","ok":true,"session":"0b7a3c1e","host":"homelab","message":"Moved to homelab."}'"#,
        );
        let texts: Vec<(bool, String)> = replies(&program)
            .into_iter()
            .map(|reply| (reply.ok, reply.text))
            .collect();
        assert_eq!(
            texts,
            [
                (true, "Session 0b7a3c1e is in the middle of a turn".into()),
                (true, "Moved to homelab.".into())
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_cielago_that_fails_without_saying_why_on_stdout_is_explained_from_stderr() {
        let (dir, program) = fake_cielago(
            "fails",
            "echo 'cielago: homelab has work of its own' >&2; exit 1",
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
    fn the_outcome_is_the_last_thing_cielago_said() {
        assert_eq!(
            last_line(b"syncing\nmoved fix login to homelab\n\n"),
            Some("moved fix login to homelab".into())
        );
        assert_eq!(
            last_line(b"cielago: homelab has work of its own\n"),
            Some("homelab has work of its own".into())
        );
        assert_eq!(last_line(b"\n  \n"), None);
    }
}
