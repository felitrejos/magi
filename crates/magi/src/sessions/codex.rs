use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::Found;
use super::codex_app;
use super::ledger::{Counted, Ledger};
use super::limits::{self, Limits};
use super::pricing::{self, OpenAiUsage};
use super::process;
use super::repo;
use super::tail::{self, Lines, Tail};
use crate::handoff;
use crate::proto::{Agent, AgentSession, BasisPoints, FULL, Plan, SessionState, Tokens, Window};

const PROCESS_NAME: &str = "codex";
const FIND_PROCESSES_EVERY: Duration = Duration::from_secs(2);
const MAX_SESSIONS: usize = 64;
const MAX_TITLE_CHARS: usize = 120;
const MARKS: [&[u8]; 9] = [
    b"\"type\":\"session_meta\"",
    b"\"type\":\"turn_context\"",
    b"\"type\":\"token_usage_record\"",
    b"\"type\":\"token_count\"",
    b"\"type\":\"task_started\"",
    b"\"type\":\"task_complete\"",
    b"\"type\":\"turn_aborted\"",
    b"\"call_id\"",
    b"\"thread_name\"",
];

pub struct Codex {
    home: PathBuf,
    found_at: Option<Instant>,
    live: Vec<(PathBuf, i32)>,
    rollouts: HashMap<PathBuf, Rollout>,
    titles: Option<(Tail, Titles)>,
    claims: HashSet<String>,
    asked: Option<Limits>,
}

struct Rollout {
    tail: Tail,
    state: State,
}

#[derive(Default)]
struct State {
    id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    started_ms: u64,
    working: bool,
    since_ms: u64,
    open_tools: Vec<(String, String)>,
    ledger: Ledger,
    context: Option<(u64, u64)>,
    limits: Option<(u64, Vec<Window>)>,
}

#[derive(Default)]
struct Titles(HashMap<String, String>);

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    payload: Option<Payload>,
}

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    usage: Option<UsageJson>,
    #[serde(default)]
    info: Option<Info>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize, Default)]
struct UsageJson {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Deserialize)]
struct Info {
    #[serde(default)]
    last_token_usage: Option<UsageJson>,
    #[serde(default)]
    model_context_window: Option<u64>,
}

#[derive(Deserialize)]
struct RateLimits {
    #[serde(default)]
    primary: Option<LimitWindow>,
    #[serde(default)]
    secondary: Option<LimitWindow>,
}

#[derive(Deserialize)]
struct LimitWindow {
    used_percent: f64,
    window_minutes: u32,
    resets_at: u64,
}

#[derive(Deserialize)]
struct TitleLine {
    id: String,
    thread_name: String,
}

struct Feed<'a> {
    state: &'a mut State,
    claims: &'a mut HashSet<String>,
}

impl Codex {
    pub fn new(home: PathBuf) -> Self {
        let home = std::fs::canonicalize(&home).unwrap_or(home);
        Self {
            home,
            found_at: None,
            live: Vec::new(),
            rollouts: HashMap::new(),
            titles: None,
            claims: HashSet::new(),
            asked: None,
        }
    }

    pub fn for_this_user() -> Option<Self> {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(Path::new(&std::env::var_os("HOME")?).join(".codex")))?;
        let mut codex = Self::new(home);
        codex.asked = handoff::find("codex", &handoff::search_dirs())
            .map(|program| Limits::new(move |_| codex_app::limits(&program)));
        Some(codex)
    }

    pub fn poll(
        &mut self,
        now: Instant,
        now_ms: u64,
        budget: &mut u64,
    ) -> (Vec<Found>, Option<Plan>) {
        if self
            .found_at
            .is_none_or(|at| now.duration_since(at) >= FIND_PROCESSES_EVERY)
        {
            self.found_at = Some(now);
            self.live = self.find_live();
        }
        let live = &self.live;
        let claims = &mut self.claims;
        self.rollouts.retain(|path, rollout| {
            let keep = live.iter().any(|(live, _)| live == path);
            if !keep {
                rollout.state.ledger.release(claims);
            }
            keep
        });
        if !self.live.is_empty() {
            self.read_titles(budget);
        }
        let mut sessions = Vec::new();
        for (path, _) in &self.live {
            let rollout = self
                .rollouts
                .entry(path.clone())
                .or_insert_with(|| Rollout {
                    tail: Tail::new(path.clone()),
                    state: State::default(),
                });
            let _ = rollout.tail.advance(
                budget,
                &mut Feed {
                    state: &mut rollout.state,
                    claims: &mut self.claims,
                },
            );
            rollout.state.ledger.adopt(&mut self.claims);
        }
        let titles = self.titles.as_ref().map(|(_, titles)| titles);
        for (path, pid) in &self.live {
            if let Some(session) = self
                .rollouts
                .get(path)
                .and_then(|rollout| describe(&rollout.state, titles, now_ms))
            {
                let cwd = self
                    .rollouts
                    .get(path)
                    .and_then(|rollout| rollout.state.cwd.clone())
                    .unwrap_or_default();
                sessions.push(Found {
                    session,
                    pid: Some(*pid),
                    transcript: Some(path.clone()),
                    cwd,
                });
            }
        }
        (sessions, self.plan(now, now_ms / 1000))
    }

    fn find_live(&self) -> Vec<(PathBuf, i32)> {
        let sessions = self.home.join("sessions");
        if !sessions.is_dir() {
            return Vec::new();
        }
        let mut live: Vec<(PathBuf, i32)> = process::open_files(PROCESS_NAME)
            .into_iter()
            .filter(|(_, path)| path.starts_with(&sessions) && is_rollout(path))
            .map(|(pid, path)| (path, pid))
            .collect();
        live.sort();
        live.dedup_by(|a, b| a.0 == b.0);
        live.truncate(MAX_SESSIONS);
        live
    }

    fn read_titles(&mut self, budget: &mut u64) {
        let (tail, titles) = self.titles.get_or_insert_with(|| {
            (
                Tail::new(self.home.join("session_index.jsonl")),
                Titles::default(),
            )
        });
        let _ = tail.advance(budget, titles);
    }

    fn plan(&mut self, now: Instant, now_secs: u64) -> Option<Plan> {
        let seen = self
            .rollouts
            .values()
            .filter_map(|rollout| rollout.state.limits.as_ref())
            .max_by_key(|(at_ms, _)| *at_ms)
            .map(|(at_ms, windows)| (at_ms / 1000, windows.clone()));
        limits::plan(Agent::Codex, seen, self.asked.as_mut(), now, now_secs)
    }
}

fn is_rollout(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

impl Lines for Titles {
    fn restart(&mut self) {
        self.0.clear();
    }

    fn line(&mut self, line: &[u8]) {
        if let Ok(title) = serde_json::from_slice::<TitleLine>(line) {
            self.0.insert(title.id, title.thread_name);
        }
    }
}

impl Lines for Feed<'_> {
    fn restart(&mut self) {
        self.state.ledger.release(self.claims);
        *self.state = State::default();
    }

    fn line(&mut self, line: &[u8]) {
        if !MARKS.iter().any(|mark| tail::contains(line, mark)) {
            return;
        }
        let Ok(parsed) = serde_json::from_slice::<Line>(line) else {
            return;
        };
        let Some(payload) = parsed.payload else {
            return;
        };
        let at_ms = parsed
            .timestamp
            .as_deref()
            .and_then(parse_iso_ms)
            .unwrap_or(0);
        let state = &mut *self.state;
        match (parsed.kind.as_str(), payload.kind.as_deref()) {
            ("session_meta", _) => {
                state.id = payload.id;
                state.cwd = payload.cwd;
                state.started_ms = payload
                    .timestamp
                    .as_deref()
                    .and_then(parse_iso_ms)
                    .unwrap_or(at_ms);
                state.since_ms = state.since_ms.max(state.started_ms);
            }
            ("turn_context", _) => {
                state.model = payload.model.or(state.model.take());
                state.cwd = payload.cwd.or(state.cwd.take());
            }
            ("token_usage_record", _) => state.count(&payload, self.claims),
            ("event_msg", Some("task_started")) => state.turn(true, at_ms),
            ("event_msg", Some("task_complete" | "turn_aborted")) => state.turn(false, at_ms),
            ("event_msg", Some("token_count")) => state.measure(payload, at_ms),
            ("response_item", Some(kind)) => state.tool(kind, payload.call_id, payload.name),
            _ => {}
        }
    }
}

impl State {
    fn turn(&mut self, working: bool, at_ms: u64) {
        self.working = working;
        self.since_ms = at_ms;
        self.open_tools.clear();
    }

    fn tool(&mut self, kind: &str, call_id: Option<String>, name: Option<String>) {
        let Some(call_id) = call_id else {
            return;
        };
        if kind.ends_with("_output") {
            self.open_tools.retain(|(open, _)| *open != call_id);
        } else if kind.ends_with("_call") {
            let name = name.unwrap_or_else(|| kind.trim_end_matches("_call").replace('_', " "));
            self.open_tools.push((call_id, name));
        }
    }

    fn measure(&mut self, payload: Payload, at_ms: u64) {
        if let Some(info) = payload.info
            && let (Some(last), Some(window)) = (info.last_token_usage, info.model_context_window)
            && window > 0
        {
            self.context = Some((last.total_tokens, window));
        }
        if let Some(limits) = payload.rate_limits {
            let windows = [limits.primary, limits.secondary]
                .into_iter()
                .flatten()
                .map(|window| Window {
                    minutes: window.window_minutes,
                    used: (window.used_percent.clamp(0.0, 100.0) * f64::from(FULL) / 100.0).round()
                        as BasisPoints,
                    resets_at: window.resets_at,
                })
                .collect();
            self.limits = Some((at_ms, windows));
        }
    }

    fn count(&mut self, payload: &Payload, claims: &mut HashSet<String>) {
        let (Some(id), Some(usage)) = (&payload.response_id, &payload.usage) else {
            return;
        };
        let price = self.model.as_deref().and_then(|model| {
            pricing::openai_cost_micros(
                model,
                &OpenAiUsage {
                    input: usage.input_tokens,
                    cached: usage.cached_input_tokens,
                    cache_write: usage.cache_write_input_tokens,
                    output: usage.output_tokens,
                },
            )
        });
        self.ledger.count(
            id,
            Counted {
                tokens: Tokens {
                    input: usage.input_tokens.saturating_sub(usage.cached_input_tokens),
                    output: usage.output_tokens,
                    cache_read: usage.cached_input_tokens,
                    cache_write: usage.cache_write_input_tokens,
                },
                cost: price.unwrap_or(0),
                priced: price.is_some(),
            },
            claims,
        );
    }
}

fn describe(state: &State, titles: Option<&Titles>, now_ms: u64) -> Option<AgentSession> {
    let id = state.id.clone()?;
    let cwd = state.cwd.clone().unwrap_or_default();
    let repo = repo::locate(&cwd);
    let project = repo.project.clone();
    let title: String = titles
        .and_then(|titles| titles.0.get(&id))
        .cloned()
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| project.clone())
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    let session_state = if state.working {
        SessionState::Working {
            tool: state.open_tools.last().map(|(_, name)| name.clone()),
        }
    } else {
        SessionState::Idle
    };
    Some(AgentSession {
        agent: Agent::Codex,
        id,
        title,
        project,
        state: session_state,
        state_secs: (now_ms.saturating_sub(state.since_ms) / 1000) as u32,
        model: state.model.clone().unwrap_or_default(),
        context_tokens: state.context.map_or(0, |(tokens, _)| tokens),
        context_used: state
            .context
            .map(|(tokens, window)| (tokens.min(window) * u64::from(FULL) / window) as BasisPoints),
        tokens: state.ledger.tokens,
        cost_micros: state.ledger.cost_micros,
        cost_complete: !state.ledger.unpriced,
        age_secs: (now_ms.saturating_sub(state.started_ms) / 1000) as u32,
        pane: None,
        usage: None,
        children: Vec::new(),
        branch: repo.branch,
        worktree: repo.worktree,
        parent: None,
        activity: None,
    })
}

fn parse_iso_ms(text: &str) -> Option<u64> {
    let (seconds, fraction) = text
        .trim_end_matches('Z')
        .split_once('.')
        .unwrap_or((text.trim_end_matches('Z'), ""));
    let seconds = CString::new(seconds).ok()?;
    let mut tm = MaybeUninit::<libc::tm>::zeroed();
    let end = unsafe {
        libc::strptime(
            seconds.as_ptr(),
            c"%Y-%m-%dT%H:%M:%S".as_ptr(),
            tm.as_mut_ptr(),
        )
    };
    if end.is_null() || unsafe { *end } != 0 {
        return None;
    }
    let mut tm = unsafe { tm.assume_init() };
    let secs = u64::try_from(unsafe { libc::timegm(&mut tm) }).ok()?;
    let digits: Vec<u8> = fraction.bytes().take(3).collect();
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let millis = digits
        .iter()
        .chain(std::iter::repeat(&b'0'))
        .take(3)
        .fold(0, |millis, digit| millis * 10 + u64::from(digit - b'0'));
    Some(secs * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::tail::tests::{append, temp};
    use super::*;

    const META: &str = r#"{"timestamp":"2026-11-11T00:45:46.100Z","type":"session_meta","payload":{"id":"thread-1","session_id":"thread-1","timestamp":"2026-11-11T00:45:46.000Z","cwd":"/Users/me/magi","originator":"codex_cli_rs","source":"cli"}}"#;
    const CONTEXT: &str = r#"{"timestamp":"2026-11-11T00:45:47.000Z","type":"turn_context","payload":{"turn_id":"t1","cwd":"/Users/me/magi","model":"gpt-5.6-terra","approval_policy":"on-request"}}"#;
    const STARTED: &str = r#"{"timestamp":"2026-11-11T00:45:48.000Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#;
    const CALL: &str = r#"{"timestamp":"2026-11-11T00:45:49.000Z","type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"c1","name":"apply_patch","input":"*** Begin Patch"}}"#;
    const OUTPUT: &str = r#"{"timestamp":"2026-11-11T00:45:50.000Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":"done"}}"#;
    const COMPLETE: &str = r#"{"timestamp":"2026-11-11T00:46:00.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"ok"}}"#;

    fn record(response: &str, input: u64, cached: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":"2026-11-11T00:45:51.000Z","type":"token_usage_record","payload":{{"thread_id":"thread-1","turn_id":"t1","response_id":"{response}","usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":{}}}}}}}"#,
            input + output
        )
    }

    fn count(used: f64) -> String {
        format!(
            r#"{{"timestamp":"2026-11-11T00:45:52.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1,"reasoning_output_tokens":0,"total_tokens":2}},"last_token_usage":{{"input_tokens":20000,"cached_input_tokens":0,"output_tokens":500,"reasoning_output_tokens":0,"total_tokens":20500}},"model_context_window":205000}},"rate_limits":{{"limit_id":"codex","primary":{{"used_percent":{used},"window_minutes":300,"resets_at":4102444800}},"secondary":null,"plan_type":"pro"}}}}}}"#
        )
    }

    fn read(path: &Path, state: &mut State, claims: &mut HashSet<String>) {
        Tail::new(path.to_path_buf())
            .advance(&mut u64::MAX.clone(), &mut Feed { state, claims })
            .unwrap();
    }

    #[test]
    fn a_rollout_gives_state_tool_model_tokens_cost_context_and_limits() {
        let path = temp("codex-rollout");
        append(
            &path,
            &[
                META,
                CONTEXT,
                STARTED,
                CALL,
                &record("r1", 200_000, 80_000, 100_000),
                &count(12.5),
            ]
            .join("\n"),
        );
        append(&path, "\n");
        let mut state = State::default();
        let mut claims = HashSet::new();
        read(&path, &mut state, &mut claims);
        let session = describe(&state, None, 1_794_444_349_000 + 1_000).unwrap();
        assert_eq!(session.title, "magi");
        assert_eq!(
            session.state,
            SessionState::Working {
                tool: Some("apply_patch".into())
            }
        );
        assert_eq!(session.model, "gpt-5.6-terra");
        assert_eq!(
            session.tokens,
            Tokens {
                input: 120_000,
                output: 100_000,
                cache_read: 80_000,
                cache_write: 0
            }
        );
        assert_eq!(session.cost_micros, 240_000 + 16_000 + 1_200_000);
        assert!(session.cost_complete);
        assert_eq!(
            (session.context_tokens, session.context_used),
            (20_500, Some(1_000))
        );
        assert_eq!(
            state.limits.as_ref().unwrap().1,
            vec![Window {
                minutes: 300,
                used: 1_250,
                resets_at: 4_102_444_800
            }]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_finished_turn_is_idle_and_closes_tools() {
        let path = temp("codex-idle");
        append(
            &path,
            &[META, CONTEXT, STARTED, CALL, OUTPUT, COMPLETE, ""].join("\n"),
        );
        let mut state = State::default();
        read(&path, &mut state, &mut HashSet::new());
        let session = describe(
            &state,
            None,
            parse_iso_ms("2026-11-11T00:46:30.000Z").unwrap(),
        )
        .unwrap();
        assert_eq!(
            (session.state, session.state_secs),
            (SessionState::Idle, 30)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_fork_counts_the_shared_responses_once() {
        let original = temp("codex-original");
        let fork = temp("codex-fork");
        append(
            &original,
            &[META, CONTEXT, &record("r1", 10, 0, 100), ""].join("\n"),
        );
        append(
            &fork,
            &[
                META,
                CONTEXT,
                &record("r1", 10, 0, 100),
                &record("r2", 10, 0, 7),
                "",
            ]
            .join("\n"),
        );
        let mut claims = HashSet::new();
        let (mut first, mut second) = (State::default(), State::default());
        read(&original, &mut first, &mut claims);
        read(&fork, &mut second, &mut claims);
        assert_eq!(
            (first.ledger.tokens.output, second.ledger.tokens.output),
            (100, 7)
        );
        fs::remove_file(original).unwrap();
        fs::remove_file(fork).unwrap();
    }

    #[test]
    fn unknown_models_and_titles_from_the_session_index() {
        let path = temp("codex-unknown");
        append(
            &path,
            &[
                META,
                &CONTEXT.replace("gpt-5.6-terra", "gpt-9"),
                &record("r1", 10, 0, 1),
                "",
            ]
            .join("\n"),
        );
        let mut state = State::default();
        read(&path, &mut state, &mut HashSet::new());
        let mut titles = Titles::default();
        titles.line(br#"{"id":"thread-1","thread_name":"Fix the login","updated_at":"2026-11-11T00:50:00Z"}"#);
        let session = describe(&state, Some(&titles), 0).unwrap();
        assert_eq!(session.title, "Fix the login");
        assert!(!session.cost_complete);
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "reads this machine's Codex sessions: cargo test --release -- --ignored --nocapture real_codex"]
    fn real_codex_rollouts() {
        let home = Codex::for_this_user().unwrap().home;
        let mut paths = Vec::new();
        let mut dirs = vec![home.join("sessions")];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if is_rollout(&path) {
                    paths.push(path);
                }
            }
        }
        let mut claims = HashSet::new();
        for path in paths {
            let mut state = State::default();
            read(&path, &mut state, &mut claims);
            let session = describe(&state, None, 0).unwrap();
            println!(
                "{} {:<14} in {:>8} cached {:>8} out {:>7} cost ${:.4}{}",
                &session.id[..8],
                session.model,
                session.tokens.input + session.tokens.cache_read,
                session.tokens.cache_read,
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

    #[test]
    fn iso_timestamps_parse_to_milliseconds() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.250Z"), Some(1_250));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:02Z"), Some(2_000));
        assert_eq!(parse_iso_ms("yesterday"), None);
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.éé"), None);
    }
}
