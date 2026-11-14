mod alerts;
mod base64;
mod chart;
mod diacritics;
mod graphics;
mod input;
mod link;
mod model;
mod rex;
mod style;
mod terminal;
mod topology;
mod units;
mod view;

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

use crate::{log, net};
use graphics::Graphics;
use input::{Input, Key};
use link::{Dial, LinkHandle};
use model::{Endpoint, Link, LinkUpdate, Model, Topology};

const FRAME: Duration = Duration::from_millis(33);
const TICK: Duration = Duration::from_secs(1);
const THEME_CHECK: Duration = Duration::from_secs(2);
const EVENT_BACKLOG: usize = 4096;

pub struct Config {
    pub port: u16,
}

pub enum Event {
    Input(input::Input),
    Opened(Result<(), String>),
    Topology(Topology),
    Link(Endpoint, u64, LinkUpdate),
}

pub fn run(config: Config) -> io::Result<()> {
    let (sender, events) = mpsc::sync_channel(EVENT_BACKLOG);
    let mut links = Links {
        handles: HashMap::new(),
        next_generation: 0,
        port: config.port,
        socket: net::local_socket()?,
        sender: sender.clone(),
    };
    let alerts = alerts::Alerts::open(net::state_dir()?.join("alerts.jsonl"), Instant::now())?;
    let mut terminal = ratatui::try_init()?;
    let reply = terminal::probe(&mut io::stdout())?;
    style::update(&reply);
    let mut graphics = Graphics::from_reply(&reply);
    write_terminal(terminal::FOLLOW)?;
    topology::spawn(sender.clone())?;
    spawn_input(sender, terminal::unanswered(&reply).to_vec())?;
    let result = write_status(alerts::CLEAR_ALL).and_then(|()| {
        event_loop(
            &mut terminal,
            &events,
            &mut links,
            config.port,
            alerts,
            &mut graphics,
        )
    });
    let cleared = write_status(alerts::CLEAR_ALL)
        .and_then(|()| write_terminal(terminal::UNFOLLOW))
        .and_then(|()| graphics.clear(&mut io::stdout()));
    ratatui::restore();
    result.and(cleared)
}

fn spawn_input(events: SyncSender<Event>, early: Vec<u8>) -> io::Result<()> {
    thread::Builder::new().name("input".into()).spawn(move || {
        let _ = input::read(&early, |input| events.send(Event::Input(input)).is_ok());
        let _ = events.send(Event::Input(Input::Key(Key::Quit)));
    })?;
    Ok(())
}

#[derive(PartialEq)]
enum Flow {
    Continue,
    Quit,
}

struct Links {
    handles: HashMap<Endpoint, (LinkHandle, u64)>,
    next_generation: u64,
    port: u16,
    socket: PathBuf,
    sender: SyncSender<Event>,
}

impl Links {
    fn reconcile(&mut self, model: &mut Model, wanted: &[Endpoint]) -> io::Result<()> {
        self.handles.retain(|endpoint, _| wanted.contains(endpoint));
        model.links.retain(|endpoint, _| wanted.contains(endpoint));
        for endpoint in wanted {
            if self.handles.contains_key(endpoint) {
                continue;
            }
            let dial = match endpoint {
                Endpoint::Local => Dial::Unix(self.socket.clone()),
                Endpoint::Tailnet(ip) => Dial::Tcp(SocketAddr::from((*ip, self.port))),
            };
            self.next_generation += 1;
            let handle = link::spawn(*endpoint, dial, self.next_generation, self.sender.clone())?;
            self.handles
                .insert(*endpoint, (handle, self.next_generation));
            model.links.insert(*endpoint, Link::connecting());
        }
        Ok(())
    }

    fn is_current(&self, endpoint: Endpoint, generation: u64) -> bool {
        self.handles
            .get(&endpoint)
            .is_some_and(|(_, current)| *current == generation)
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    events: &Receiver<Event>,
    links: &mut Links,
    port: u16,
    alerts: alerts::Alerts,
    graphics: &mut Graphics,
) -> io::Result<()> {
    let mut model = Model::new(port, Instant::now(), alerts);
    links.reconcile(&mut model, &[Endpoint::Local])?;
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    let mut last_theme_check = Instant::now();
    loop {
        if last_theme_check.elapsed() >= THEME_CHECK {
            write_terminal(terminal::COLOURS)?;
            last_theme_check = Instant::now();
        }
        if dirty && last_draw.elapsed() >= FRAME {
            let now = Instant::now();
            let now_secs = log::now_secs();
            let conditions = alerts::conditions(&model, now, now_secs);
            let changes = model.alerts.update(conditions, now, now_secs);
            write_status(&alerts::report(&changes))?;
            let mut out = io::stdout();
            execute!(out, BeginSynchronizedUpdate)?;
            let drawn = terminal
                .draw(|frame| view::render(frame, &model, now, graphics))
                .map(drop)
                .and_then(|()| graphics.flush(&mut out));
            execute!(out, EndSynchronizedUpdate)?;
            drawn?;
            last_draw = now;
            dirty = false;
        }
        let wait = if dirty {
            FRAME.saturating_sub(last_draw.elapsed())
        } else {
            TICK
        };
        let Ok(first) = events.recv_timeout(wait) else {
            dirty = true;
            continue;
        };
        if handle_batch(
            std::iter::once(first).chain(events.try_iter()),
            &mut model,
            links,
        )? == Flow::Quit
        {
            return Ok(());
        }
        dirty = true;
    }
}

fn open_selected(model: &mut Model, links: &Links, now: Instant) -> io::Result<()> {
    if model.opening {
        return Ok(());
    }
    let Some(agent) = model.selected_agent(now) else {
        return Ok(());
    };
    let Some(pane) = agent.session.pane.clone() else {
        let reason = format!("{} isn't running in a Rex pane", agent.session.title);
        model.flash(reason, now);
        return Ok(());
    };
    model.opening = true;
    let sender = links.sender.clone();
    thread::Builder::new().name("open".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::open(&pane)));
    })?;
    Ok(())
}

fn write_terminal(sequence: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(sequence)?;
    out.flush()
}

fn write_status(sequence: &str) -> io::Result<()> {
    if sequence.is_empty() {
        return Ok(());
    }
    write_terminal(sequence.as_bytes())
}

fn handle_batch(
    events: impl Iterator<Item = Event>,
    model: &mut Model,
    links: &mut Links,
) -> io::Result<Flow> {
    for event in events {
        if handle(event, model, links)? == Flow::Quit {
            return Ok(Flow::Quit);
        }
    }
    model.reselect(Instant::now());
    Ok(Flow::Continue)
}

fn handle(event: Event, model: &mut Model, links: &mut Links) -> io::Result<Flow> {
    let now = Instant::now();
    match event {
        Event::Input(Input::Key(key)) => match key {
            Key::Quit => return Ok(Flow::Quit),
            Key::Up => model.move_selection(-1, now),
            Key::Down => model.move_selection(1, now),
            Key::First => model.move_selection(isize::MIN, now),
            Key::Last => model.move_selection(isize::MAX, now),
            Key::Alerts => model.showing_alerts = !model.showing_alerts,
            Key::Open => open_selected(model, links, now)?,
        },
        Event::Opened(result) => {
            model.opening = false;
            if let Err(reason) = result {
                model.flash(reason, now);
            }
        }
        Event::Input(Input::Recheck) => write_terminal(terminal::COLOURS)?,
        Event::Input(Input::Theme(reply)) => {
            style::update(&reply);
        }
        Event::Input(Input::Resize) => {}
        Event::Topology(topology) => {
            let mut wanted: Vec<Endpoint> = std::iter::once(Endpoint::Local)
                .chain(
                    topology
                        .peers
                        .iter()
                        .map(|peer| Endpoint::Tailnet(peer.ipv4)),
                )
                .chain(model.known_agents())
                .collect();
            wanted.sort();
            wanted.dedup();
            model.topology = topology;
            links.reconcile(model, &wanted)?;
        }
        Event::Link(endpoint, generation, mut update) => {
            if !links.is_current(endpoint, generation) {
                return Ok(Flow::Continue);
            }
            if let (Endpoint::Tailnet(ip), LinkUpdate::Welcome(welcome)) = (endpoint, &mut update) {
                welcome.identity.tailscale_node = model
                    .topology
                    .peers
                    .iter()
                    .find(|peer| peer.ipv4 == ip)
                    .map(|peer| peer.node.clone());
            }
            model.apply(endpoint, update, now);
        }
    }
    Ok(Flow::Continue)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::proto::{AgentSession, Identity, RexPane, Sample, SessionState, Tokens, Welcome};

    fn key(key: Key) -> Event {
        Event::Input(Input::Key(key))
    }

    fn agent(model: &mut Model, endpoint: Endpoint, name: &str) {
        model.links.insert(endpoint, Link::connecting());
        let identity = Identity {
            hostname: name.into(),
            tailscale_node: Some(name.into()),
            os: String::new(),
            kernel: String::new(),
            arch: String::new(),
            cpu_model: String::new(),
            agent_version: String::new(),
        };
        model.apply(
            endpoint,
            LinkUpdate::Welcome(Box::new(Welcome {
                interval_ms: 1000,
                identity,
            })),
            Instant::now(),
        );
    }

    fn fleet() -> (Model, Links) {
        let (sender, _) = mpsc::sync_channel(8);
        let links = Links {
            handles: HashMap::new(),
            next_generation: 0,
            port: 2015,
            socket: PathBuf::new(),
            sender,
        };
        let now = Instant::now();
        let mut model = Model::new(2015, now, alerts::Alerts::in_memory(now));
        agent(&mut model, Endpoint::Local, "mac");
        agent(
            &mut model,
            Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2)),
            "homelab",
        );
        agent(
            &mut model,
            Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3)),
            "nas",
        );
        (model, links)
    }

    fn selected(model: &Model) -> String {
        match model.selection(Instant::now()) {
            Some(model::Focus::Machine(name)) => name,
            other => panic!("expected a machine, got {other:?}"),
        }
    }

    #[test]
    fn arrow_keys_move_the_selection() {
        let (mut model, mut links) = fleet();
        handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model), "homelab");
        handle_batch(
            [key(Key::Down), key(Key::Down), key(Key::Up)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(selected(&model), "homelab");
        handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model), "nas");
    }

    #[test]
    fn the_selection_follows_its_machine_when_rows_reorder() {
        let (mut model, mut links) = fleet();
        handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
        agent(
            &mut model,
            Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 4)),
            "laptop",
        );
        let sample = Event::Link(Endpoint::Local, 0, LinkUpdate::Sample(Box::default()));
        handle_batch([sample].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model), "nas");
    }

    #[test]
    fn q_quits() {
        let (mut model, mut links) = fleet();
        let flow = handle_batch([key(Key::Quit)].into_iter(), &mut model, &mut links).unwrap();
        assert!(flow == Flow::Quit);
    }

    fn with_session(model: &mut Model, pane: Option<RexPane>) {
        let session = AgentSession {
            agent: crate::proto::Agent::ClaudeCode,
            id: "s1".into(),
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
            pane,
        };
        let sample = Sample {
            sessions: vec![session],
            ..Sample::default()
        };
        model.apply(
            Endpoint::Local,
            LinkUpdate::Sample(Box::new(sample)),
            Instant::now(),
        );
        model.selected = 3;
    }

    fn footer(model: &Model) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| view::render(frame, model, Instant::now(), &mut Graphics::cells_only()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..120).map(|x| buffer[(x, 29)].symbol()).collect()
    }

    #[test]
    fn enter_explains_when_a_session_is_not_in_rex() {
        let (mut model, mut links) = fleet();
        with_session(&mut model, None);
        assert!(!footer(&model).contains("open"));
        handle_batch([key(Key::Open)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some("fix login isn't running in a Rex pane")
        );
        assert!(footer(&model).contains("fix login isn't running in a Rex pane"));
    }

    #[test]
    fn the_footer_offers_to_open_a_session_that_is_in_rex() {
        let (mut model, _) = fleet();
        with_session(
            &mut model,
            Some(RexPane {
                session: "session:1".into(),
                window: "window:2".into(),
            }),
        );
        assert!(footer(&model).contains("↵ open"), "{}", footer(&model));
        model.selected = 0;
        assert!(!footer(&model).contains("↵ open"));
    }
}
