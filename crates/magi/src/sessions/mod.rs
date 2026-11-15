mod claude;
mod codex;
mod forwards;
mod ledger;
mod pricing;
mod process;
mod registry;
pub mod rex;
mod tail;
mod tasks;
mod transcript;
mod usage;

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::proto::{Agent, AgentSession, BasisPoints, Forward, Plan, StatusReport, Window};
use claude::ClaudeSessions;
use codex::Codex;

const READ_BUDGET_PER_POLL: u64 = 32 << 20;
const CONTEXT_KEPT_FOR: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Default)]
pub struct Board {
    state: Mutex<BoardState>,
    saved: Option<PathBuf>,
}

#[derive(Default)]
struct BoardState {
    context: HashMap<String, (BasisPoints, Instant)>,
    windows: HashMap<u32, Window>,
}

impl Board {
    pub fn saved_at(path: PathBuf) -> Self {
        let windows: Vec<Window> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            state: Mutex::new(BoardState {
                context: HashMap::new(),
                windows: windows
                    .into_iter()
                    .map(|window| (window.minutes, window))
                    .collect(),
            }),
            saved: Some(path),
        }
    }

    fn save(&self, windows: &HashMap<u32, Window>) {
        let Some(path) = &self.saved else {
            return;
        };
        let mut windows: Vec<&Window> = windows.values().collect();
        windows.sort_by_key(|window| window.minutes);
        let Ok(json) = serde_json::to_vec(&windows) else {
            return;
        };
        let temporary = path.with_extension("json.tmp");
        let written = std::fs::write(&temporary, json)
            .and_then(|()| {
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            })
            .and_then(|()| std::fs::rename(&temporary, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    pub fn record(&self, report: StatusReport) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        state
            .context
            .retain(|_, (_, at)| now.duration_since(*at) < CONTEXT_KEPT_FOR);
        if let Some(used) = report.context_used {
            state.context.insert(report.session_id, (used, now));
        }
        if report.windows.is_empty() {
            return;
        }
        for window in report.windows {
            state.windows.insert(window.minutes, window);
        }
        self.save(&state.windows);
    }

    pub fn context(&self, session_id: &str) -> Option<BasisPoints> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .context
            .get(session_id)
            .map(|(used, _)| *used)
    }

    fn plan(&self, now_secs: u64) -> Option<Plan> {
        let mut windows: Vec<Window> = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .windows
            .values()
            .filter(|window| window.resets_at > now_secs)
            .copied()
            .collect();
        windows.sort_by_key(|window| window.minutes);
        (!windows.is_empty()).then_some(Plan {
            agent: Agent::ClaudeCode,
            windows,
        })
    }
}

#[derive(Default)]
pub struct Snapshot {
    pub sessions: Vec<AgentSession>,
    pub plans: Vec<Plan>,
    pub notices: Vec<String>,
    pub panes: HashMap<String, rex::Location>,
    pub transcripts: HashMap<String, (Agent, PathBuf)>,
    pub forwards: Vec<Forward>,
    pub cwds: HashMap<String, String>,
}

pub struct Found {
    pub session: AgentSession,
    pub pid: Option<i32>,
    pub transcript: Option<PathBuf>,
    pub cwd: String,
}

pub struct Sessions {
    claude: ClaudeSessions,
    codex: Option<Codex>,
    panes: rex::Panes,
    meter: usage::Meter,
    tasks: Option<tasks::Tasks>,
    forwards: Option<forwards::Forwards>,
}

impl Sessions {
    pub fn for_this_user() -> io::Result<Self> {
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
        Ok(Self {
            claude: ClaudeSessions::new(Path::new(&home).join(".claude")),
            codex: Codex::for_this_user(),
            panes: rex::Panes::default(),
            meter: usage::Meter::default(),
            tasks: tasks::Tasks::for_this_user(),
            forwards: forwards::Forwards::for_this_user(),
        })
    }

    pub fn poll(&mut self, board: &Board) -> Snapshot {
        let now = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64);
        let mut budget = READ_BUDGET_PER_POLL;
        let mut snapshot = Snapshot::default();
        let mut found = Vec::new();
        match self.claude.poll(board, now, now_ms, &mut budget) {
            Ok(polled) => {
                found = polled.sessions;
                snapshot.notices.extend(polled.notice);
            }
            Err(error) => snapshot
                .notices
                .push(format!("can't read Claude Code's sessions: {error}")),
        }
        snapshot.plans.extend(board.plan(now_ms / 1000));
        if let Some(forwards) = &self.forwards {
            snapshot.forwards = forwards.read();
        }
        if let Some(codex) = &mut self.codex {
            let (sessions, plan) = codex.poll(now, now_ms, &mut budget);
            found.extend(sessions);
            snapshot.plans.extend(plan);
        }
        let pids: Vec<i32> = found.iter().filter_map(|found| found.pid).collect();
        let usage = self.meter.measure(&pids, now);
        for Found {
            session,
            pid,
            transcript,
            cwd,
        } in found
        {
            if !cwd.is_empty() {
                snapshot.cwds.insert(session.id.clone(), cwd);
            }
            if let Some(path) = transcript {
                snapshot
                    .transcripts
                    .insert(session.id.clone(), (session.agent, path));
            }
            let pane = pid.and_then(|pid| self.panes.find(pid, now));
            let pane = pane.map(|(pane, location)| {
                snapshot.panes.insert(session.id.clone(), location);
                pane
            });
            let mut children = session.children;
            if let (Some(pid), Some(tasks)) = (pid, &mut self.tasks) {
                children.extend(tasks.started_by(pid, now));
            }
            snapshot.sessions.push(AgentSession {
                pane,
                usage: pid.and_then(|pid| usage.get(&pid).copied()),
                children,
                ..session
            });
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{FIVE_HOURS, ONE_WEEK};

    #[test]
    fn plan_windows_merge_and_expire_and_context_is_per_session() {
        let board = Board::default();
        let window = |minutes, used, resets_at| Window {
            minutes,
            used,
            resets_at,
        };
        board.record(StatusReport {
            session_id: "a".into(),
            context_used: Some(1_200),
            windows: vec![window(FIVE_HOURS, 3_000, 1_000)],
        });
        board.record(StatusReport {
            session_id: "b".into(),
            context_used: None,
            windows: vec![window(ONE_WEEK, 6_000, 9_000)],
        });
        let plan = |windows| {
            Some(Plan {
                agent: Agent::ClaudeCode,
                windows,
            })
        };
        assert_eq!(
            board.plan(500),
            plan(vec![
                window(FIVE_HOURS, 3_000, 1_000),
                window(ONE_WEEK, 6_000, 9_000)
            ])
        );
        assert_eq!(
            board.plan(2_000),
            plan(vec![window(ONE_WEEK, 6_000, 9_000)])
        );
        assert_eq!(board.plan(10_000), None);
        assert_eq!(
            (board.context("a"), board.context("b")),
            (Some(1_200), None)
        );
    }

    #[test]
    fn plan_windows_survive_a_restart_but_context_does_not() {
        let path = std::env::temp_dir().join(format!("magi-plans-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let board = Board::saved_at(path.clone());
        board.record(StatusReport {
            session_id: "a".into(),
            context_used: Some(1_200),
            windows: vec![Window {
                minutes: FIVE_HOURS,
                used: 3_700,
                resets_at: 9_000,
            }],
        });
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let restarted = Board::saved_at(path.clone());
        assert_eq!(
            restarted.plan(500).map(|plan| plan.windows),
            Some(vec![Window {
                minutes: FIVE_HOURS,
                used: 3_700,
                resets_at: 9_000,
            }])
        );
        assert_eq!(restarted.context("a"), None);
        assert_eq!(restarted.plan(10_000), None);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reads_this_machines_live_sessions_without_failing() {
        let snapshot = Sessions::for_this_user().unwrap().poll(&Board::default());
        for session in snapshot.sessions {
            assert!(!session.id.is_empty());
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_sessions"]
    fn bench_sessions() {
        let board = Board::default();
        let mut sessions = Sessions::for_this_user().unwrap();
        let started = Instant::now();
        let first = sessions.poll(&board);
        println!(
            "first read (catch up on {} sessions): {:?}",
            first.sessions.len(),
            started.elapsed()
        );
        for _ in 0..100 {
            sessions.poll(&board);
        }
        let snapshot = sessions.poll(&board);
        let reads = 200;
        let started = Instant::now();
        for _ in 0..reads {
            sessions.poll(&board);
        }
        println!("steady read: {:?}", started.elapsed() / reads);
        for session in snapshot.sessions {
            println!(
                "  {:<12} {:<18} {:<40} {:<16} out {:>9} cost ${:.2}{}",
                session.agent.name(),
                session.project,
                format!("{:?}", session.state),
                session.model,
                session.tokens.output,
                session.cost_micros as f64 / 1e6,
                if session.cost_complete {
                    ""
                } else {
                    " (partial)"
                }
            );
        }
        for notice in snapshot.notices {
            println!("  notice: {notice}");
        }
    }
}
