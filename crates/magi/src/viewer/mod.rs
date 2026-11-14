mod alerts;
mod chart;
mod link;
mod model;
mod style;
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
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};

use crate::{log, net};
use link::{Dial, LinkHandle};
use model::{Endpoint, Link, LinkUpdate, Model, Topology};

const FRAME: Duration = Duration::from_millis(33);
const TICK: Duration = Duration::from_secs(1);
const EVENT_BACKLOG: usize = 4096;

pub struct Config {
    pub port: u16,
}

pub enum Event {
    Input(TermEvent),
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
    topology::spawn(sender.clone())?;
    spawn_input(sender)?;
    let alerts = alerts::Alerts::open(net::state_dir()?.join("alerts.jsonl"), Instant::now())?;
    let mut terminal = ratatui::try_init()?;
    let result = write_status(alerts::CLEAR_ALL)
        .and_then(|()| event_loop(&mut terminal, &events, &mut links, config.port, alerts));
    let cleared = write_status(alerts::CLEAR_ALL);
    ratatui::restore();
    result.and(cleared)
}

fn spawn_input(events: SyncSender<Event>) -> io::Result<()> {
    thread::Builder::new().name("input".into()).spawn(move || {
        while let Ok(input) = event::read() {
            if events.send(Event::Input(input)).is_err() {
                return;
            }
        }
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
) -> io::Result<()> {
    let mut model = Model::new(port, Instant::now(), alerts);
    links.reconcile(&mut model, &[Endpoint::Local])?;
    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    loop {
        if dirty && last_draw.elapsed() >= FRAME {
            let now = Instant::now();
            let now_secs = log::now_secs();
            let conditions = alerts::conditions(&model, now, now_secs);
            let changes = model.alerts.update(conditions, now, now_secs);
            write_status(&alerts::report(&changes))?;
            terminal.draw(|frame| view::render(frame, &model, now))?;
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

fn write_status(sequence: &str) -> io::Result<()> {
    if sequence.is_empty() {
        return Ok(());
    }
    let mut out = io::stdout().lock();
    out.write_all(sequence.as_bytes())?;
    out.flush()
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
        Event::Input(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(Flow::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Flow::Quit);
            }
            KeyCode::Up | KeyCode::Char('k') => model.move_selection(-1, now),
            KeyCode::Down | KeyCode::Char('j') => model.move_selection(1, now),
            KeyCode::Home | KeyCode::Char('g') => model.move_selection(isize::MIN, now),
            KeyCode::End | KeyCode::Char('G') => model.move_selection(isize::MAX, now),
            KeyCode::Char('a') => model.showing_alerts = !model.showing_alerts,
            _ => {}
        },
        Event::Input(_) => {}
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

    use ratatui::crossterm::event::KeyEvent;

    use super::*;
    use crate::proto::{Identity, Welcome};

    fn key(code: KeyCode) -> Event {
        Event::Input(TermEvent::Key(KeyEvent::from(code)))
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
        handle_batch([key(KeyCode::Down)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model), "homelab");
        handle_batch(
            [key(KeyCode::Down), key(KeyCode::Down), key(KeyCode::Up)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(selected(&model), "homelab");
        handle_batch([key(KeyCode::End)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model), "nas");
    }

    #[test]
    fn the_selection_follows_its_machine_when_rows_reorder() {
        let (mut model, mut links) = fleet();
        handle_batch([key(KeyCode::End)].into_iter(), &mut model, &mut links).unwrap();
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
        let flow = handle_batch(
            [key(KeyCode::Char('q'))].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert!(flow == Flow::Quit);
    }
}
