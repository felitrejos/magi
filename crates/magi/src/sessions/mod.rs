mod claude;
mod claude_limits;
mod codex;
pub mod codex_app;
mod forwards;
mod ledger;
mod limits;
mod pricing;
mod process;
mod registry;
mod repo;
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

use crate::proto::{
    Agent, AgentSession, BasisPoints, Forward, Plan, StatusReport, TopProcess, Window,
};
use claude::ClaudeSessions;
use codex::Codex;
use limits::{Limits, Reading};

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
    as_of: u64,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SavedPlan {
    as_of: u64,
    windows: Vec<Window>,
}

impl Board {
    pub fn saved_at(path: PathBuf) -> Self {
        let saved: SavedPlan = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            state: Mutex::new(BoardState {
                context: HashMap::new(),
                windows: saved
                    .windows
                    .into_iter()
                    .map(|window| (window.minutes, window))
                    .collect(),
                as_of: saved.as_of,
            }),
            saved: Some(path),
        }
    }

    fn save(&self, state: &BoardState) {
        let Some(path) = &self.saved else {
            return;
        };
        let mut windows: Vec<Window> = state.windows.values().copied().collect();
        windows.sort_by_key(|window| window.minutes);
        let saved = SavedPlan {
            as_of: state.as_of,
            windows,
        };
        let Ok(json) = serde_json::to_vec(&saved) else {
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
        state.as_of = unix_secs();
        self.save(&state);
    }

    pub fn context(&self, session_id: &str) -> Option<BasisPoints> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .context
            .get(session_id)
            .map(|(used, _)| *used)
    }

    fn reading(&self) -> Option<Reading> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (!state.windows.is_empty())
            .then(|| (state.as_of, state.windows.values().copied().collect()))
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
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
    pub processes: Vec<TopProcess>,
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
    claude_limits: Limits,
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
            claude_limits: Limits::new(claude_limits::ask),
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
        snapshot.plans.extend(limits::plan(
            Agent::ClaudeCode,
            board.reading(),
            Some(&mut self.claude_limits),
            now,
            now_ms / 1000,
        ));
        if let Some(forwards) = &self.forwards {
            snapshot.forwards = forwards.read();
        }
        if let Some(codex) = &mut self.codex {
            let (sessions, plan) = codex.poll(now, now_ms, &mut budget);
            found.extend(sessions);
            snapshot.plans.extend(plan);
        }
        let pids: Vec<i32> = found.iter().filter_map(|found| found.pid).collect();
        let measured = self.meter.measure(&pids, now);
        let ids: HashMap<i32, String> = found
            .iter()
            .filter_map(|found| Some((found.pid?, found.session.id.clone())))
            .collect();
        snapshot.processes = measured
            .top
            .iter()
            .map(|busy| TopProcess {
                pid: busy.pid,
                name: busy.name.clone(),
                cpu: busy.cpu,
                memory: busy.memory,
                session: busy.root.and_then(|root| ids.get(&root)).cloned(),
            })
            .collect();
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
            let parent = pid
                .and_then(|pid| measured.parents.get(&pid))
                .and_then(|parent| ids.get(parent))
                .cloned();
            snapshot.sessions.push(AgentSession {
                pane,
                usage: pid.and_then(|pid| measured.usage.get(&pid).copied()),
                children,
                parent,
                ..session
            });
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_plan(board: &Board, now_secs: u64) -> Option<Plan> {
        limits::plan(
            Agent::ClaudeCode,
            board.reading(),
            None,
            Instant::now(),
            now_secs,
        )
    }
    use crate::proto::{FIVE_HOURS, ONE_WEEK};

    #[test]
    fn plan_windows_merge_and_start_over_at_their_reset_and_context_is_per_session() {
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
        let plan = |windows| Some(windows);
        assert_eq!(
            claude_plan(&board, 500).map(|plan| plan.windows),
            plan(vec![
                window(FIVE_HOURS, 3_000, 1_000),
                window(ONE_WEEK, 6_000, 9_000)
            ])
        );
        assert_eq!(
            claude_plan(&board, 2_000).map(|plan| plan.windows),
            plan(vec![
                window(FIVE_HOURS, 0, 19_000),
                window(ONE_WEEK, 6_000, 9_000)
            ])
        );
        assert_eq!(
            claude_plan(&board, 10_000).map(|plan| plan
                .windows
                .iter()
                .map(|w| w.used)
                .sum::<u16>()),
            Some(0)
        );
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
            claude_plan(&restarted, 500).map(|plan| plan.windows),
            Some(vec![Window {
                minutes: FIVE_HOURS,
                used: 3_700,
                resets_at: 9_000,
            }])
        );
        assert_eq!(restarted.context("a"), None);
        assert_eq!(
            claude_plan(&restarted, 10_000).map(|plan| plan.windows[0].used),
            Some(0)
        );
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
