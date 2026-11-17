use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::registry::{self, Registry, Status};
use super::repo;
use super::tail;
use super::transcript::Transcript;
use super::{Board, Found};
use crate::proto::{Agent, AgentSession, BasisPoints, Child, SessionState, Waiting};

const FIND_TRANSCRIPT_EVERY: Duration = Duration::from_secs(10);
const ACTIVE_WITHIN: Duration = Duration::from_secs(30 * 60);
const WORKING_IF_WRITTEN_WITHIN: Duration = Duration::from_secs(30);
const MAX_SESSIONS: usize = 64;
const MAX_TITLE_CHARS: usize = 120;
pub const FORMAT_CHANGED: &str =
    "Claude Code's session format changed; states are a guess until magi is updated";

pub struct ClaudeSessions {
    claude: PathBuf,
    registry: Registry,
    tracked: HashMap<String, Tracked>,
    claims: HashSet<String>,
    recent: Vec<(String, PathBuf)>,
    recent_at: Option<Instant>,
}

pub struct Polled {
    pub sessions: Vec<Found>,
    pub notices: Vec<String>,
}

struct Tracked {
    transcript: Option<Transcript>,
    looked_for_transcript: Option<Instant>,
    subagents: HashMap<PathBuf, Transcript>,
}

struct Seen {
    id: String,
    pid: Option<i32>,
    cwd: Option<String>,
    name: Option<String>,
    remote: Option<String>,
    state: SessionState,
    state_secs: u32,
}

impl Tracked {
    fn new(transcript: Option<Transcript>) -> Self {
        Self {
            transcript,
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
    ) -> Vec<String> {
        let due = self
            .looked_for_transcript
            .is_none_or(|at| now.duration_since(at) >= FIND_TRANSCRIPT_EVERY);
        if self.transcript.is_none() && due {
            self.looked_for_transcript = Some(now);
            self.transcript = registry::transcript(projects, session_id).map(Transcript::new);
        }
        let Some(transcript) = &mut self.transcript else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        let read = transcript.advance(budget, claims);
        problems.extend(tail::unreadable(transcript.path(), read));
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
            let read = subagent.advance(budget, claims);
            problems.extend(tail::unreadable(subagent.path(), read));
        }
        self.transcripts_mut()
            .for_each(|transcript| transcript.adopt(claims));
        problems
    }

    fn transcripts(&self) -> impl Iterator<Item = &Transcript> {
        self.transcript.iter().chain(self.subagents.values())
    }

    fn transcripts_mut(&mut self) -> impl Iterator<Item = &mut Transcript> {
        self.transcript
            .iter_mut()
            .chain(self.subagents.values_mut())
    }
}

impl ClaudeSessions {
    pub fn new(claude: PathBuf) -> Self {
        Self {
            claude,
            registry: Registry::default(),
            tracked: HashMap::new(),
            claims: HashSet::new(),
            recent: Vec::new(),
            recent_at: None,
        }
    }

    pub fn poll(
        &mut self,
        board: &Board,
        now: Instant,
        now_ms: u64,
        budget: &mut u64,
    ) -> io::Result<Polled> {
        let scan = self.registry.scan(&self.claude.join("sessions"))?;
        let mut registered = scan.sessions;
        registered.drain(..registered.len().saturating_sub(MAX_SESSIONS));
        let mut seen: Vec<Seen> = registered
            .into_iter()
            .map(|session| Seen {
                state: state(&session.status),
                state_secs: secs_between(session.status_since_ms, now_ms),
                cwd: Some(session.cwd),
                name: session.name,
                remote: session.remote,
                id: session.session_id,
                pid: Some(session.pid),
            })
            .collect();
        let mut notices = Vec::new();
        if !scan.unknown.is_empty() {
            notices.push(FORMAT_CHANGED.to_string());
            let mut ids: Vec<String> = scan.unknown.iter().flatten().cloned().collect();
            if scan.unknown.iter().any(Option::is_none) {
                self.refresh_recent(now);
                ids.extend(self.recent.iter().map(|(id, _)| id.clone()));
            }
            ids.sort();
            ids.dedup();
            ids.retain(|id| !seen.iter().any(|session| &session.id == id));
            ids.truncate(MAX_SESSIONS.saturating_sub(seen.len()));
            let guessed: Vec<Seen> = ids.into_iter().map(|id| self.guess(id, now_ms)).collect();
            seen.extend(guessed);
        }
        let claims = &mut self.claims;
        self.tracked.retain(|id, tracked| {
            let live = seen.iter().any(|session| &session.id == id);
            if !live {
                tracked
                    .transcripts()
                    .for_each(|transcript| transcript.release(claims));
            }
            live
        });
        let projects = self.claude.join("projects");
        let sessions = seen
            .into_iter()
            .map(|session| {
                let tracked = self.tracked.entry(session.id.clone()).or_insert_with(|| {
                    Tracked::new(
                        self.recent
                            .iter()
                            .find(|(id, _)| *id == session.id)
                            .map(|(_, path)| Transcript::new(path.clone())),
                    )
                });
                notices.extend(tracked.follow(
                    &projects,
                    &session.id,
                    now,
                    budget,
                    &mut self.claims,
                ));
                let context = board.context(&session.id);
                let pid = session.pid;
                let transcript = tracked
                    .transcript
                    .as_ref()
                    .map(|transcript| transcript.path().to_path_buf());
                let cwd = session_cwd(&session, tracked);
                Found {
                    session: describe(session, tracked, context, &cwd),
                    pid,
                    transcript,
                    cwd,
                }
            })
            .collect();
        Ok(Polled { sessions, notices })
    }

    fn refresh_recent(&mut self, now: Instant) {
        if self
            .recent_at
            .is_none_or(|at| now.duration_since(at) >= FIND_TRANSCRIPT_EVERY)
        {
            self.recent_at = Some(now);
            self.recent = recently_written(&self.claude.join("projects"));
        }
    }

    fn guess(&self, id: String, now_ms: u64) -> Seen {
        let tracked = self
            .tracked
            .get(&id)
            .and_then(|tracked| tracked.transcript.as_ref());
        let path = tracked
            .map(|transcript| transcript.path().to_path_buf())
            .or_else(|| {
                self.recent
                    .iter()
                    .find(|(recent, _)| *recent == id)
                    .map(|(_, path)| path.clone())
            })
            .or_else(|| registry::transcript(&self.claude.join("projects"), &id));
        let written = path
            .and_then(|path| fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(UNIX_EPOCH);
        let wall = UNIX_EPOCH + Duration::from_millis(now_ms);
        let quiet = wall.duration_since(written).unwrap_or_default();
        let ended = tracked.is_some_and(|transcript| transcript.state.ended_turn);
        Seen {
            id,
            pid: None,
            cwd: None,
            name: None,
            remote: None,
            state: if quiet < WORKING_IF_WRITTEN_WITHIN && !ended {
                SessionState::Working { tool: None }
            } else {
                SessionState::Idle
            },
            state_secs: quiet.as_secs() as u32,
        }
    }
}

fn recently_written(projects: &Path) -> Vec<(String, PathBuf)> {
    let Ok(projects) = fs::read_dir(projects) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut recent: Vec<(SystemTime, String, PathBuf)> = projects
        .flatten()
        .filter_map(|project| fs::read_dir(project.path()).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.to_string();
            let written = entry.metadata().ok()?.modified().ok()?;
            (path.extension()? == "jsonl"
                && now.duration_since(written).unwrap_or_default() < ACTIVE_WITHIN)
                .then_some((written, id, path))
        })
        .collect();
    recent.sort_by_key(|(written, _, _)| std::cmp::Reverse(*written));
    recent.truncate(MAX_SESSIONS);
    recent.into_iter().map(|(_, id, path)| (id, path)).collect()
}

fn secs_between(earlier_ms: u64, now_ms: u64) -> u32 {
    (now_ms.saturating_sub(earlier_ms) / 1000) as u32
}

fn state(status: &Status) -> SessionState {
    match status {
        Status::Busy => SessionState::Working { tool: None },
        Status::Waiting(reason) if reason == "permission prompt" => {
            SessionState::Waiting(Waiting::Permission(None))
        }
        Status::Waiting(reason) => SessionState::Waiting(Waiting::Input(reason.clone())),
        Status::Idle => SessionState::Idle,
    }
}

fn with_tool(state: SessionState, tool: Option<&str>) -> SessionState {
    let tool = tool.map(str::to_string);
    match state {
        SessionState::Working { .. } => SessionState::Working { tool },
        SessionState::Waiting(Waiting::Permission(_)) => {
            SessionState::Waiting(Waiting::Permission(tool))
        }
        other => other,
    }
}

fn session_cwd(session: &Seen, tracked: &Tracked) -> String {
    session
        .cwd
        .clone()
        .or_else(|| {
            tracked
                .transcript
                .as_ref()
                .and_then(|transcript| transcript.state.cwd.clone())
        })
        .unwrap_or_default()
}

fn with_steps(mut children: Vec<Child>, tracked: &Tracked) -> Vec<Child> {
    for child in &mut children {
        let Some(agent) = &child.task else {
            continue;
        };
        let stem = format!("agent-{agent}");
        child.doing = tracked
            .subagents
            .iter()
            .find(|(path, _)| path.file_stem().is_some_and(|name| *name == *stem))
            .and_then(|(_, transcript)| transcript.running_tool())
            .map(str::to_string);
    }
    children
}

fn describe(
    session: Seen,
    tracked: &Tracked,
    context_used: Option<BasisPoints>,
    cwd: &str,
) -> AgentSession {
    let main = tracked.transcript.as_ref();
    let repo = repo::locate(cwd);
    let project = repo.project.clone();
    let title: String = session
        .name
        .unwrap_or_else(|| project.clone())
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    AgentSession {
        agent: Agent::ClaudeCode,
        id: session.id,
        title,
        project,
        state: with_tool(session.state, main.and_then(Transcript::running_tool)),
        state_secs: session.state_secs,
        model: main
            .and_then(|transcript| transcript.state.model.clone())
            .unwrap_or_default(),
        context_tokens: main.map_or(0, |transcript| transcript.state.context_tokens),
        context_used,
        cost_micros: tracked
            .transcripts()
            .map(|transcript| transcript.state.ledger.cost_micros)
            .sum(),
        cost_complete: tracked
            .transcripts()
            .all(|transcript| !transcript.state.ledger.unpriced),
        pane: None,
        usage: None,
        children: main
            .map(|transcript| with_steps(transcript.children(crate::log::now_secs()), tracked))
            .unwrap_or_default(),
        branch: repo.branch,
        parent: None,
        activity: None,
        mode: main.and_then(|transcript| transcript.state.mode),
        queued: None,
        remote: session.remote,
        pr: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_status_decides_the_state_and_the_transcript_names_the_tool() {
        assert_eq!(
            with_tool(state(&Status::Busy), Some("Edit")),
            SessionState::Working {
                tool: Some("Edit".into())
            }
        );
        assert_eq!(
            with_tool(
                state(&Status::Waiting("permission prompt".into())),
                Some("Bash")
            ),
            SessionState::Waiting(Waiting::Permission(Some("Bash".into())))
        );
        assert_eq!(
            state(&Status::Waiting("input needed".into())),
            SessionState::Waiting(Waiting::Input("input needed".into()))
        );
        assert_eq!(
            with_tool(state(&Status::Idle), Some("Bash")),
            SessionState::Idle
        );
    }

    #[test]
    fn a_changed_registry_format_falls_back_to_transcripts_and_says_so() {
        let root = std::env::temp_dir().join(format!("magi-claude-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let sessions = root.join("sessions");
        let project = root.join("projects/-Users-me-magi");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&project).unwrap();
        let me = std::process::id();
        fs::write(
            sessions.join("a.json"),
            format!(r#"{{"pid":{me},"sessionId":"abc","status":"compacting"}}"#),
        )
        .unwrap();
        fs::write(
            sessions.join("b.json"),
            format!(
                r#"{{"pid":{me},"sessionId":"def","cwd":"/Users/me/notes","status":"waiting","waitingFor":"permission prompt","statusUpdatedAt":1,"startedAt":2}}"#
            ),
        )
        .unwrap();
        fs::write(
            project.join("abc.jsonl"),
            concat!(
                r#"{"type":"assistant","cwd":"/Users/me/magi","message":{"id":"m1","model":"claude-opus-5-5","stop_reason":"tool_use","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
                "\n"
            ),
        )
        .unwrap();
        let mut claude = ClaudeSessions::new(root.clone());
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let polled = claude
            .poll(
                &Board::default(),
                Instant::now(),
                now_ms,
                &mut u64::MAX.clone(),
            )
            .unwrap();
        assert_eq!(polled.notices, [FORMAT_CHANGED]);
        assert_eq!(polled.sessions.len(), 2);
        let exact = &polled.sessions[0].session;
        assert_eq!(
            (exact.id.as_str(), &exact.state),
            ("def", &SessionState::Waiting(Waiting::Permission(None)))
        );
        let session = &polled.sessions[1].session;
        assert_eq!(
            (session.id.as_str(), session.project.as_str()),
            ("abc", "magi")
        );
        assert_eq!(
            session.state,
            SessionState::Working {
                tool: Some("Bash".into())
            }
        );
        assert!(session.cost_micros > 0);
        fs::remove_dir_all(root).unwrap();
    }
}
