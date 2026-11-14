use std::collections::{HashMap, VecDeque};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use crate::proto::{AgentSession, FULL, Identity, Plan, Sample, SessionState, Welcome};
use crate::tailscale::Peer;

pub const HISTORY: usize = 300;
const SEARCH_GRACE: Duration = Duration::from_secs(8);
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
    pub rx: History,
    pub tx: History,
}

impl Machine {
    fn new(welcome: Welcome) -> Self {
        Self {
            identity: welcome.identity,
            interval: Duration::from_millis(u64::from(welcome.interval_ms)),
            latest: None,
            cpu: History::default(),
            rx: History::default(),
            tx: History::default(),
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

    pub fn recent_max(&self, count: usize) -> f32 {
        self.tail(count).into_iter().fold(0.0, f32::max)
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

#[derive(Debug, Clone, PartialEq)]
pub struct Strain {
    pub subsystem: Subsystem,
    pub level: Level,
    pub worst: f32,
    pub culprit: Option<String>,
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
    pub machine: String,
    pub session: &'a AgentSession,
    pub live: bool,
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
    focus: Option<Focus>,
    started: Instant,
}

impl Model {
    pub fn new(port: u16, now: Instant) -> Self {
        Self {
            topology: Topology::default(),
            links: HashMap::new(),
            selected: 0,
            port,
            focus: None,
            started: now,
        }
    }

    pub fn apply(&mut self, endpoint: Endpoint, update: LinkUpdate, now: Instant) {
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
                machine.rx.push(sample.network.rx_per_sec as f32);
                machine.tx.push(sample.network.tx_per_sec as f32);
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

    pub fn strains(rows: &[Row<'_>]) -> [Strain; 3] {
        [Subsystem::Cpu, Subsystem::Memory, Subsystem::Storage].map(|subsystem| {
            let worst = rows
                .iter()
                .filter(|row| row.state() == Some(&LinkState::Live))
                .filter_map(|row| row.machine().map(|machine| (row.name.as_str(), machine)))
                .map(|(name, machine)| {
                    let value = match subsystem {
                        Subsystem::Cpu => machine.smoothed_cpu(),
                        Subsystem::Memory => machine.memory_fraction(),
                        Subsystem::Storage => machine.fullest_filesystem(),
                    };
                    (name, value)
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            Strain {
                subsystem,
                level: worst.map_or(Level::Normal, |(_, value)| grade(subsystem, value)),
                worst: worst.map_or(0.0, |(_, value)| value),
                culprit: worst.map(|(name, _)| name.to_string()),
            }
        })
    }

    pub fn agents(&self, now: Instant) -> Vec<AgentRow<'_>> {
        let mut agents: Vec<AgentRow<'_>> = self
            .rows(now)
            .into_iter()
            .filter_map(|row| {
                let live = row.state() == Some(&LinkState::Live);
                let machine = row.machine()?;
                let sample = machine.latest.as_ref()?;
                Some(sample.sessions.iter().map(move |session| AgentRow {
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

    pub fn plan(&self, now: Instant) -> Option<&Plan> {
        self.rows(now)
            .into_iter()
            .filter(|row| row.state() == Some(&LinkState::Live))
            .find_map(|row| row.machine()?.latest.as_ref()?.plan.as_ref())
    }

    pub fn selection(&self, now: Instant) -> Option<Focus> {
        let rows = self.rows(now);
        match rows.get(self.selected) {
            Some(row) => Some(Focus::Machine(row.name.clone())),
            None => self
                .agents(now)
                .get(self.selected - rows.len())
                .map(|agent| Focus::Agent(agent.session.id.clone())),
        }
    }

    pub fn move_selection(&mut self, delta: isize, now: Instant) {
        let count = self.rows(now).len() + self.agents(now).len();
        self.select(delta, count);
        self.focus = self.selection(now);
    }

    pub fn reselect(&mut self, now: Instant) {
        let rows = self.rows(now);
        let agents = self.agents(now);
        let count = rows.len() + agents.len();
        let found = match &self.focus {
            Some(Focus::Machine(name)) => rows.iter().position(|row| &row.name == name),
            Some(Focus::Agent(id)) => agents
                .iter()
                .position(|agent| &agent.session.id == id)
                .map(|index| rows.len() + index),
            None => None,
        };
        match found {
            Some(index) => self.selected = index,
            None => self.select(0, count),
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
    use crate::proto::{Filesystem, Memory};

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
                agent_version: "test".into(),
            },
        }))
    }

    fn model_with(agents: &[(Endpoint, &str)], silent: &[Endpoint], now: Instant) -> Model {
        let mut model = Model::new(2015, now - SEARCH_GRACE * 2);
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
        let mut model = Model::new(2015, now);
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
    fn strains_report_the_worst_live_machine() {
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac"), (HOMELAB, "homelab")], &[], now);
        let busy = Sample {
            memory: Memory {
                total: 100,
                used: 95,
                ..Memory::default()
            },
            filesystems: vec![Filesystem {
                mount: "/".into(),
                total: 100,
                used: 50,
            }],
            cpu: 9_500,
            ..Sample::default()
        };
        model.apply(HOMELAB, LinkUpdate::Sample(Box::new(busy)), now);
        model.apply(LOCAL, LinkUpdate::Sample(Box::default()), now);
        let [cpu, memory, storage] = Model::strains(&model.rows(now));
        assert_eq!(
            (cpu.level, cpu.culprit.as_deref()),
            (Level::Critical, Some("homelab"))
        );
        assert_eq!(memory.level, Level::Critical);
        assert_eq!(storage.level, Level::Normal);
    }

    #[test]
    fn agents_sort_by_who_needs_you_and_lost_machines_go_last() {
        use crate::proto::{AgentSession, Tokens, Waiting};
        let now = Instant::now();
        let mut model = model_with(&[(LOCAL, "mac"), (HOMELAB, "homelab")], &[], now);
        let session = |title: &str, state: SessionState, state_secs: u32| AgentSession {
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
        let mut model = Model::new(2015, Instant::now());
        model.select(5, 3);
        assert_eq!(model.selected, 2);
        model.select(-9, 3);
        assert_eq!(model.selected, 0);
        model.select(1, 0);
        assert_eq!(model.selected, 0);
    }
}
