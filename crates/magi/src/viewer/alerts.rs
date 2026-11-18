use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::base64;
use super::model::{self, Level, LinkState, Model, Subsystem};
use crate::proto::{Checks, FULL, Plan, PrState, SessionState, Waiting};

const KEPT: usize = 200;
const REWRITE_AT: usize = KEPT * 2;
const LOST_FOR: Duration = Duration::from_secs(30);
const CRITICAL_FOR: Duration = Duration::from_secs(60);
const CLEAR_AFTER: Duration = Duration::from_secs(30);
const LOST_REMEMBERED_SECS: u64 = 24 * 60 * 60;
const PLAN_ALERT_AT: u16 = 9_000;
const PLAN_HOLD: Duration = Duration::from_secs(60);
const INHERIT_GRACE: Duration = Duration::from_secs(30);
const FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Permission,
    Question,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub key: String,
    pub machine: String,
    pub text: String,
    pub severity: Severity,
    hold: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Event {
    Raised,
    Resolved,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub event: Event,
    pub key: String,
    pub machine: String,
    pub text: String,
    pub severity: Severity,
}

#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub raised: Vec<Condition>,
    pub resolved: Vec<String>,
}

struct Active {
    condition: Condition,
    raised_at: u64,
    missing_since: Option<Instant>,
    dismissed: bool,
}

pub struct Alerts {
    active: HashMap<String, Active>,
    pending: HashMap<String, Instant>,
    inherited: HashMap<String, Entry>,
    inherited_dismissed: HashMap<String, Entry>,
    elsewhere: Vec<String>,
    inherit_until: Instant,
    history: VecDeque<Entry>,
    log: Option<Log>,
    problem: Option<String>,
}

struct Log {
    path: PathBuf,
    lines: usize,
    bytes: u64,
}

impl Alerts {
    pub fn in_memory(now: Instant) -> Self {
        Self::with_history(VecDeque::new(), None, now)
    }

    pub fn open(path: PathBuf, now: Instant) -> io::Result<Self> {
        let history = read_history(&path)?;
        Ok(Self::with_history(history, Some(path), now))
    }

    fn with_history(history: VecDeque<Entry>, path: Option<PathBuf>, now: Instant) -> Self {
        let (open, dismissed) = still_open(&history);
        let lines = history.len();
        Self {
            active: HashMap::new(),
            pending: HashMap::new(),
            inherited: open,
            inherited_dismissed: dismissed,
            elsewhere: Vec::new(),
            inherit_until: now + INHERIT_GRACE,
            history,
            log: path.map(|path| Log {
                bytes: size(&path),
                path,
                lines,
            }),
            problem: None,
        }
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn active(&self) -> impl Iterator<Item = &Condition> {
        self.active
            .values()
            .filter(|active| !active.dismissed)
            .map(|active| &active.condition)
    }

    fn lost_machines(&self) -> impl Iterator<Item = (&str, &str, u64)> {
        self.active
            .iter()
            .map(|(key, active)| {
                (
                    key.as_str(),
                    active.condition.machine.as_str(),
                    active.raised_at,
                )
            })
            .chain(
                self.inherited
                    .iter()
                    .map(|(key, entry)| (key.as_str(), entry.machine.as_str(), entry.at)),
            )
            .filter(|(key, _, _)| key.starts_with("machine/") && key.ends_with("/lost"))
    }

    #[cfg(test)]
    pub fn history(&self) -> impl Iterator<Item = &Entry> {
        self.history.iter().rev()
    }

    pub fn update(&mut self, conditions: Vec<Condition>, now: Instant, now_secs: u64) -> Changes {
        let mut changes = Changes {
            resolved: {
                let mut resolved = std::mem::take(&mut self.elsewhere);
                resolved.extend(self.dismissed_elsewhere());
                resolved
            },
            ..Changes::default()
        };
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for condition in conditions {
            seen.insert(condition.key.clone());
            if let Some(active) = self.active.get_mut(&condition.key) {
                active.missing_since = None;
                if !active.dismissed
                    && (active.condition.text != condition.text
                        || active.condition.severity != condition.severity)
                {
                    changes.raised.push(condition.clone());
                    active.condition = condition;
                }
                continue;
            }
            if self.inherited_dismissed.remove(&condition.key).is_some() {
                self.activate(condition, now_secs, true);
                continue;
            }
            if let Some(raised) = self.inherited.remove(&condition.key) {
                changes.raised.push(condition.clone());
                self.activate(condition, raised.at, false);
                continue;
            }
            let since = *self.pending.entry(condition.key.clone()).or_insert(now);
            if now.duration_since(since) < condition.hold {
                continue;
            }
            self.pending.remove(&condition.key);
            entries.push(entry(&condition, Event::Raised, now_secs));
            changes.raised.push(condition.clone());
            self.activate(condition, now_secs, false);
        }
        self.pending.retain(|key, _| seen.contains(key));
        let mut ended = Vec::new();
        for (key, active) in &mut self.active {
            if seen.contains(key) {
                continue;
            }
            let since = *active.missing_since.get_or_insert(now);
            let clear_after = if active.condition.hold.is_zero() {
                Duration::ZERO
            } else {
                CLEAR_AFTER
            };
            if now.duration_since(since) >= clear_after {
                ended.push(key.clone());
            }
        }
        for key in ended {
            if let Some(active) = self.active.remove(&key) {
                entries.push(entry(&active.condition, Event::Resolved, now_secs));
                changes.resolved.push(key);
            }
        }
        if now >= self.inherit_until {
            for (_, dismissed) in self.inherited_dismissed.drain() {
                entries.push(Entry {
                    at: now_secs,
                    event: Event::Resolved,
                    ..dismissed
                });
            }
            for (key, raised) in self.inherited.drain() {
                entries.push(Entry {
                    at: now_secs,
                    event: Event::Resolved,
                    ..raised
                });
                changes.resolved.push(key);
            }
        }
        self.save(entries);
        changes
    }

    fn dismissed_elsewhere(&mut self) -> Vec<String> {
        let Some(log) = &mut self.log else {
            return Vec::new();
        };
        let bytes = size(&log.path);
        if bytes == log.bytes {
            return Vec::new();
        }
        log.bytes = bytes;
        let Ok(history) = read_history(&log.path) else {
            return Vec::new();
        };
        let mut dismissed = Vec::new();
        for entry in history
            .into_iter()
            .filter(|entry| entry.event == Event::Dismissed)
        {
            let key = entry.key.clone();
            let newly = match self.active.get_mut(&key) {
                Some(active) if !active.dismissed && entry.at >= active.raised_at => {
                    active.dismissed = true;
                    true
                }
                Some(_) => false,
                None => match self.inherited.get(&key) {
                    Some(raised) if entry.at >= raised.at => {
                        self.inherited.remove(&key);
                        self.inherited_dismissed.insert(key.clone(), entry.clone());
                        true
                    }
                    _ => false,
                },
            };
            if newly {
                self.history.push_back(entry);
                dismissed.push(key);
            }
        }
        dismissed
    }

    fn activate(&mut self, condition: Condition, raised_at: u64, dismissed: bool) {
        self.active.insert(
            condition.key.clone(),
            Active {
                condition,
                raised_at,
                missing_since: None,
                dismissed,
            },
        );
    }

    fn save(&mut self, entries: Vec<Entry>) {
        if !entries.is_empty() {
            self.problem = self
                .record(&entries)
                .err()
                .map(|error| format!("alert history not saved: {error}"));
        }
    }

    fn record(&mut self, entries: &[Entry]) -> io::Result<()> {
        self.history.extend(entries.iter().cloned());
        while self.history.len() > KEPT {
            self.history.pop_front();
        }
        let Some(log) = &self.log else {
            return Ok(());
        };
        let written_elsewhere = size(&log.path) != log.bytes;
        if written_elsewhere && log.lines + entries.len() > REWRITE_AT {
            let dismissed = self.dismissed_elsewhere();
            self.elsewhere.extend(dismissed);
        }
        let Some(log) = &mut self.log else {
            return Ok(());
        };
        log.lines += entries.len();
        if log.lines > REWRITE_AT {
            log.lines = self.history.len();
            rewrite(&log.path, &self.history)?;
            log.bytes = size(&log.path);
            return Ok(());
        }
        let text = lines(entries)?;
        append(&log.path, &text)?;
        if !written_elsewhere {
            log.bytes += text.len() as u64;
        }
        Ok(())
    }
}

fn entry(condition: &Condition, event: Event, at: u64) -> Entry {
    Entry {
        at,
        event,
        key: condition.key.clone(),
        machine: condition.machine.clone(),
        text: condition.text.clone(),
        severity: condition.severity,
    }
}

pub fn read_history(path: &PathBuf) -> io::Result<VecDeque<Entry>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(error) => return Err(error),
    };
    let mut history = VecDeque::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(entry) = serde_json::from_slice::<Entry>(line) else {
            continue;
        };
        history.push_back(entry);
        if history.len() > KEPT {
            history.pop_front();
        }
    }
    Ok(history)
}

pub fn dismiss_open(path: &PathBuf, now_secs: u64) -> io::Result<()> {
    let (open, _) = still_open(&read_history(path)?);
    let entries: Vec<Entry> = open
        .into_values()
        .map(|raised| Entry {
            at: now_secs,
            event: Event::Dismissed,
            ..raised
        })
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    append(path, &lines(&entries)?)
}

fn still_open(history: &VecDeque<Entry>) -> (HashMap<String, Entry>, HashMap<String, Entry>) {
    let mut open = HashMap::new();
    let mut dismissed = HashMap::new();
    for entry in history {
        match entry.event {
            Event::Raised => {
                dismissed.remove(&entry.key);
                open.insert(entry.key.clone(), entry.clone());
            }
            Event::Resolved => {
                dismissed.remove(&entry.key);
                open.remove(&entry.key);
            }
            Event::Dismissed => {
                if open.remove(&entry.key).is_some() {
                    dismissed.insert(entry.key.clone(), entry.clone());
                }
            }
        }
    }
    (open, dismissed)
}

fn size(path: &PathBuf) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn open_private(path: &PathBuf, options: &mut OpenOptions) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    options.mode(FILE_MODE).open(path)
}

fn lines<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> io::Result<String> {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry).map_err(io::Error::other)?);
        text.push('\n');
    }
    Ok(text)
}

fn append(path: &PathBuf, text: &str) -> io::Result<()> {
    open_private(path, OpenOptions::new().create(true).append(true))?.write_all(text.as_bytes())
}

fn rewrite(path: &PathBuf, history: &VecDeque<Entry>) -> io::Result<()> {
    let temp = path.with_extension(format!("jsonl.{}", std::process::id()));
    let mut file = open_private(
        &temp,
        OpenOptions::new().write(true).create(true).truncate(true),
    )?;
    file.write_all(lines(history)?.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

pub fn conditions(model: &Model, now: Instant, now_secs: u64) -> Vec<Condition> {
    let rows = model.rows(now);
    let mut found = Vec::new();
    for agent in model.agents(now).iter().filter(|agent| agent.live) {
        if let Some(pr) = agent.session.pr.as_ref().filter(|pr| {
            pr.checks == Checks::Failing && matches!(pr.state, PrState::Open | PrState::Draft)
        }) {
            found.push(Condition {
                key: format!("pr/{}/{}", agent.machine, agent.session.id),
                machine: agent.machine.clone(),
                text: format!("{}: PR #{} checks failing", agent.session.title, pr.number),
                severity: Severity::Error,
                hold: Duration::ZERO,
            });
        }
        let SessionState::Waiting(waiting) = &agent.session.state else {
            continue;
        };
        let (severity, what) = match waiting {
            Waiting::Trust => (Severity::Permission, "asks you to trust its folder".into()),
            Waiting::Permission(Some(tool)) => {
                (Severity::Permission, format!("wants to run {tool}"))
            }
            Waiting::Permission(None) => (Severity::Permission, "wants permission".into()),
            Waiting::Input(reason) => (Severity::Question, reason.clone()),
        };
        found.push(Condition {
            key: format!("agent/{}/{}", agent.machine, agent.session.id),
            machine: agent.machine.clone(),
            text: format!("{} needs you · {what}", agent.session.title),
            severity,
            hold: Duration::ZERO,
        });
    }
    for row in &rows {
        match row.state() {
            Some(LinkState::Lost { .. }) => found.push(Condition {
                key: format!("machine/{}/lost", row.name),
                machine: row.name.clone(),
                text: format!("{} stopped answering", row.name),
                severity: Severity::Error,
                hold: LOST_FOR,
            }),
            Some(LinkState::Live) => {
                let Some(machine) = row.machine() else {
                    continue;
                };
                for (subsystem, value) in [
                    (Subsystem::Cpu, machine.smoothed_cpu()),
                    (Subsystem::Memory, machine.memory_fraction()),
                    (Subsystem::Storage, machine.fullest_filesystem()),
                ] {
                    if model::grade(subsystem, value) != Level::Critical {
                        continue;
                    }
                    let name = subsystem.name();
                    found.push(Condition {
                        key: format!("machine/{}/{name}", row.name),
                        machine: row.name.clone(),
                        text: format!("{} {name} critical", row.name),
                        severity: Severity::Error,
                        hold: CRITICAL_FOR,
                    });
                }
            }
            _ => {}
        }
    }
    for (key, machine, raised_at) in model.alerts.lost_machines() {
        let answering = rows
            .iter()
            .any(|row| row.name == machine && row.state() == Some(&LinkState::Live));
        let listed = found.iter().any(|condition| condition.key == key);
        if !answering && !listed && now_secs.saturating_sub(raised_at) < LOST_REMEMBERED_SECS {
            found.push(Condition {
                key: key.to_string(),
                machine: machine.to_string(),
                text: format!("{machine} stopped answering"),
                severity: Severity::Error,
                hold: LOST_FOR,
            });
        }
    }
    found.extend(plan_conditions(&model.plans(now)));
    found
}

fn plan_conditions(plans: &[&Plan]) -> Vec<Condition> {
    let mut found = Vec::new();
    for plan in plans {
        for window in plan
            .windows
            .iter()
            .filter(|window| window.used >= PLAN_ALERT_AT)
        {
            let label = window.label();
            let mut text = format!(
                "{} {label} plan {}% used",
                plan.agent.name(),
                u32::from(window.used) * 100 / u32::from(FULL)
            );
            if let Some(other) = plans
                .iter()
                .find(|other| other.agent != plan.agent && has_room(other))
            {
                text.push_str(&format!(
                    ". Start the next session in {}",
                    other.agent.name()
                ));
            }
            found.push(Condition {
                key: format!("plan/{:?}/{label}", plan.agent),
                machine: String::new(),
                text,
                severity: Severity::Error,
                hold: PLAN_HOLD,
            });
        }
    }
    found
}

fn has_room(plan: &Plan) -> bool {
    !plan.windows.is_empty()
        && plan
            .windows
            .iter()
            .all(|window| window.used < PLAN_ALERT_AT)
}

pub fn report(changes: &Changes) -> String {
    let mut out = String::new();
    for key in &changes.resolved {
        out.push_str(&format!(
            "\x1b]7501;state=clear:id={}\x1b\\",
            record_id(key)
        ));
    }
    for condition in &changes.raised {
        let state = match condition.severity {
            Severity::Permission => "blocked:kind=permission",
            Severity::Question => "blocked:kind=question",
            Severity::Error => "error",
        };
        let title = if condition.machine.is_empty() {
            "plan"
        } else {
            &condition.machine
        };
        out.push_str(&format!(
            "\x1b]7501;state={state}:app=magi:id={}:title={}:msg={}\x1b\\",
            record_id(&condition.key),
            base64::encode(printable(title, MAX_TITLE_BYTES).as_bytes()),
            base64::encode(printable(&condition.text, MAX_MSG_BYTES).as_bytes()),
        ));
    }
    out
}

pub const CLEAR_ALL: &str = "\x1b]7501;state=clear\x1b\\";
const MAX_TITLE_BYTES: usize = 192;
const MAX_MSG_BYTES: usize = 2048;
const MAX_SEGMENT: usize = 32;
const MAX_DEPTH: usize = 8;

fn record_id(key: &str) -> String {
    key.split('/')
        .take(MAX_DEPTH)
        .map(record_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn record_segment(segment: &str) -> String {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "_.+-".contains(c);
    if !segment.is_empty() && segment.len() <= MAX_SEGMENT && segment.chars().all(allowed) {
        return segment.to_string();
    }
    let cleaned: String = segment
        .chars()
        .map(|c| if allowed(c) { c } else { '-' })
        .take(MAX_SEGMENT - 9)
        .collect();
    format!("{cleaned}-{:08x}", fnv1a(segment.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash: u32, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

fn printable(text: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > max_bytes {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::os::unix::fs::PermissionsExt;

    use super::super::model::{Endpoint, Link, LinkUpdate};
    use super::*;
    use crate::proto::{Identity, Welcome};

    fn condition(key: &str, hold: Duration) -> Condition {
        Condition {
            key: key.into(),
            machine: "homelab".into(),
            text: format!("{key} text"),
            severity: Severity::Error,
            hold,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "magi-alerts-{}-{name}/alerts.jsonl",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(path.parent().unwrap());
        path
    }

    fn events(alerts: &Alerts) -> Vec<Event> {
        alerts.history().map(|entry| entry.event).collect()
    }

    #[test]
    fn reports_follow_the_program_status_protocol() {
        assert_eq!(printable("a\nb\u{1b}c", 2048), "abc");
        assert_eq!(printable("ééé", 4), "éé");
        let changes = Changes {
            raised: vec![Condition {
                key: "agent/mac/s1".into(),
                machine: "mac".into(),
                text: "deploy needs you · wants to run Bash".into(),
                severity: Severity::Permission,
                hold: Duration::ZERO,
            }],
            resolved: vec!["machine/nas/lost".into()],
        };
        assert_eq!(
            report(&changes),
            format!(
                "\x1b]7501;state=clear:id=machine/nas/lost\x1b\\\x1b]7501;state=blocked:kind=permission:app=magi:id=agent/mac/s1:title={}:msg={}\x1b\\",
                base64::encode(b"mac"),
                base64::encode("deploy needs you · wants to run Bash".as_bytes())
            )
        );
        let long = Condition {
            key: "k".into(),
            machine: "m".repeat(500),
            text: "x".repeat(10_000),
            severity: Severity::Error,
            hold: Duration::ZERO,
        };
        let sequence = report(&Changes {
            raised: vec![long],
            resolved: Vec::new(),
        });
        assert!(sequence.len() <= 4096, "{}", sequence.len());
    }

    #[test]
    fn record_ids_stay_valid_and_distinct_after_cleaning() {
        let valid = |id: &str| {
            id.len() <= 128
                && id.split('/').all(|segment| {
                    (1..=32).contains(&segment.len())
                        && segment
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "_.+-".contains(c))
                })
        };
        let ids = [
            "agent/Felipe's Mac/s1",
            "agent/Felipe-s Mac/s1",
            "agent/mac/0b7a3c1e-6d2f-4e1a-9c55-2f8e1d0a7b44",
            "agent/mac/0b7a3c1e-6d2f-4e1a-9c55-2f8e1d0a7b45",
            "machine//lost",
        ]
        .map(record_id);
        for id in &ids {
            assert!(valid(id), "{id}");
        }
        let distinct: HashSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len(), "{ids:?}");
        assert_eq!(record_id("plan/ClaudeCode/5h"), "plan/ClaudeCode/5h");
    }

    #[test]
    fn a_held_alert_raises_after_its_hold_and_clears_only_after_it_stays_away() {
        let start = Instant::now();
        let mut alerts = Alerts::in_memory(start);
        let lost = condition("machine/nas/lost", LOST_FOR);
        assert_eq!(
            alerts.update(vec![lost.clone()], start, 0),
            Changes::default()
        );
        let raised_at = start + LOST_FOR;
        assert_eq!(
            alerts.update(vec![lost.clone()], raised_at, 30).raised,
            vec![lost.clone()]
        );
        let back = raised_at + Duration::from_secs(1);
        assert_eq!(alerts.update(Vec::new(), back, 31), Changes::default());
        assert_eq!(
            alerts.update(vec![lost.clone()], back + CLEAR_AFTER / 2, 45),
            Changes::default()
        );
        let gone = back + CLEAR_AFTER;
        assert_eq!(alerts.update(Vec::new(), gone, 60), Changes::default());
        let resolved = alerts.update(Vec::new(), gone + CLEAR_AFTER, 90);
        assert_eq!(resolved.resolved, vec![lost.key]);
        assert_eq!(events(&alerts), vec![Event::Resolved, Event::Raised]);
    }

    #[test]
    fn an_alert_without_a_hold_resolves_at_once_and_a_changed_text_is_reported_again() {
        let start = Instant::now();
        let mut alerts = Alerts::in_memory(start);
        let question = Condition {
            severity: Severity::Question,
            ..condition("agent/mac/s1", Duration::ZERO)
        };
        alerts.update(vec![question.clone()], start, 0);
        let permission = Condition {
            severity: Severity::Permission,
            text: "s1 needs you · wants to run Bash".into(),
            ..question
        };
        let changed = alerts.update(vec![permission.clone()], start, 1);
        assert_eq!(changed.raised, vec![permission.clone()]);
        assert_eq!(alerts.history().count(), 1);
        assert_eq!(
            alerts.update(Vec::new(), start, 2).resolved,
            vec![permission.key]
        );
    }

    #[test]
    fn a_condition_that_clears_before_its_hold_never_alerts() {
        let start = Instant::now();
        let mut alerts = Alerts::in_memory(start);
        let blip = condition("machine/nas/memory", CRITICAL_FOR);
        alerts.update(vec![blip.clone()], start, 0);
        alerts.update(Vec::new(), start + CRITICAL_FOR / 2, 30);
        assert!(
            alerts
                .update(vec![blip], start + CRITICAL_FOR, 60)
                .raised
                .is_empty()
        );
        assert_eq!(alerts.history().count(), 0);
    }

    #[test]
    fn a_plan_alerts_only_once_it_is_nearly_used_up_whatever_its_pace() {
        let plan = |agent, used| Plan {
            agent,
            windows: vec![crate::proto::Window {
                minutes: crate::proto::ONE_WEEK,
                used,
                resets_at: 1,
            }],
            as_of: 0,
        };
        let (claude, codex) = (
            plan(crate::proto::Agent::ClaudeCode, 8_600),
            plan(crate::proto::Agent::Codex, 9_100),
        );
        let found = plan_conditions(&[&claude, &codex]);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].text.starts_with("Codex") && found[0].text.contains("91% used"),
            "{}",
            found[0].text
        );
        assert!(
            found[0]
                .text
                .ends_with("Start the next session in Claude Code")
        );
    }

    #[test]
    fn a_dismissed_alert_stays_quiet_until_its_cause_clears_and_returns() {
        let path = temp("dismiss");
        let start = Instant::now();
        let plan = condition("plan/Codex/30d", Duration::ZERO);
        let mut alerts = Alerts::open(path.clone(), start).unwrap();
        alerts.update(vec![plan.clone()], start, 10);
        assert_eq!(alerts.active().count(), 1);
        dismiss_open(&path, 20).unwrap();
        let mut changed = plan.clone();
        changed.text = "Codex 30d plan 91% used".into();
        let dismissed = alerts.update(vec![changed.clone()], start, 30);
        assert_eq!(dismissed.resolved, vec![plan.key.clone()]);
        assert!(dismissed.raised.is_empty());
        assert_eq!(alerts.active().count(), 0);
        let mut reopened = Alerts::open(path.clone(), start).unwrap();
        assert_eq!(
            reopened.update(vec![changed.clone()], start, 40),
            Changes::default()
        );
        assert_eq!(reopened.active().count(), 0);
        reopened.update(Vec::new(), start, 50);
        let back = reopened.update(vec![changed.clone()], start, 60);
        assert_eq!(back.raised, vec![changed]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_dismissal_that_outlived_its_cause_is_closed_so_the_alert_can_come_back() {
        let path = temp("dismiss-gone");
        let start = Instant::now();
        let plan = condition("plan/ClaudeCode/5h", Duration::ZERO);
        Alerts::open(path.clone(), start)
            .unwrap()
            .update(vec![plan.clone()], start, 10);
        dismiss_open(&path, 20).unwrap();
        let mut restarted = Alerts::open(path.clone(), start).unwrap();
        restarted.update(Vec::new(), start + INHERIT_GRACE, 30);
        let (open, dismissed) = still_open(&read_history(&path).unwrap());
        assert!(open.is_empty() && dismissed.is_empty());
        let mut later = Alerts::open(path.clone(), start).unwrap();
        assert_eq!(
            later.update(vec![plan.clone()], start, 40).raised,
            vec![plan]
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn history_survives_a_restart_without_repeating_alerts_that_are_still_true() {
        let path = temp("restart");
        let start = Instant::now();
        let waiting = condition("agent/mac/s1", Duration::ZERO);
        let plan = condition("plan/Codex/5h", Duration::ZERO);
        Alerts::open(path.clone(), start).unwrap().update(
            vec![waiting.clone(), plan.clone()],
            start,
            10,
        );
        let mut reopened = Alerts::open(path.clone(), start).unwrap();
        assert_eq!(reopened.history().count(), 2);
        let changes = reopened.update(vec![waiting.clone()], start + Duration::from_secs(1), 20);
        assert_eq!(changes.raised, vec![waiting.clone()]);
        assert_eq!(reopened.history().count(), 2);
        let settled = reopened.update(vec![waiting], start + INHERIT_GRACE, 50);
        assert_eq!(settled.resolved, vec![plan.key]);
        let newest = reopened.history().next().unwrap();
        assert_eq!((newest.event, newest.at), (Event::Resolved, 50));
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_machine_still_down_after_a_restart_stays_alerted_until_it_answers() {
        let path = temp("down");
        let start = Instant::now();
        let lost = Condition {
            machine: "nas".into(),
            ..condition("machine/nas/lost", Duration::ZERO)
        };
        Alerts::open(path.clone(), start)
            .unwrap()
            .update(vec![lost.clone()], start, 10);
        let mut model = Model::new(2015, start, Alerts::open(path.clone(), start).unwrap());
        let later = start + INHERIT_GRACE * 2;
        for (at, secs) in [(start, 20), (later, 80)] {
            let found = conditions(&model, at, secs);
            assert_eq!(found.len(), 1, "{found:?}");
            let changes = model.alerts.update(found, at, secs);
            assert!(changes.resolved.is_empty());
        }
        assert_eq!(model.alerts.active().count(), 1);
        assert_eq!(model.alerts.history().count(), 1);
        let nas = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 9));
        model.links.insert(nas, Link::connecting());
        model.apply(
            nas,
            LinkUpdate::Welcome(Box::new(Welcome {
                interval_ms: 1000,
                identity: Identity {
                    hostname: "nas".into(),
                    tailscale_node: Some("nas".into()),
                    os: String::new(),
                    arch: String::new(),
                    tools: Vec::new(),
                    cpu_model: String::new(),
                    core_kinds: Vec::new(),
                    reaches: Vec::new(),
                },
            })),
            later,
        );
        let answering = conditions(&model, later, 90);
        assert!(answering.is_empty(), "{answering:?}");
        model.alerts.update(answering, later, 90);
        let resolved = model.alerts.update(Vec::new(), later + CLEAR_AFTER, 120);
        assert_eq!(resolved.resolved, vec![lost.key]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_file_is_private_survives_torn_lines_and_stays_bounded() {
        let path = temp("bounded");
        let start = Instant::now();
        let mut alerts = Alerts::open(path.clone(), start).unwrap();
        for index in 0..(REWRITE_AT as u64) {
            let flap = condition(&format!("agent/m{index}/s"), Duration::ZERO);
            alerts.update(vec![flap], start, index);
            alerts.update(Vec::new(), start, index);
        }
        let text = fs::read(&path).unwrap();
        assert!(text.split(|byte| *byte == b'\n').count() <= REWRITE_AT + 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            FILE_MODE
        );
        let mut torn = b"{\"at\":1,\"text\":\"\xc2".to_vec();
        torn.extend_from_slice(b"\nnot json\n");
        torn.extend_from_slice(&text);
        fs::write(&path, torn).unwrap();
        let reopened = Alerts::open(path.clone(), start).unwrap();
        assert_eq!(reopened.history().count(), KEPT);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_failed_save_stays_visible() {
        let path = temp("unwritable");
        let dir = path.parent().unwrap().to_path_buf();
        fs::create_dir_all(&dir).unwrap();
        let start = Instant::now();
        let mut alerts = Alerts::open(path, start).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        alerts.update(vec![condition("agent/mac/s1", Duration::ZERO)], start, 1);
        let failed = alerts.problem().is_some();
        alerts.update(vec![condition("agent/mac/s1", Duration::ZERO)], start, 2);
        let still = alerts.problem().is_some();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(failed && still);
    }
}
