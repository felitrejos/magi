mod pricing;
mod registry;
mod transcript;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::proto::{
    AgentSession, BasisPoints, Plan, SessionState, StatusReport, Tokens, Waiting, Window,
};
use registry::{Registered, Registry, Status};
use transcript::Transcript;

const FIND_TRANSCRIPT_EVERY: Duration = Duration::from_secs(10);
const READ_BUDGET_PER_POLL: u64 = 32 << 20;
const CONTEXT_KEPT_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SESSIONS: usize = 64;
const MAX_TITLE_CHARS: usize = 120;

#[derive(Default)]
pub struct Board {
    state: Mutex<BoardState>,
}

#[derive(Default)]
struct BoardState {
    context: HashMap<String, (BasisPoints, Instant)>,
    plan: Plan,
}

impl Board {
    pub fn record(&self, report: StatusReport) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        state
            .context
            .retain(|_, (_, at)| now.duration_since(*at) < CONTEXT_KEPT_FOR);
        if let Some(used) = report.context_used {
            state.context.insert(report.session_id, (used, now));
        }
        if report.plan.five_hour.is_some() {
            state.plan.five_hour = report.plan.five_hour;
        }
        if report.plan.seven_day.is_some() {
            state.plan.seven_day = report.plan.seven_day;
        }
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
        let plan = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .plan
            .clone();
        let current = |window: Option<Window>| window.filter(|window| window.resets_at > now_secs);
        let plan = Plan {
            five_hour: current(plan.five_hour),
            seven_day: current(plan.seven_day),
        };
        (plan.five_hour.is_some() || plan.seven_day.is_some()).then_some(plan)
    }
}

pub struct Sessions {
    claude: PathBuf,
    registry: Registry,
    tracked: HashMap<String, Tracked>,
    claims: HashSet<String>,
}

struct Tracked {
    transcript: Option<Transcript>,
    looked_for_transcript: Option<Instant>,
    subagents: HashMap<PathBuf, Transcript>,
}

impl Tracked {
    fn new() -> Self {
        Self {
            transcript: None,
            looked_for_transcript: None,
            subagents: HashMap::new(),
        }
    }

    fn follow(
        &mut self,
        projects: &Path,
        session_id: &str,
        now: Instant,
        budget: &mut u64,
        claims: &mut HashSet<String>,
    ) {
        let due = self
            .looked_for_transcript
            .is_none_or(|at| now.duration_since(at) >= FIND_TRANSCRIPT_EVERY);
        if self.transcript.is_none() && due {
            self.looked_for_transcript = Some(now);
            self.transcript = registry::transcript(projects, session_id).map(Transcript::new);
        }
        let Some(transcript) = &mut self.transcript else {
            return;
        };
        let _ = transcript.advance(budget, claims);
        let subagents_dir = transcript.path().with_extension("").join("subagents");
        if let Ok(entries) = fs::read_dir(&subagents_dir) {
            for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            }) {
                self.subagents
                    .entry(path.clone())
                    .or_insert_with(|| Transcript::new(path));
            }
        }
        for subagent in self.subagents.values_mut() {
            let _ = subagent.advance(budget, claims);
        }
    }

    fn all_transcripts(&self) -> impl Iterator<Item = &Transcript> {
        self.transcript.iter().chain(self.subagents.values())
    }
}

impl Sessions {
    pub fn new(claude: PathBuf) -> Self {
        Self {
            claude,
            registry: Registry::default(),
            tracked: HashMap::new(),
            claims: HashSet::new(),
        }
    }

    pub fn for_this_user() -> io::Result<Self> {
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is not set"))?;
        Ok(Self::new(Path::new(&home).join(".claude")))
    }

    pub fn poll(&mut self, board: &Board) -> Option<(Vec<AgentSession>, Option<Plan>)> {
        let now = Instant::now();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64);
        let mut registered = self.registry.scan(&self.claude.join("sessions")).ok()?;
        registered.drain(..registered.len().saturating_sub(MAX_SESSIONS));
        let claims = &mut self.claims;
        self.tracked.retain(|id, tracked| {
            let live = registered.iter().any(|session| &session.session_id == id);
            if !live {
                tracked
                    .all_transcripts()
                    .for_each(|transcript| transcript.release(claims));
            }
            live
        });
        let projects = self.claude.join("projects");
        let mut budget = READ_BUDGET_PER_POLL;
        let sessions = registered
            .iter()
            .map(|session| {
                let tracked = self
                    .tracked
                    .entry(session.session_id.clone())
                    .or_insert_with(Tracked::new);
                tracked.follow(
                    &projects,
                    &session.session_id,
                    now,
                    &mut budget,
                    &mut self.claims,
                );
                tracked
                    .transcript
                    .iter_mut()
                    .chain(tracked.subagents.values_mut())
                    .for_each(|transcript| transcript.adopt(&mut self.claims));
                describe(session, tracked, board.context(&session.session_id), now_ms)
            })
            .collect();
        Some((sessions, board.plan(now_ms / 1000)))
    }
}

fn state(status: &Status, running_tool: Option<&str>) -> SessionState {
    match status {
        Status::Busy => SessionState::Working {
            tool: running_tool.map(str::to_string),
        },
        Status::Waiting(reason) if reason == "permission prompt" => {
            SessionState::Waiting(Waiting::Permission(running_tool.map(str::to_string)))
        }
        Status::Waiting(reason) => SessionState::Waiting(Waiting::Input(reason.clone())),
        Status::Idle => SessionState::Idle,
    }
}

fn describe(
    session: &Registered,
    tracked: &Tracked,
    context_used: Option<BasisPoints>,
    now_ms: u64,
) -> AgentSession {
    let project = Path::new(&session.cwd).file_name().map_or_else(
        || session.cwd.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    let main = tracked.transcript.as_ref();
    let title: String = session
        .name
        .clone()
        .unwrap_or_else(|| project.clone())
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    AgentSession {
        id: session.session_id.clone(),
        title,
        project,
        state: state(&session.status, main.and_then(Transcript::running_tool)),
        state_secs: (now_ms.saturating_sub(session.status_since_ms) / 1000) as u32,
        model: main
            .and_then(|transcript| transcript.model.clone())
            .unwrap_or_default(),
        context_tokens: main.map_or(0, |transcript| transcript.context_tokens),
        context_used,
        tokens: tracked
            .all_transcripts()
            .fold(Tokens::default(), |sum, transcript| sum + transcript.tokens),
        cost_micros: tracked
            .all_transcripts()
            .map(|transcript| transcript.cost_micros)
            .sum(),
        cost_complete: tracked
            .all_transcripts()
            .all(|transcript| !transcript.unpriced),
        age_secs: (now_ms.saturating_sub(session.started_ms) / 1000) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_status_decides_the_state_and_the_transcript_names_the_tool() {
        assert_eq!(
            state(&Status::Busy, Some("Edit")),
            SessionState::Working {
                tool: Some("Edit".into())
            }
        );
        assert_eq!(
            state(&Status::Waiting("permission prompt".into()), Some("Bash")),
            SessionState::Waiting(Waiting::Permission(Some("Bash".into())))
        );
        assert_eq!(
            state(&Status::Waiting("input needed".into()), None),
            SessionState::Waiting(Waiting::Input("input needed".into()))
        );
        assert_eq!(state(&Status::Idle, Some("Bash")), SessionState::Idle);
    }

    #[test]
    fn plan_windows_merge_and_expire_and_context_is_per_session() {
        let board = Board::default();
        let window = |used, resets_at| Some(Window { used, resets_at });
        board.record(StatusReport {
            session_id: "a".into(),
            context_used: Some(1_200),
            plan: Plan {
                five_hour: window(3_000, 1_000),
                seven_day: None,
            },
        });
        board.record(StatusReport {
            session_id: "b".into(),
            context_used: None,
            plan: Plan {
                five_hour: None,
                seven_day: window(6_000, 9_000),
            },
        });
        assert_eq!(
            board.plan(500),
            Some(Plan {
                five_hour: window(3_000, 1_000),
                seven_day: window(6_000, 9_000)
            })
        );
        assert_eq!(
            board.plan(2_000),
            Some(Plan {
                five_hour: None,
                seven_day: window(6_000, 9_000)
            })
        );
        assert_eq!(board.plan(10_000), None);
        assert_eq!(
            (board.context("a"), board.context("b")),
            (Some(1_200), None)
        );
    }

    #[test]
    fn reads_this_machines_live_sessions_without_failing() {
        let mut sessions = Sessions::for_this_user().unwrap();
        let (live, _) = sessions.poll(&Board::default()).unwrap();
        for session in live {
            assert!(!session.id.is_empty() && !session.project.is_empty());
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
        let (live, _) = sessions.poll(&board).unwrap();
        println!(
            "first read (catch up on {} sessions): {:?}",
            live.len(),
            started.elapsed()
        );
        for _ in 0..100 {
            sessions.poll(&board);
        }
        for _ in 0..20 {
            sessions.poll(&board);
        }
        let (live, _) = sessions.poll(&board).unwrap();
        let reads = 200;
        let started = Instant::now();
        for _ in 0..reads {
            sessions.poll(&board);
        }
        println!("steady read: {:?}", started.elapsed() / reads);
        for session in live {
            println!(
                "  {:<18} {:<40} {:<16} out {:>9} cost ${:.2}{}",
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
    }
}
