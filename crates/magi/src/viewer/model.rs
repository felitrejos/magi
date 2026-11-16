use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use super::alerts::Alerts;
use super::launcher::{self, Listing, Menu, Place};
use super::namer;
use super::projects::{Projects, Resolved};
use super::units;
use crate::proto::{
    self, Agent, AgentSession, CodexModel, FULL, Forward, ForwardState, Identity, Plan, Reply,
    Sample, SessionState, SharedProjects, Welcome,
};
use crate::tailscale::Peer;

pub const HISTORY: usize = 300;
const SEARCH_GRACE: Duration = Duration::from_secs(8);
const FLASH_FOR: Duration = Duration::from_secs(5);
const MAX_NEW_PER_ASK: usize = 20;
const FOLLOW_FOR: Duration = Duration::from_secs(300);
const AWAIT_FOR: Duration = Duration::from_secs(60);
const HEAVY_SESSION_MILLICORES: u32 = 1_000;
const HEAVY_SESSION_BYTES: u64 = 1 << 30;
const SUSTAINED_SAMPLES: usize = 60;
const BUSY_CPU: f32 = 0.8;
const BUSY_MEMORY: f32 = 0.9;
const BUSY_LOAD_PER_CORE: f32 = 1.0;
const ROOM_CPU: f32 = 0.5;
const ROOM_MEMORY: f32 = 0.75;
const ROOM_LOAD_PER_CORE: f32 = 0.7;
const CPU_SMOOTHING_SAMPLES: usize = 5;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Topology {
    pub self_node: Option<String>,
    pub peers: Vec<Peer>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LinkUpdate {
    Welcome(Box<Welcome>),
    Sample(Box<Sample>),
    Lost(String),
    Rejected(String),
    Reply(Reply),
    Folders(Vec<String>),
    Models(Vec<CodexModel>),
    Projects(SharedProjects),
    Shown(crate::proto::Shown),
    Screen(crate::proto::Screen),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Connecting,
    Live,
    Lost { since: Instant, reason: String },
    Rejected(String),
}

pub struct Link {
    pub state: LinkState,
    pub machine: Option<Machine>,
}

impl Link {
    pub fn connecting() -> Self {
        Self {
            state: LinkState::Connecting,
            machine: None,
        }
    }
}

pub struct Machine {
    pub identity: Identity,
    pub interval: Duration,
    pub latest: Option<Sample>,
    pub cpu: History,
    pub memory: History,
    pub rx: History,
    pub tx: History,
    pub disk_read: History,
    pub disk_write: History,
    pub core_history: Vec<History>,
}

impl Machine {
    fn new(welcome: Welcome) -> Self {
        Self {
            identity: welcome.identity,
            interval: Duration::from_millis(u64::from(welcome.interval_ms)),
            latest: None,
            cpu: History::default(),
            memory: History::default(),
            rx: History::default(),
            tx: History::default(),
            disk_read: History::default(),
            disk_write: History::default(),
            core_history: Vec::new(),
        }
    }

    pub fn name(&self) -> String {
        self.identity.tailscale_node.clone().unwrap_or_else(|| {
            self.identity
                .hostname
                .split_once('.')
                .map_or(self.identity.hostname.as_str(), |(host, _)| host)
                .to_ascii_lowercase()
        })
    }

    pub fn smoothed_cpu(&self) -> f32 {
        self.cpu.recent_mean(CPU_SMOOTHING_SAMPLES)
    }

    fn sustained_cpu(&self) -> f32 {
        self.cpu.recent_mean(SUSTAINED_SAMPLES)
    }

    fn load_per_core(&self) -> f32 {
        self.latest.as_ref().map_or(0.0, |sample| {
            sample.load[1] / sample.cores.len().max(1) as f32
        })
    }

    pub fn pressure(&self) -> Option<Pressure> {
        let sample = self.latest.as_ref()?;
        let cpu = self.sustained_cpu();
        let memory = self.memory_fraction();
        if memory >= BUSY_MEMORY {
            Some(Pressure::Memory(memory))
        } else if cpu >= BUSY_CPU {
            Some(Pressure::Cpu(cpu))
        } else if self.load_per_core() >= BUSY_LOAD_PER_CORE {
            Some(Pressure::Load {
                load: sample.load[1],
                cores: sample.cores.len(),
            })
        } else {
            None
        }
    }

    fn crowding(&self) -> f32 {
        self.sustained_cpu()
            .max(self.memory_fraction())
            .max(self.load_per_core())
    }

    fn has_room(&self) -> bool {
        self.sustained_cpu() < ROOM_CPU
            && self.memory_fraction() < ROOM_MEMORY
            && self.load_per_core() < ROOM_LOAD_PER_CORE
    }

    pub fn memory_fraction(&self) -> f32 {
        self.latest.as_ref().map_or(0.0, |sample| {
            fraction(sample.memory.used, sample.memory.total)
        })
    }

    pub fn fullest_filesystem(&self) -> f32 {
        self.latest
            .as_ref()
            .and_then(|sample| {
                sample
                    .filesystems
                    .iter()
                    .map(|fs| fraction(fs.used, fs.total))
                    .reduce(f32::max)
            })
            .unwrap_or(0.0)
    }
}

#[derive(Default)]
pub struct History {
    values: VecDeque<f32>,
}

impl History {
    fn push(&mut self, value: f32) {
        if self.values.len() == HISTORY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn tail(&self, count: usize) -> Vec<f32> {
        self.values
            .iter()
            .skip(self.values.len().saturating_sub(count))
            .copied()
            .collect()
    }

    pub fn recent_mean(&self, count: usize) -> f32 {
        let recent = self.tail(count);
        if recent.is_empty() {
            0.0
        } else {
            recent.iter().sum::<f32>() / recent.len() as f32
        }
    }
}

pub fn fraction(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64) as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Endpoint {
    Local,
    Tailnet(Ipv4Addr),
}

pub enum Presence<'a> {
    Agent(&'a Link),
    Searching,
    NoAgent,
}

pub struct Row<'a> {
    pub endpoint: Endpoint,
    pub name: String,
    pub here: bool,
    pub presence: Presence<'a>,
}

impl<'a> Row<'a> {
    pub fn machine(&self) -> Option<&'a Machine> {
        match self.presence {
            Presence::Agent(link) => link.machine.as_ref(),
            _ => None,
        }
    }

    pub fn state(&self) -> Option<&'a LinkState> {
        match self.presence {
            Presence::Agent(link) => Some(&link.state),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Level {
    Normal,
    High,
    Critical,
}

impl Level {
    fn grade(value: f32, high: f32, critical: f32) -> Self {
        if value >= critical {
            Level::Critical
        } else if value >= high {
            Level::High
        } else {
            Level::Normal
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Subsystem {
    Cpu,
    Memory,
    Storage,
}

impl Subsystem {
    pub fn name(self) -> &'static str {
        match self {
            Subsystem::Cpu => "cpu",
            Subsystem::Memory => "memory",
            Subsystem::Storage => "storage",
        }
    }
}

pub fn grade(subsystem: Subsystem, value: f32) -> Level {
    match subsystem {
        Subsystem::Cpu => Level::grade(value, 0.70, 0.90),
        Subsystem::Memory => Level::grade(value, 0.80, 0.92),
        Subsystem::Storage => Level::grade(value, 0.85, 0.95),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Machine(String),
    Agent(String),
}

pub struct AgentRow<'a> {
    pub endpoint: Endpoint,
    pub machine: String,
    pub session: &'a AgentSession,
    pub live: bool,
}

type ProjectGroup<'a> = (String, Option<String>, Vec<AgentRow<'a>>);

fn add_to_project<'a>(
    groups: &mut Vec<ProjectGroup<'a>>,
    projects: &Projects,
    agent: AgentRow<'a>,
) {
    let Resolved { key, name } = projects.resolve(&agent.session.project);
    match groups.iter_mut().find(|(known, ..)| *known == key) {
        Some((_, named, agents)) => {
            if named.is_none() {
                *named = name;
            }
            agents.push(agent);
        }
        None => groups.push((key, name, vec![agent])),
    }
}

fn project_name(name: Option<String>, agents: &[AgentRow<'_>]) -> String {
    name.unwrap_or_else(|| {
        agents
            .iter()
            .map(|agent| agent.session.project.as_str())
            .min_by_key(|name| (name.len(), *name))
            .unwrap_or_default()
            .to_string()
    })
}

fn urgency(agent: &AgentRow<'_>) -> (u8, i64) {
    let rank = match agent.session.state {
        SessionState::Waiting(_) => 0,
        SessionState::Working { .. } => 1,
        SessionState::Idle => 2,
    };
    let recency = match agent.session.state {
        SessionState::Waiting(_) => -i64::from(agent.session.state_secs),
        _ => i64::from(agent.session.state_secs),
    };
    (if agent.live { rank } else { 3 }, recency)
}

pub struct Model {
    pub topology: Topology,
    pub links: HashMap<Endpoint, Link>,
    pub selected: usize,
    pub port: u16,
    pub projects: Projects,
    pub naming: Option<Naming>,
    pub namer_busy: bool,
    pub machine_page: Option<String>,
    pub namer_asked_at: Option<Instant>,
    pub alerts: Alerts,
    focus: Option<Focus>,
    started: Instant,
    flash: Option<Flash>,
    pub opening: bool,
    pub pending_move: Option<PendingMove>,
    following: Option<Following>,
    pub folders: BTreeMap<Endpoint, Folders>,
    pub codex_models: BTreeMap<Endpoint, Vec<CodexModel>>,
    pub cell_size: Option<(u32, u32)>,
    pub shared_sent: u64,
    awaiting: Option<Awaiting>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Renaming {
    Machine(String),
    Repo(String),
    Project { key: String, name: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Naming {
    pub what: Renaming,
    pub text: String,
}

impl Naming {
    pub fn completion<'a>(&self, names: &'a [String]) -> Option<&'a str> {
        if self.text.is_empty() {
            return None;
        }
        let typed = self.text.to_lowercase();
        names
            .iter()
            .find(|name| name.to_lowercase().starts_with(&typed) && name.len() > self.text.len())
            .map(|name| name.as_str())
    }
}

struct Flash {
    text: String,
    good: bool,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingMove {
    pub endpoint: Endpoint,
    pub session: String,
    pub title: String,
    pub from: String,
    pub to: String,
}

pub fn machine_tag<'a>(name: &'a str, names: &[&str]) -> &'a str {
    let tag = first_word(name);
    let shared = names
        .iter()
        .filter(|other| first_word(other) == tag)
        .count()
        > 1;
    if shared { name } else { tag }
}

fn first_word(name: &str) -> &str {
    name.split(['-', '.', '_', ' ']).next().unwrap_or(name)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Folders {
    Asking,
    Listed(Vec<String>),
    Failed(String),
}

#[derive(Clone)]
struct Awaiting {
    endpoint: Endpoint,
    project: String,
    known: Vec<String>,
    since: Instant,
}

#[derive(Clone)]
struct Following {
    session: String,
    from: Endpoint,
    to: String,
    since: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pressure {
    Cpu(f32),
    Memory(f32),
    Load { load: f32, cores: usize },
}

impl Pressure {
    fn weighs_on(self, usage: proto::Usage) -> bool {
        match self {
            Self::Memory(_) => usage.memory >= HEAVY_SESSION_BYTES,
            Self::Cpu(_) | Self::Load { .. } => {
                usage.cpu.is_some_and(|cpu| cpu >= HEAVY_SESSION_MILLICORES)
            }
        }
    }

    pub fn short(self) -> String {
        match self {
            Self::Cpu(cpu) => format!("CPU {} for a minute", units::percent(cpu).trim()),
            Self::Memory(memory) => format!("memory {} full", units::percent(memory).trim()),
            Self::Load { load, cores } => format!("load {load:.0} on {cores} cores"),
        }
    }

    pub fn describe(self, machine: &str) -> String {
        match self {
            Self::Cpu(cpu) => format!(
                "{machine}'s CPU has averaged {} for a minute",
                units::percent(cpu).trim()
            ),
            Self::Memory(memory) => {
                format!(
                    "{machine}'s memory is {} full",
                    units::percent(memory).trim()
                )
            }
            Self::Load { load, cores } => {
                format!("{machine} has a load of {load:.0} on {cores} cores")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoveTarget {
    pub machine: String,
    pub cpu: f32,
    pub memory: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MoveSuggestion {
    pub pressure: Pressure,
    pub target: MoveTarget,
}

fn same_machine(name: &str, machine: &str) -> bool {
    let short = |name: &str| name.split('.').next().unwrap_or(name).to_string();
    !name.is_empty() && short(name).eq_ignore_ascii_case(&short(machine))
}

impl Model {
    pub fn new(port: u16, now: Instant, alerts: Alerts) -> Self {
        Self {
            topology: Topology::default(),
            links: HashMap::new(),
            selected: 0,
            port,
            projects: Projects::default(),
            naming: None,
            namer_busy: false,
            machine_page: None,
            namer_asked_at: None,
            alerts,
            focus: None,
            started: now,
            flash: None,
            opening: false,
            pending_move: None,
            following: None,
            folders: BTreeMap::new(),
            codex_models: BTreeMap::new(),
            cell_size: None,
            shared_sent: 0,
            awaiting: None,
        }
    }

    pub fn apply(&mut self, endpoint: Endpoint, update: LinkUpdate, now: Instant) {
        let update = match update {
            LinkUpdate::Reply(reply) => {
                if !reply.ok {
                    if self
                        .awaiting
                        .as_ref()
                        .is_some_and(|awaiting| awaiting.endpoint == endpoint)
                    {
                        self.awaiting = None;
                    }
                    if let Some(folders @ Folders::Asking) = self.folders.get_mut(&endpoint) {
                        *folders = Folders::Failed(reply.text.clone());
                    }
                }
                return self.flash(reply.text, reply.ok, now);
            }
            LinkUpdate::Folders(folders) => {
                if let Some(asked) = self.folders.get_mut(&endpoint) {
                    *asked = Folders::Listed(folders);
                }
                return;
            }
            LinkUpdate::Projects(shared) => {
                let updated = shared.updated_ms;
                match self.projects.adopt(shared) {
                    Ok(true) => self.shared_sent = updated,
                    Ok(false) => {}
                    Err(error) => {
                        self.flash(format!("couldn't save projects: {error}"), false, now)
                    }
                }
                return;
            }
            LinkUpdate::Models(models) => {
                if self.folders.contains_key(&endpoint) {
                    self.codex_models.insert(endpoint, models);
                }
                return;
            }
            LinkUpdate::Screen(_) => return,
            update => update,
        };
        if matches!(update, LinkUpdate::Welcome(_)) {
            self.shared_sent = 0;
        }
        let Some(link) = self.links.get_mut(&endpoint) else {
            return;
        };
        match update {
            LinkUpdate::Welcome(welcome) => {
                link.state = LinkState::Live;
                match &mut link.machine {
                    Some(machine) if machine.identity.hostname == welcome.identity.hostname => {
                        machine.interval = Duration::from_millis(u64::from(welcome.interval_ms));
                        machine.identity = welcome.identity;
                    }
                    _ => link.machine = Some(Machine::new(*welcome)),
                }
            }
            LinkUpdate::Sample(sample) => {
                let Some(machine) = &mut link.machine else {
                    return;
                };
                link.state = LinkState::Live;
                machine.cpu.push(f32::from(sample.cpu) / f32::from(FULL));
                machine
                    .memory
                    .push(fraction(sample.memory.used, sample.memory.total));
                machine.rx.push(sample.network.rx_per_sec as f32);
                machine.tx.push(sample.network.tx_per_sec as f32);
                machine.disk_read.push(sample.disk.read_per_sec as f32);
                machine.disk_write.push(sample.disk.write_per_sec as f32);
                machine
                    .core_history
                    .resize_with(sample.cores.len(), History::default);
                for (history, core) in machine.core_history.iter_mut().zip(&sample.cores) {
                    history.push(f32::from(*core) / f32::from(FULL));
                }
                machine.latest = Some(*sample);
            }
            LinkUpdate::Lost(reason) => {
                let since = match &link.state {
                    LinkState::Lost { since, .. } => *since,
                    _ => now,
                };
                link.state = LinkState::Lost { since, reason };
            }
            LinkUpdate::Rejected(reason) => link.state = LinkState::Rejected(reason),
            LinkUpdate::Reply(_)
            | LinkUpdate::Folders(_)
            | LinkUpdate::Models(_)
            | LinkUpdate::Projects(_)
            | LinkUpdate::Screen(_)
            | LinkUpdate::Shown(_) => {
                unreachable!("replies, screens and folders are handled before a link is looked up")
            }
        }
    }

    pub fn known_agents(&self) -> Vec<Endpoint> {
        self.links
            .iter()
            .filter(|(_, link)| link.machine.is_some())
            .map(|(endpoint, _)| *endpoint)
            .collect()
    }

    pub fn rows(&self, now: Instant) -> Vec<Row<'_>> {
        let searching = now.duration_since(self.started) < SEARCH_GRACE;
        let local = self.links.get(&Endpoint::Local);
        let local_name = local
            .and_then(|link| link.machine.as_ref())
            .map(Machine::name)
            .or_else(|| self.topology.self_node.clone())
            .unwrap_or_else(|| "this machine".into());
        let local_presence = match local {
            Some(link)
                if link.machine.is_some() || matches!(link.state, LinkState::Rejected(_)) =>
            {
                Presence::Agent(link)
            }
            _ if searching => Presence::Searching,
            _ => Presence::NoAgent,
        };
        let mut rows = vec![Row {
            endpoint: Endpoint::Local,
            name: local_name,
            here: true,
            presence: local_presence,
        }];

        let mut others: Vec<Row<'_>> = self
            .links
            .iter()
            .filter(|(endpoint, _)| **endpoint != Endpoint::Local)
            .filter_map(|(endpoint, link)| {
                let name = match (&link.machine, &link.state) {
                    (Some(machine), _) => machine.name(),
                    (None, LinkState::Rejected(_)) => self.peer_name(*endpoint)?,
                    (None, _) => return None,
                };
                Some(Row {
                    endpoint: *endpoint,
                    name,
                    here: false,
                    presence: Presence::Agent(link),
                })
            })
            .collect();
        others.sort_by(|a, b| a.name.cmp(&b.name));
        rows.extend(others);
        rows
    }

    fn peer_name(&self, endpoint: Endpoint) -> Option<String> {
        self.topology
            .peers
            .iter()
            .find(|peer| Endpoint::Tailnet(peer.ipv4) == endpoint)
            .map(|peer| peer.node.clone())
    }

    pub fn agents(&self, now: Instant) -> Vec<AgentRow<'_>> {
        let mut agents: Vec<AgentRow<'_>> = self
            .rows(now)
            .into_iter()
            .filter_map(|row| {
                let live = row.state() == Some(&LinkState::Live);
                let machine = row.machine()?;
                let sample = machine.latest.as_ref()?;
                let endpoint = row.endpoint;
                Some(sample.sessions.iter().map(move |session| AgentRow {
                    endpoint,
                    machine: row.name.clone(),
                    session,
                    live,
                }))
            })
            .flatten()
            .collect();
        agents.sort_by(|a, b| {
            urgency(a)
                .cmp(&urgency(b))
                .then(a.machine.cmp(&b.machine))
                .then(a.session.title.cmp(&b.session.title))
        });
        agents
    }

    pub fn plans(&self, now: Instant) -> Vec<&Plan> {
        let mut plans: Vec<&Plan> = Vec::new();
        for plan in self
            .rows(now)
            .into_iter()
            .filter(|row| row.state() == Some(&LinkState::Live))
            .filter_map(|row| row.machine()?.latest.as_ref())
            .flat_map(|sample| &sample.plans)
        {
            match plans.iter_mut().find(|known| known.agent == plan.agent) {
                Some(known) if known.as_of < plan.as_of => *known = plan,
                Some(_) => {}
                None => plans.push(plan),
            }
        }
        plans.sort_by_key(|plan| plan.agent);
        plans
    }

    pub fn orphaned_forwards(&self, machine: &str, now: Instant) -> Vec<Forward> {
        let rows = self.rows(now);
        let agents = self.agents(now);
        let Some(sample) = rows
            .iter()
            .find(|row| row.name == machine)
            .and_then(Row::machine)
            .and_then(|machine| machine.latest.as_ref())
        else {
            return Vec::new();
        };
        sample
            .forwards
            .iter()
            .filter(|forward| {
                let host = rows.iter().find(|row| {
                    same_machine(&forward.host, &row.name) || same_machine(&forward.ssh, &row.name)
                });
                host.is_some_and(|host| {
                    host.state() == Some(&LinkState::Live)
                        && host
                            .machine()
                            .is_some_and(|machine| machine.latest.is_some())
                        && !agents.iter().any(|agent| {
                            agent.live
                                && agent.machine == host.name
                                && agent.session.id == forward.session
                        })
                })
            })
            .cloned()
            .collect()
    }

    pub fn forwards_to(
        &self,
        session: &str,
        machine: &str,
        now: Instant,
    ) -> Vec<(u16, ForwardState)> {
        let names = |name: &str| same_machine(name, machine);
        let mut ports: Vec<(u16, ForwardState)> = self
            .rows(now)
            .into_iter()
            .filter(|row| row.state() == Some(&LinkState::Live))
            .filter_map(|row| row.machine()?.latest.as_ref())
            .flat_map(|sample| &sample.forwards)
            .filter(|forward| {
                forward.session == session && (names(&forward.host) || names(&forward.ssh))
            })
            .flat_map(|forward| forward.ports.iter().map(|port| (*port, forward.state)))
            .collect();
        ports.sort_unstable();
        ports.dedup_by_key(|(port, _)| *port);
        ports
    }

    pub fn notices(&self, now: Instant) -> Vec<String> {
        self.rows(now)
            .into_iter()
            .filter(|row| row.state() == Some(&LinkState::Live))
            .flat_map(|row| {
                let notices = row
                    .machine()
                    .and_then(|machine| machine.latest.as_ref())
                    .map_or(&[][..], |sample| &sample.notices[..]);
                notices
                    .iter()
                    .map(move |notice| format!("{}: {notice}", row.name))
            })
            .collect()
    }

    pub fn flash(&mut self, text: String, good: bool, now: Instant) {
        self.flash = Some(Flash {
            text,
            good,
            at: now,
        });
    }

    pub fn flashed(&self, now: Instant) -> Option<(&str, bool)> {
        self.flash
            .as_ref()
            .filter(|flash| now.duration_since(flash.at) < FLASH_FOR)
            .map(|flash| (flash.text.as_str(), flash.good))
    }

    pub fn move_target(&self, agent: &AgentRow<'_>, now: Instant) -> Option<MoveTarget> {
        self.rows(now)
            .into_iter()
            .filter(|row| row.name != agent.machine && row.state() == Some(&LinkState::Live))
            .filter_map(|row| {
                let machine = row.machine()?;
                machine.latest.as_ref()?;
                Some((row.name.clone(), machine))
            })
            .min_by(|(_, a), (_, b)| a.crowding().total_cmp(&b.crowding()))
            .map(|(name, machine)| MoveTarget {
                machine: name,
                cpu: machine.sustained_cpu(),
                memory: machine.memory_fraction(),
            })
    }

    pub fn move_suggestion(&self, agent: &AgentRow<'_>, now: Instant) -> Option<MoveSuggestion> {
        if agent.session.agent != Agent::ClaudeCode || !agent.live {
            return None;
        }
        let rows = self.rows(now);
        let pressure = rows
            .iter()
            .find(|row| row.name == agent.machine)
            .and_then(Row::machine)?
            .pressure()?;
        if !agent
            .session
            .usage
            .is_some_and(|usage| pressure.weighs_on(usage))
        {
            return None;
        }
        let target = self.move_target(agent, now)?;
        let roomy = rows
            .iter()
            .find(|row| row.name == target.machine)
            .and_then(Row::machine)
            .is_some_and(Machine::has_room);
        roomy.then_some(MoveSuggestion { pressure, target })
    }

    pub fn selected_agent(&self, now: Instant) -> Option<AgentRow<'_>> {
        let Some(Focus::Agent(id)) = self.selection(now) else {
            return None;
        };
        self.agents(now)
            .into_iter()
            .find(|agent| agent.session.id == id)
    }

    pub fn since_start(&self, now: Instant) -> Duration {
        now.duration_since(self.started)
    }

    pub fn repos(&self, now: Instant) -> Vec<String> {
        let mut repos: Vec<String> = self
            .agents(now)
            .iter()
            .map(|agent| agent.session.project.clone())
            .filter(|repo| !repo.is_empty())
            .collect();
        repos.sort_by_key(|repo| repo.to_lowercase());
        repos.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        repos
    }

    pub fn naming_request(&self, now: Instant) -> Option<(Vec<String>, namer::Request)> {
        let new: Vec<String> = self
            .repos(now)
            .into_iter()
            .filter(|repo| self.projects.is_new(repo))
            .take(MAX_NEW_PER_ASK)
            .collect();
        if new.is_empty() {
            return None;
        }
        let agents = self.agents(now);
        let mut projects: Vec<ProjectGroup<'_>> = Vec::new();
        for agent in self
            .agents(now)
            .into_iter()
            .filter(|agent| !new.contains(&agent.session.project))
        {
            add_to_project(&mut projects, &self.projects, agent);
        }
        let request = namer::Request {
            projects: projects
                .into_iter()
                .map(|(_, name, members)| {
                    let mut repos: Vec<String> = members
                        .iter()
                        .map(|agent| agent.session.project.clone())
                        .collect();
                    repos.dedup();
                    namer::Project {
                        name: project_name(name, &members),
                        repos,
                    }
                })
                .collect(),
            new: new
                .iter()
                .map(|repo| {
                    let titles = agents
                        .iter()
                        .filter(|agent| agent.session.project == *repo)
                        .map(|agent| agent.session.title.clone());
                    namer::new_repo(repo, titles)
                })
                .collect(),
        };
        Some((new, request))
    }

    pub fn project_names(&self, now: Instant) -> Vec<String> {
        let mut groups: Vec<ProjectGroup<'_>> = Vec::new();
        for agent in self.agents(now) {
            add_to_project(&mut groups, &self.projects, agent);
        }
        let mut names: Vec<String> = groups
            .into_iter()
            .map(|(_, name, agents)| project_name(name, &agents))
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names
    }

    pub fn project_of(&self, repo: &str, now: Instant) -> (String, String) {
        let Resolved { key, name } = self.projects.resolve(repo);
        let name = name.unwrap_or_else(|| {
            self.agents(now)
                .iter()
                .map(|agent| agent.session.project.as_str())
                .filter(|other| self.projects.resolve(other).key == key)
                .min_by_key(|name| (name.len(), *name))
                .unwrap_or(repo)
                .to_string()
        });
        (key, name)
    }

    pub fn board(&self, now: Instant) -> Vec<Focus> {
        let agents = self.agents(now);
        let waiting = |agent: &AgentRow<'_>| {
            agent.live && matches!(agent.session.state, SessionState::Waiting(_))
        };
        let mut needs: Vec<&AgentRow<'_>> = agents.iter().filter(|agent| waiting(agent)).collect();
        needs.sort_by_key(|agent| urgency(agent));
        let mut board: Vec<Focus> = needs
            .iter()
            .map(|agent| Focus::Agent(agent.session.id.clone()))
            .collect();
        for row in self.rows(now) {
            board.push(Focus::Machine(row.name.clone()));
            let mut mine: Vec<&AgentRow<'_>> = agents
                .iter()
                .filter(|agent| agent.machine == row.name && !waiting(agent))
                .collect();
            mine.sort_by_key(|agent| urgency(agent));
            board.extend(
                mine.iter()
                    .map(|agent| Focus::Agent(agent.session.id.clone())),
            );
        }
        board
    }

    pub fn selection(&self, now: Instant) -> Option<Focus> {
        self.board(now).get(self.selected).cloned()
    }

    pub fn move_selection(&mut self, delta: isize, now: Instant) {
        let count = self.board(now).len();
        self.select(delta, count);
        self.focus = self.selection(now);
    }

    pub fn machine_label(&self, name: &str, names: &[&str]) -> String {
        self.projects
            .machine_label(name)
            .map_or_else(|| machine_tag(name, names).to_string(), str::to_string)
    }

    pub fn end_launch(&mut self) {
        self.folders.clear();
        self.codex_models.clear();
    }

    pub fn menu(&self, now: Instant) -> Menu {
        let rows = self.rows(now);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        let agents = self.agents(now);
        let worked_in = |live: bool| -> Vec<String> {
            agents
                .iter()
                .filter(|agent| !live || agent.live)
                .map(|agent| agent.session.project.to_lowercase())
                .collect()
        };
        let (active, recent) = (worked_in(true), worked_in(false));
        let mut menu = Menu::default();
        for row in &rows {
            let Some(folders) = self.folders.get(&row.endpoint) else {
                continue;
            };
            let sample = row.machine().and_then(|machine| machine.latest.as_ref());
            let machine = menu.machines.len();
            menu.machines.push(Place {
                name: row.name.clone(),
                tag: self.machine_label(&row.name, &names),
                cpu: row.machine().map_or(0.0, Machine::smoothed_cpu),
                free: sample.map_or(0, |sample| {
                    sample.memory.total.saturating_sub(sample.memory.used)
                }),
                codex_models: self.codex_models.get(&row.endpoint).cloned(),
                listing: match folders {
                    Folders::Asking => Listing::Asking,
                    Folders::Listed(_) => Listing::Listed,
                    Folders::Failed(reason) => Listing::Failed(reason.clone()),
                },
            });
            let Folders::Listed(folders) = folders else {
                continue;
            };
            for dir in folders {
                let repo = dir.rsplit('/').next().unwrap_or(dir);
                let key = repo.to_lowercase();
                let folder = launcher::Folder {
                    machine,
                    dir: dir.clone(),
                };
                let Some(project) = menu.projects.iter_mut().find(|project| project.key == key)
                else {
                    let (_, group) = self.project_of(repo, now);
                    menu.projects.push(launcher::Project {
                        active: active.contains(&key),
                        recent: recent.contains(&key),
                        chat: false,
                        group: (group.to_lowercase() != key).then_some(group),
                        key,
                        name: repo.to_string(),
                        folders: vec![folder],
                    });
                    continue;
                };
                match project
                    .folders
                    .iter_mut()
                    .find(|known| known.machine == machine)
                {
                    Some(known) if dir.len() < known.dir.len() => *known = folder,
                    Some(_) => {}
                    None => project.folders.push(folder),
                }
            }
        }
        let chat = launcher::Project {
            key: String::new(),
            name: "No project".into(),
            group: Some(format!("a chat in {}", launcher::CHAT_FOLDER)),
            active: false,
            recent: false,
            chat: true,
            folders: menu
                .machines
                .iter()
                .enumerate()
                .filter(|(_, place)| place.listing == Listing::Listed)
                .map(|(machine, _)| launcher::Folder {
                    machine,
                    dir: launcher::CHAT_FOLDER.into(),
                })
                .collect(),
        };
        if !chat.folders.is_empty() {
            menu.projects.push(chat);
        }
        menu.projects.sort_by_key(|project| {
            (
                !project.chat,
                !project.active,
                !project.recent,
                project.name.to_lowercase(),
            )
        });
        menu
    }

    pub fn await_session(&mut self, endpoint: Endpoint, project: &str, now: Instant) {
        let known = self
            .agents(now)
            .into_iter()
            .map(|agent| agent.session.id.clone())
            .collect();
        self.awaiting = Some(Awaiting {
            endpoint,
            project: project.to_string(),
            known,
            since: now,
        });
    }

    fn select_opened(&mut self, now: Instant) {
        let Some(awaiting) = self.awaiting.clone() else {
            return;
        };
        let opened = self
            .agents(now)
            .iter()
            .find(|agent| {
                agent.endpoint == awaiting.endpoint
                    && agent.session.project == awaiting.project
                    && !awaiting.known.contains(&agent.session.id)
            })
            .map(|agent| agent.session.id.clone());
        if let Some(session) = opened {
            self.focus = Some(Focus::Agent(session));
            self.awaiting = None;
        } else if now.duration_since(awaiting.since) > AWAIT_FOR {
            self.awaiting = None;
        }
    }

    pub fn follow(&mut self, session: String, from: Endpoint, to: String, now: Instant) {
        self.following = Some(Following {
            session,
            from,
            to,
            since: now,
        });
    }

    fn follow_moved(&mut self, now: Instant) {
        let Some(following) = self.following.clone() else {
            return;
        };
        let found = self
            .agents(now)
            .iter()
            .find(|agent| agent.session.id == following.session)
            .map(|agent| agent.endpoint != following.from && agent.live);
        match found {
            Some(true) => {
                self.focus = Some(Focus::Agent(following.session));
                self.following = None;
            }
            Some(false) => {}
            None => self.focus = Some(Focus::Machine(following.to)),
        }
        if now.duration_since(following.since) > FOLLOW_FOR {
            self.following = None;
        }
    }

    pub fn reselect(&mut self, now: Instant) {
        self.follow_moved(now);
        self.select_opened(now);
        let board = self.board(now);
        let found = self
            .focus
            .as_ref()
            .and_then(|focus| board.iter().position(|item| item == focus));
        match found {
            Some(index) => self.selected = index,
            None => {
                if matches!(self.focus, Some(Focus::Agent(_))) {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.select(0, board.len());
                self.focus = board.get(self.selected).cloned();
            }
        }
        if self
            .machine_page
            .as_ref()
            .is_some_and(|name| !self.rows(now).iter().any(|row| row.name == *name))
        {
            self.machine_page = None;
        }
    }

    pub fn select(&mut self, delta: isize, row_count: usize) {
        if row_count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(row_count - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: Endpoint = Endpoint::Local;
    const HOMELAB: Endpoint = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 7));
    const NAS: Endpoint = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 9));
    const PHONE: Endpoint = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 8));

    fn welcome(node: &str) -> LinkUpdate {
        LinkUpdate::Welcome(Box::new(Welcome {
            interval_ms: 1000,
            identity: Identity {
                hostname: node.into(),
                tailscale_node: Some(node.into()),
                os: "test".into(),
                kernel: "test".into(),
                arch: "test".into(),
                cpu_model: "test".into(),
                core_kinds: Vec::new(),
                agent_version: "test".into(),
            },
        }))
    }

    fn model_with(agents: &[(Endpoint, &str)], silent: &[Endpoint], now: Instant) -> Model {
        let mut model = Model::new(2015, now - SEARCH_GRACE * 2, Alerts::in_memory(now));
        for (endpoint, node) in agents {
            model.links.insert(*endpoint, Link::connecting());
            model.apply(*endpoint, welcome(node), now);
        }
        for endpoint in silent {
            model.links.insert(*endpoint, Link::connecting());
            model.apply(*endpoint, LinkUpdate::Lost("agent not running".into()), now);
        }
        model
    }

    fn names(model: &Model, now: Instant) -> Vec<(String, bool)> {
        model
            .rows(now)
            .into_iter()
            .map(|row| (row.name, row.here))
            .collect()
    }

    #[test]
    fn shows_this_machine_first_then_every_agent_by_name() {
        let now = Instant::now();
        let model = model_with(
            &[(NAS, "nas"), (LOCAL, "mac"), (HOMELAB, "homelab")],
            &[],
            now,
        );
        assert_eq!(
            names(&model, now),
            vec![
                ("mac".into(), true),
                ("homelab".into(), false),
                ("nas".into(), false)
            ]
        );
    }

    #[test]
    fn peers_that_never_answered_are_not_shown() {
        let now = Instant::now();
        let model = model_with(&[(LOCAL, "mac")], &[PHONE], now);
        assert_eq!(names(&model, now), vec![("mac".into(), true)]);
    }

    #[test]
    fn a_peer_whose_agent_refuses_this_viewer_is_listed_with_the_reason() {
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac")], &[], now);
        model.topology.peers = vec![Peer {
            node: "homelab".into(),
            ipv4: Ipv4Addr::new(100, 64, 0, 7),
        }];
        model.links.insert(HOMELAB, Link::connecting());
        model.apply(
            HOMELAB,
            LinkUpdate::Rejected("this agent speaks protocol 3, the viewer speaks 4".into()),
            now,
        );
        let rows = model.rows(now);
        assert_eq!(rows[1].name, "homelab");
        assert!(matches!(rows[1].state(), Some(LinkState::Rejected(_))));
    }

    #[test]
    fn a_known_agent_that_drops_off_stays_listed_as_lost() {
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac"), (HOMELAB, "homelab")], &[], now);
        model.apply(HOMELAB, LinkUpdate::Lost("no answer".into()), now);
        let rows = model.rows(now);
        assert!(matches!(rows[1].state(), Some(LinkState::Lost { .. })));
        assert_eq!(model.known_agents().len(), 2);
    }

    #[test]
    fn this_machine_without_an_agent_is_missing_not_lost() {
        let now = Instant::now();
        let mut model = model_with(&[], &[LOCAL], now);
        model.topology.self_node = Some("mac".into());
        let rows = model.rows(now);
        assert_eq!(rows[0].name, "mac");
        assert!(matches!(rows[0].presence, Presence::NoAgent));
    }

    #[test]
    fn this_machine_is_searching_during_startup() {
        let now = Instant::now();
        let mut model = Model::new(2015, now, Alerts::in_memory(now));
        model.links.insert(LOCAL, Link::connecting());
        assert!(matches!(model.rows(now)[0].presence, Presence::Searching));
    }

    #[test]
    fn reconnecting_keeps_history_and_lost_time_does_not_reset() {
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac")], &[], now);
        model.apply(LOCAL, LinkUpdate::Sample(Box::default()), now);
        model.apply(LOCAL, LinkUpdate::Lost("reset".into()), now);
        model.apply(
            LOCAL,
            LinkUpdate::Lost("refused".into()),
            now + Duration::from_secs(5),
        );
        let LinkState::Lost { since, reason } = &model.links[&LOCAL].state else {
            panic!()
        };
        assert_eq!((*since, reason.as_str()), (now, "refused"));
        model.apply(LOCAL, welcome("mac"), now);
        assert_eq!(
            model.links[&LOCAL]
                .machine
                .as_ref()
                .unwrap()
                .cpu
                .tail(10)
                .len(),
            1
        );
    }

    #[test]
    fn agents_sort_by_who_needs_you_and_lost_machines_go_last() {
        use crate::proto::{Agent, AgentSession, Tokens, Waiting};
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac"), (HOMELAB, "homelab")], &[], now);
        let session = |title: &str, state: SessionState, state_secs: u32| AgentSession {
            agent: Agent::ClaudeCode,
            id: title.into(),
            title: title.into(),
            project: "p".into(),
            state,
            state_secs,
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
            branch: None,
            worktree: None,
            parent: None,
            activity: None,
            mode: None,
            queued: None,
        };
        let mac = Sample {
            sessions: vec![
                session("idle", SessionState::Idle, 5),
                session("work", SessionState::Working { tool: None }, 1),
            ],
            ..Sample::default()
        };
        let homelab = Sample {
            sessions: vec![session(
                "ask",
                SessionState::Waiting(Waiting::Input("input needed".into())),
                30,
            )],
            ..Sample::default()
        };
        model.apply(LOCAL, LinkUpdate::Sample(Box::new(mac)), now);
        model.apply(HOMELAB, LinkUpdate::Sample(Box::new(homelab)), now);
        let order = |model: &Model| {
            model
                .agents(now)
                .iter()
                .map(|agent| agent.session.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&model), vec!["ask", "work", "idle"]);
        model.apply(HOMELAB, LinkUpdate::Lost("no answer".into()), now);
        assert_eq!(order(&model), vec!["work", "idle", "ask"]);
    }

    #[test]
    fn selection_stays_in_range() {
        let now = Instant::now();
        let mut model = Model::new(2015, now, Alerts::in_memory(now));
        model.select(5, 3);
        assert_eq!(model.selected, 2);
        model.select(-9, 3);
        assert_eq!(model.selected, 0);
        model.select(1, 0);
        assert_eq!(model.selected, 0);
    }
}
