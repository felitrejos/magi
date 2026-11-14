mod alerts;
mod base64;
mod chart;
mod diacritics;
mod fleet;
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

use crate::{log, net, proto};
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

pub use fleet::print as fleet;

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

    fn request(&self, endpoint: Endpoint, request: &proto::Request) -> io::Result<()> {
        let (handle, _) = self.handles.get(&endpoint).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "that machine isn't connected")
        })?;
        handle.request(request)
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
        model.flash(reason, false, now);
        return Ok(());
    };
    model.opening = true;
    let sender = links.sender.clone();
    thread::Builder::new().name("open".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::open(&pane)));
    })?;
    Ok(())
}

fn propose_move(model: &mut Model, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    if agent.session.agent != proto::Agent::ClaudeCode {
        let reason = format!(
            "{} is a {} session; cielago moves Claude Code sessions",
            agent.session.title,
            agent.session.agent.name()
        );
        return model.flash(reason, false, now);
    }
    if !agent.live {
        return model.flash(format!("{} isn't answering", agent.machine), false, now);
    }
    let Some(target) = model.move_target(&agent, now) else {
        return model.flash("no other machine is answering".into(), false, now);
    };
    let pending = model::PendingMove {
        endpoint: agent.endpoint,
        session: agent.session.id.clone(),
        title: agent.session.title.clone(),
        from: agent.machine.clone(),
        to: target.machine,
    };
    model.pending_move = Some(pending);
}

fn confirm_move(model: &mut Model, links: &Links, pending: model::PendingMove, now: Instant) {
    let request = proto::Request::Move {
        session: pending.session,
        to: pending.to.clone(),
    };
    if let Err(error) = links.request(pending.endpoint, &request) {
        let reason = format!("couldn't ask {} to move it: {error}", pending.from);
        model.flash(reason, false, now);
    }
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
    sync_watch(model, links);
    Ok(Flow::Continue)
}

fn sync_watch(model: &mut Model, links: &Links) {
    let wanted = model.wanted_watch(Instant::now());
    if wanted == model.watching {
        return;
    }
    if let Some((endpoint, _)) = &model.watching {
        let _ = links.request(*endpoint, &proto::Request::Watch { session: None });
    }
    let started = wanted.filter(|(endpoint, session)| {
        let request = proto::Request::Watch {
            session: Some(session.clone()),
        };
        links.request(*endpoint, &request).is_ok()
    });
    model.watch(started);
}

fn handle(event: Event, model: &mut Model, links: &mut Links) -> io::Result<Flow> {
    let now = Instant::now();
    match event {
        Event::Input(Input::Key(key)) if let Some(pending) = model.pending_move.take() => {
            if key == Key::Yes {
                confirm_move(model, links, pending, now);
            }
        }
        Event::Input(Input::Key(key)) => match key {
            Key::Quit => return Ok(Flow::Quit),
            Key::Up => model.move_selection(-1, now),
            Key::Down => model.move_selection(1, now),
            Key::First => model.move_selection(isize::MIN, now),
            Key::Last => model.move_selection(isize::MAX, now),
            Key::Alerts => model.showing_alerts = !model.showing_alerts,
            Key::Open => open_selected(model, links, now)?,
            Key::Move => propose_move(model, now),
            Key::Screen => model.show_details = !model.show_details,
            Key::Yes | Key::No => {}
        },
        Event::Opened(result) => {
            model.opening = false;
            if let Err(reason) = result {
                model.flash(reason, false, now);
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
            usage: None,
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
            Some(("fix login isn't running in a Rex pane", false))
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

    fn live_sample(cpu: u16, session: Option<AgentSession>) -> LinkUpdate {
        LinkUpdate::Sample(Box::new(Sample {
            cpu,
            cores: vec![0; 8],
            sessions: session.into_iter().collect(),
            ..Sample::default()
        }))
    }

    fn claude(id: &str, millicores: u32) -> AgentSession {
        AgentSession {
            agent: crate::proto::Agent::ClaudeCode,
            id: id.into(),
            title: "run the tests".into(),
            project: "api".into(),
            state: SessionState::Working { tool: None },
            state_secs: 0,
            model: String::new(),
            context_tokens: 0,
            context_used: None,
            tokens: Tokens::default(),
            cost_micros: 0,
            cost_complete: true,
            age_secs: 0,
            pane: None,
            usage: Some(crate::proto::Usage {
                cpu: Some(millicores),
                memory: 0,
            }),
        }
    }

    #[test]
    fn m_proposes_the_least_busy_other_machine_and_only_y_sends_it() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(9_500, Some(claude("s1", 3_000))),
            now,
        );
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        let nas = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3));
        model.apply(homelab, live_sample(4_000, None), now);
        model.apply(nas, live_sample(300, None), now);
        model.selected = 3;
        let agent = model.selected_agent(now).unwrap();
        assert_eq!(
            model
                .move_suggestion(&agent, now)
                .map(|target| target.machine),
            Some("nas".into())
        );
        handle_batch([key(Key::Move)].into_iter(), &mut model, &mut links).unwrap();
        let pending = model.pending_move.clone().unwrap();
        assert_eq!((pending.from.as_str(), pending.to.as_str()), ("mac", "nas"));
        assert!(footer(&model).contains("Move run the tests from mac to nas?"));
        handle_batch([key(Key::No)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.pending_move, None);
        assert_eq!(model.flashed(now), None);
        handle_batch(
            [key(Key::Move), key(Key::Yes)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(model.pending_move, None);
        assert_eq!(
            model.flashed(Instant::now()),
            Some((
                "couldn't ask mac to move it: that machine isn't connected",
                false
            ))
        );
    }

    #[test]
    fn a_move_is_only_suggested_for_a_heavy_session_on_a_busy_machine_with_room_elsewhere() {
        let suggestion = |machine_cpu: u16, session_millicores: u32, other_cpu: u16| {
            let (mut model, _) = fleet();
            let now = Instant::now();
            model.apply(
                Endpoint::Local,
                live_sample(machine_cpu, Some(claude("s1", session_millicores))),
                now,
            );
            model.apply(
                Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2)),
                live_sample(other_cpu, None),
                now,
            );
            model.selected = 3;
            let agent = model.selected_agent(now).unwrap();
            model.move_suggestion(&agent, now).is_some()
        };
        assert!(suggestion(9_000, 2_000, 1_000));
        assert!(!suggestion(5_000, 2_000, 1_000));
        assert!(!suggestion(9_000, 500, 1_000));
        assert!(!suggestion(9_000, 2_000, 6_000));
    }

    #[test]
    fn replies_from_an_agent_show_in_the_footer_in_their_own_colour() {
        let (mut model, _) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            LinkUpdate::Reply(crate::proto::Reply {
                ok: true,
                text: "moving run the tests to nas…".into(),
            }),
            now,
        );
        assert_eq!(
            model.flashed(now),
            Some(("moving run the tests to nas…", true))
        );
        assert!(footer(&model).contains("moving run the tests to nas…"));
    }

    #[test]
    fn the_selected_rex_session_is_watched_and_its_screen_shown_until_s_switches_to_details() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = claude("s1", 0);
        session.pane = Some(RexPane {
            session: "session:1".into(),
            window: "window:2".into(),
        });
        model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
        model.selected = 3;
        assert_eq!(
            model.wanted_watch(now),
            Some((Endpoint::Local, "s1".to_string()))
        );
        model.watch(model.wanted_watch(now));
        assert!(footer(&model).contains("s details"));
        let screen = |text: &str| {
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                view: Ok(vec![vec![crate::proto::Run {
                    text: text.into(),
                    fg: crate::proto::Shade::Rgb(217, 119, 87),
                    bg: crate::proto::Shade::Default,
                    flags: crate::proto::BOLD,
                }]]),
            })
        };
        let draw = |model: &Model| {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
            terminal
                .draw(|frame| {
                    view::render(frame, model, Instant::now(), &mut Graphics::cells_only())
                })
                .unwrap();
            format!("{:?}", terminal.backend().buffer())
        };
        assert!(draw(&model).contains("connecting to the pane"));
        model.apply(Endpoint::Local, screen("> run the tests"), now);
        let shown = draw(&model);
        assert!(shown.contains("Screen"), "{shown}");
        assert!(shown.contains("> run the tests"), "{shown}");
        handle_batch([key(Key::Screen)].into_iter(), &mut model, &mut links).unwrap();
        assert!(model.show_details);
        assert_eq!(model.wanted_watch(now), None);
        assert_eq!(model.watching, None);
        assert!(!draw(&model).contains("> run the tests"));
        assert!(footer(&model).contains("s screen"));
    }

    #[test]
    fn a_reconnect_forgets_the_watch_so_it_is_asked_for_again_and_stale_screens_are_ignored() {
        let (mut model, _) = fleet();
        let now = Instant::now();
        model.watch(Some((Endpoint::Local, "s1".into())));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Screen(crate::proto::Screen {
                session: "other".into(),
                view: Err("old".into()),
            }),
            now,
        );
        assert_eq!(model.screen("other"), None);
        let identity = Identity {
            hostname: "mac".into(),
            tailscale_node: Some("mac".into()),
            os: String::new(),
            kernel: String::new(),
            arch: String::new(),
            cpu_model: String::new(),
            agent_version: String::new(),
        };
        model.apply(
            Endpoint::Local,
            LinkUpdate::Welcome(Box::new(Welcome {
                interval_ms: 1000,
                identity,
            })),
            now,
        );
        assert_eq!(model.watching, None);
    }
}
