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
    write_terminal(terminal::MODES)?;
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
        .and_then(|()| write_terminal(terminal::RESET_MODES))
        .and_then(|()| graphics.clear(&mut io::stdout()));
    ratatui::restore();
    result.and(cleared)
}

fn spawn_input(events: SyncSender<Event>, early: Vec<u8>) -> io::Result<()> {
    thread::Builder::new().name("input".into()).spawn(move || {
        let _ = input::read(&early, |input| events.send(Event::Input(input)).is_ok());
        let _ = events.send(Event::Input(Input::Closed));
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

enum Action {
    Quit,
    Back,
    Up,
    Down,
    First,
    Last,
    Alerts,
    Open,
    Move,
    Message,
    Allow,
    Deny,
    Choose(u8),
    New,
    Stop,
    Adopt,
}

fn action(key: &Key) -> Option<Action> {
    Some(match key {
        Key::Char('q') | Key::Interrupt => Action::Quit,
        Key::Escape => Action::Back,
        Key::Char('k') | Key::Up => Action::Up,
        Key::Char('j') | Key::Down => Action::Down,
        Key::Char('g') | Key::First => Action::First,
        Key::Char('G') | Key::Last => Action::Last,
        Key::Char('a') => Action::Alerts,
        Key::Enter => Action::Message,
        Key::Char('o') => Action::Open,
        Key::Char('m') => Action::Move,
        Key::Char('y') => Action::Allow,
        Key::Char('n') => Action::Deny,
        Key::Char('c') => Action::New,
        Key::Char('x') => Action::Stop,
        Key::Char('r') => Action::Adopt,
        Key::Char(digit @ '1'..='9') => Action::Choose(*digit as u8 - b'0'),
        _ => return None,
    })
}

fn click(model: &mut Model, column: u16, row: u16, now: Instant) {
    let Ok((width, height)) = ratatui::crossterm::terminal::size() else {
        return;
    };
    let full = ratatui::layout::Rect::new(0, 0, width, height);
    let areas = view::areas(
        full,
        model.rows(now).len(),
        model.plans(now).len(),
        model.agents(now).len(),
    );
    let on_detail = areas
        .detail
        .contains(ratatui::layout::Position::new(column, row));
    if on_detail && !model.showing_alerts && model.draft.is_none() {
        start_draft(model, now);
    }
}

fn start_draft(model: &mut Model, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    if agent.session.pane.is_none() {
        let reason = format!(
            "{} isn't in a Rex pane, so magi can't type into it",
            agent.session.title
        );
        return model.flash(reason, false, now);
    }
    if !agent.live {
        return model.flash(format!("{} isn't answering", agent.machine), false, now);
    }
    if agent.session.state != proto::SessionState::Idle {
        let reason = format!("{} isn't idle: message it once it is", agent.session.title);
        return model.flash(reason, false, now);
    }
    model.draft = Some(model::Draft {
        endpoint: agent.endpoint,
        session: agent.session.id.clone(),
        title: agent.session.title.clone(),
        text: String::new(),
    });
}

fn edit_draft(model: &mut Model, links: &Links, mut draft: model::Draft, key: Key, now: Instant) {
    match key {
        Key::Char(c) => draft.type_in(c.encode_utf8(&mut [0; 4])),
        Key::Paste(text) => draft.type_in(&text),
        Key::Newline => draft.type_in("\n"),
        Key::Backspace => draft.erase(),
        Key::Escape | Key::Interrupt => return,
        Key::Enter if !draft.text.trim().is_empty() => {
            let request = proto::Request::Prompt {
                session: draft.session,
                text: draft.text,
            };
            if let Err(error) = links.request(draft.endpoint, &request) {
                model.flash(format!("couldn't send it: {error}"), false, now);
            }
            return;
        }
        Key::Enter | Key::Up | Key::Down | Key::First | Key::Last | Key::Tab => {}
    }
    model.draft = Some(draft);
}

fn target_machine(model: &Model, now: Instant) -> Option<(Endpoint, String)> {
    if let Some(agent) = model.selected_agent(now) {
        return Some((agent.endpoint, agent.machine.clone()));
    }
    let rows = model.rows(now);
    let row = rows.get(model.selected)?;
    Some((row.endpoint, row.name.clone()))
}

fn open_picker(model: &mut Model, links: &Links, now: Instant) {
    let Some((endpoint, machine)) = target_machine(model, now) else {
        return;
    };
    if let Err(error) = links.request(endpoint, &proto::Request::Folders) {
        return model.flash(
            format!("couldn't ask {machine} for its projects: {error}"),
            false,
            now,
        );
    }
    model.picker = Some(model::Picker::new(endpoint, machine));
}

fn edit_picker(
    model: &mut Model,
    links: &Links,
    mut picker: model::Picker,
    key: Key,
    now: Instant,
) {
    let recent = model.recent_projects(picker.endpoint, now);
    let count = picker.shown(&recent).len();
    match key {
        Key::Char(c) if !c.is_control() => {
            picker.filter.push(c);
            picker.selected = 0;
        }
        Key::Paste(text) => {
            picker
                .filter
                .extend(text.chars().filter(|c| !c.is_control()));
            picker.selected = 0;
        }
        Key::Backspace => {
            picker.filter.pop();
            picker.selected = 0;
        }
        Key::Up => picker.selected = picker.selected.saturating_sub(1),
        Key::Down => picker.selected = (picker.selected + 1).min(count.saturating_sub(1)),
        Key::First => picker.selected = 0,
        Key::Last => picker.selected = count.saturating_sub(1),
        Key::Tab => picker.codex = !picker.codex,
        Key::Escape | Key::Interrupt => return,
        Key::Enter => {
            let Some(dir) = picker
                .shown(&recent)
                .get(picker.selected.min(count.saturating_sub(1)))
                .map(|dir| dir.to_string())
            else {
                model.picker = Some(picker);
                return;
            };
            let request = proto::Request::Open {
                dir: dir.clone(),
                codex: picker.codex,
            };
            match links.request(picker.endpoint, &request) {
                Ok(()) => {
                    let name = dir.rsplit('/').next().unwrap_or(&dir);
                    model.await_session(picker.endpoint, name, now);
                    let agent = if picker.codex { "Codex" } else { "Claude" };
                    model.flash(
                        format!("Opening {agent} in {name} on {}…", picker.machine),
                        true,
                        now,
                    );
                }
                Err(error) => model.flash(format!("couldn't open it: {error}"), false, now),
            }
            return;
        }
        Key::Char(_) | Key::Newline => {}
    }
    model.picker = Some(picker);
}

fn adopt(model: &mut Model, links: &Links, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    let adoptable = agent.live
        && agent.session.pane.is_none()
        && agent.session.agent == proto::Agent::ClaudeCode;
    if !adoptable {
        return;
    }
    let (endpoint, project, title) = (
        agent.endpoint,
        agent.session.project.clone(),
        agent.session.title.clone(),
    );
    let request = proto::Request::Adopt {
        session: agent.session.id.clone(),
    };
    match links.request(endpoint, &request) {
        Ok(()) => {
            model.await_session(endpoint, &project, now);
            model.flash(format!("Continuing {title} in Rex…"), true, now);
        }
        Err(error) => model.flash(format!("couldn't continue it in Rex: {error}"), false, now),
    }
}

fn stop(model: &mut Model, links: &Links, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    let working = matches!(agent.session.state, proto::SessionState::Working { .. });
    if !working || agent.session.pane.is_none() {
        return;
    }
    let request = proto::Request::Stop {
        session: agent.session.id.clone(),
    };
    if let Err(error) = links.request(agent.endpoint, &request) {
        model.flash(format!("couldn't stop it: {error}"), false, now);
    }
}

fn choose(model: &mut Model, links: &Links, option: u8, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    let asking = matches!(
        agent.session.state,
        proto::SessionState::Waiting(proto::Waiting::Input(_))
    );
    if !asking || agent.session.pane.is_none() {
        return;
    }
    let request = proto::Request::Choose {
        session: agent.session.id.clone(),
        option,
        waiting_secs: agent.session.state_secs,
    };
    if let Err(error) = links.request(agent.endpoint, &request) {
        model.flash(format!("couldn't answer it: {error}"), false, now);
    }
}

fn answer(model: &mut Model, links: &Links, allow: bool, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    let proto::SessionState::Waiting(proto::Waiting::Permission(tool)) = &agent.session.state
    else {
        return;
    };
    if agent.session.pane.is_none() {
        let reason = format!(
            "{} isn't in a Rex pane: answer it where it runs",
            agent.session.title
        );
        return model.flash(reason, false, now);
    }
    let request = proto::Request::Answer {
        session: agent.session.id.clone(),
        allow,
        tool: tool.clone(),
        waiting_secs: agent.session.state_secs,
    };
    if let Err(error) = links.request(agent.endpoint, &request) {
        model.flash(format!("couldn't answer it: {error}"), false, now);
    }
}

fn confirm_move(model: &mut Model, links: &Links, pending: model::PendingMove, now: Instant) {
    let request = proto::Request::Move {
        session: pending.session.clone(),
        to: pending.to.clone(),
    };
    match links.request(pending.endpoint, &request) {
        Ok(()) => model.follow(pending.session, pending.endpoint, pending.to, now),
        Err(error) => {
            let reason = format!("couldn't ask {} to move it: {error}", pending.from);
            model.flash(reason, false, now);
        }
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
    let mut drafted = false;
    for event in events {
        let is_key = matches!(event, Event::Input(Input::Key(_)));
        if drafted && is_key {
            continue;
        }
        let drafting = model.draft.is_some();
        if handle(event, model, links)? == Flow::Quit {
            return Ok(Flow::Quit);
        }
        drafted |= drafting && model.draft.is_none();
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
        Event::Input(Input::Key(key)) if let Some(picker) = model.picker.take() => {
            edit_picker(model, links, picker, key, now);
        }
        Event::Input(Input::Key(key)) if let Some(draft) = model.draft.take() => {
            edit_draft(model, links, draft, key, now);
        }
        Event::Input(Input::Key(key)) if let Some(pending) = model.pending_move.take() => {
            if key == Key::Char('y') {
                confirm_move(model, links, pending, now);
            }
        }
        Event::Input(Input::Key(key)) => match action(&key) {
            Some(Action::Quit) => return Ok(Flow::Quit),
            Some(Action::Back) => model.showing_alerts = false,
            Some(Action::Up) => model.move_selection(-1, now),
            Some(Action::Down) => model.move_selection(1, now),
            Some(Action::First) => model.move_selection(isize::MIN, now),
            Some(Action::Last) => model.move_selection(isize::MAX, now),
            Some(Action::Alerts) => model.showing_alerts = !model.showing_alerts,
            Some(Action::Open) => open_selected(model, links, now)?,
            Some(Action::Move) => propose_move(model, now),
            Some(Action::Message) => start_draft(model, now),
            Some(Action::Allow) => answer(model, links, true, now),
            Some(Action::Deny) => answer(model, links, false, now),
            Some(Action::Choose(option)) => choose(model, links, option, now),
            Some(Action::New) => open_picker(model, links, now),
            Some(Action::Stop) => stop(model, links, now),
            Some(Action::Adopt) => adopt(model, links, now),
            None => {}
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
        Event::Input(Input::Closed) => return Ok(Flow::Quit),
        Event::Input(Input::Click { column, row }) => click(model, column, row, now),
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
        let flow = handle_batch([key(Key::Char('q'))].into_iter(), &mut model, &mut links).unwrap();
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
            children: Vec::new(),
            branch: None,
            worktree: None,
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

    fn screen(model: &Model) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| view::render(frame, model, Instant::now(), &mut Graphics::cells_only()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..30)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
    fn o_explains_when_a_session_is_not_in_rex() {
        let (mut model, mut links) = fleet();
        with_session(&mut model, None);
        assert!(!footer(&model).contains("open"));
        handle_batch([key(Key::Char('o'))].into_iter(), &mut model, &mut links).unwrap();
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
        assert!(
            footer(&model).contains("o open in Rex"),
            "{}",
            footer(&model)
        );
        model.selected = 0;
        assert!(!footer(&model).contains("open in Rex"));
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
            children: Vec::new(),
            branch: None,
            worktree: None,
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
        handle_batch([key(Key::Char('m'))].into_iter(), &mut model, &mut links).unwrap();
        let pending = model.pending_move.clone().unwrap();
        assert_eq!((pending.from.as_str(), pending.to.as_str()), ("mac", "nas"));
        assert!(footer(&model).contains("Move run the tests from mac to nas?"));
        handle_batch([key(Key::Char('n'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.pending_move, None);
        assert_eq!(model.flashed(now), None);
        handle_batch(
            [key(Key::Char('m')), key(Key::Char('y'))].into_iter(),
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
    fn a_rex_session_asking_you_shows_its_screen_until_the_selection_moves() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = claude("s1", 0);
        session.state = SessionState::Waiting(crate::proto::Waiting::Permission(None));
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
        let screen = |text: &str| {
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                view: Ok(crate::proto::Content::Screen(vec![vec![
                    crate::proto::Run {
                        text: text.into(),
                        fg: crate::proto::Shade::Rgb(217, 119, 87),
                        bg: crate::proto::Shade::Default,
                        flags: crate::proto::BOLD,
                    },
                ]])),
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
        assert!(shown.contains("> run the tests"), "{shown}");
        handle_batch([key(Key::First)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.wanted_watch(now), None);
        assert_eq!(model.watching, None);
        assert!(!draw(&model).contains("> run the tests"));
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

    #[test]
    fn a_session_outside_rex_shows_its_conversation_wrapped_to_the_panel() {
        let (mut model, _) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.selected = 3;
        assert_eq!(
            model.wanted_watch(now),
            Some((Endpoint::Local, "s1".to_string()))
        );
        model.watch(model.wanted_watch(now));
        let long = "word ".repeat(60);
        model.apply(
            Endpoint::Local,
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                view: Ok(crate::proto::Content::Conversation(vec![
                    crate::proto::Said::You("run the tests".into()),
                    crate::proto::Said::Reply(format!("**Done.** {}", long.trim())),
                ])),
            }),
            now,
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 30)).unwrap();
        terminal
            .draw(|frame| view::render(frame, &model, Instant::now(), &mut Graphics::cells_only()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen: Vec<String> = (0..30)
            .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let joined = screen.join("\n");
        assert!(joined.contains("› run the tests"), "{joined}");
        let wrapped = screen
            .iter()
            .filter(|line| line.contains("word word"))
            .count();
        assert!(wrapped >= 3, "long replies wrap\n{joined}");
    }

    #[test]
    fn when_the_selected_session_ends_the_cursor_settles_and_then_stays_put() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let sample = |sessions: Vec<AgentSession>| {
            LinkUpdate::Sample(Box::new(Sample {
                cores: vec![0; 8],
                sessions,
                ..Sample::default()
            }))
        };
        let named = |id: &str, title: &str, state: SessionState| AgentSession {
            title: title.into(),
            state,
            ..claude(id, 0)
        };
        model.apply(
            Endpoint::Local,
            sample(vec![
                named("a", "alpha", SessionState::Idle),
                named("b", "bravo", SessionState::Idle),
            ]),
            now,
        );
        handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.selected_agent(now).unwrap().session.id, "b");
        model.apply(
            Endpoint::Local,
            sample(vec![named("a", "alpha", SessionState::Idle)]),
            now,
        );
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        let settled = model
            .selected_agent(now)
            .map(|agent| agent.session.id.clone());
        assert_eq!(settled.as_deref(), Some("a"));
        model.apply(
            Endpoint::Local,
            sample(vec![
                named("a", "alpha", SessionState::Idle),
                named(
                    "c",
                    "charlie",
                    SessionState::Waiting(crate::proto::Waiting::Permission(None)),
                ),
            ]),
            now,
        );
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        assert_eq!(model.selected_agent(now).unwrap().session.id, "a");
    }

    fn in_rex(mut session: AgentSession) -> AgentSession {
        session.state = SessionState::Idle;
        session.pane = Some(RexPane {
            session: "session:1".into(),
            window: "window:2".into(),
        });
        session
    }

    #[test]
    fn enter_opens_a_message_box_that_types_letters_instead_of_running_keys() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_rex(claude("s1", 0)))),
            now,
        );
        model.selected = 3;
        assert!(screen(&model).contains("Message this session"));
        let typed =
            std::iter::once(key(Key::Enter)).chain("qjmy\x01".chars().map(|c| key(Key::Char(c))));
        handle_batch(typed, &mut model, &mut links).unwrap();
        handle_batch(
            [key(Key::Paste("one\ntwo".into())), key(Key::Backspace)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(model.draft.as_ref().unwrap().text, "qjmy one\ntw");
        let shown = screen(&model);
        assert!(shown.contains("› qjmy one "), "{shown}");
        assert!(shown.contains("  tw▏"), "{shown}");
        let lines = (0..8).flat_map(|n| [key(Key::Newline), key(Key::Char(char::from(b'a' + n)))]);
        handle_batch(lines, &mut model, &mut links).unwrap();
        let shown = screen(&model);
        assert!(shown.contains("4 more above"), "{shown}");
        assert!(shown.contains("  h▏") && !shown.contains("qjmy"), "{shown}");
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.draft, None);
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("couldn't send it: that machine isn't connected", false))
        );
        handle_batch(
            [key(Key::Enter), key(Key::Escape)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(model.draft, None);
    }

    #[test]
    fn a_session_outside_rex_cannot_be_messaged() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.selected = 3;
        assert!(!screen(&model).contains("Message this session"));
        assert!(screen(&model).contains("continue it in Rex to message it here"));
        assert!(
            footer(&model).contains("r continue in Rex"),
            "{}",
            footer(&model)
        );
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.draft, None);
        assert_eq!(
            model.flashed(Instant::now()),
            Some((
                "run the tests isn't in a Rex pane, so magi can't type into it",
                false
            ))
        );
    }

    #[test]
    fn y_and_n_answer_only_a_session_asking_for_permission() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_rex(claude("s1", 0)))),
            now,
        );
        model.selected = 3;
        assert!(!screen(&model).contains("Allow Bash?"));
        handle_batch([key(Key::Char('y'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.flashed(Instant::now()), None);
        let mut asking = in_rex(claude("s1", 0));
        asking.state =
            SessionState::Waiting(crate::proto::Waiting::Permission(Some("Bash".into())));
        model.apply(Endpoint::Local, live_sample(1_000, Some(asking)), now);
        assert!(
            screen(&model).contains("Allow Bash?") && screen(&model).contains("y allow   n deny"),
            "{}",
            screen(&model)
        );
        handle_batch([key(Key::Char('n'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("couldn't answer it: that machine isn't connected", false))
        );
    }

    #[test]
    fn keys_in_the_same_burst_as_a_sent_message_are_dropped_and_escape_never_quits() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_rex(claude("s1", 0)))),
            now,
        );
        model.selected = 3;
        let pasted = [
            key(Key::Enter),
            key(Key::Char('h')),
            key(Key::Enter),
            key(Key::Char('q')),
            key(Key::Char('m')),
        ];
        let flow = handle_batch(pasted.into_iter(), &mut model, &mut links).unwrap();
        assert!(flow == Flow::Continue);
        assert_eq!(model.pending_move, None);
        let flow = handle_batch(
            [key(Key::Escape), key(Key::Escape)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert!(flow == Flow::Continue);
    }

    #[test]
    fn after_a_move_the_selection_follows_the_session_to_its_new_machine() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.apply(homelab, live_sample(1_000, Some(claude("other", 0))), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        model.follow("s1".into(), Endpoint::Local, "homelab".into(), now);
        model.apply(Endpoint::Local, live_sample(1_000, None), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        assert_eq!(model.selected_agent(now).map(|_| ()), None);
        assert_eq!(model.rows(now)[model.selected].name, "homelab");
        let mut arrived = claude("s1", 0);
        arrived.title = "moved".into();
        model.apply(homelab, live_sample(1_000, Some(arrived)), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        let selected = model.selected_agent(now).unwrap();
        assert_eq!(
            (selected.session.id.as_str(), selected.endpoint),
            ("s1", homelab)
        );
    }

    #[test]
    fn a_moved_session_shows_the_ports_its_old_machine_forwards_to_it() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        let LinkUpdate::Sample(mut here) = live_sample(1_000, None) else {
            unreachable!()
        };
        here.forwards = vec![
            crate::proto::Forward {
                session: "s1".into(),
                host: "homelab".into(),
                ssh: "homelab".into(),
                ports: vec![3000],
                state: crate::proto::ForwardState::Waiting,
            },
            crate::proto::Forward {
                session: "s1".into(),
                host: "nas".into(),
                ssh: "nas".into(),
                ports: vec![4000],
                state: crate::proto::ForwardState::Connected,
            },
        ];
        model.apply(Endpoint::Local, LinkUpdate::Sample(here), now);
        model.apply(homelab, live_sample(1_000, Some(claude("s1", 0))), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        model.selected = 3;
        assert_eq!(model.selected_agent(now).unwrap().session.id, "s1");
        let shown = screen(&model);
        assert!(shown.contains("localhost:3000 waiting"), "{shown}");
        assert!(!shown.contains("localhost:4000"), "{shown}");
    }

    #[test]
    fn the_picker_lists_active_projects_first_filters_as_you_type_and_opens_the_choice() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut active = claude("s1", 0);
        active.project = "web".into();
        model.apply(Endpoint::Local, live_sample(1_000, Some(active)), now);
        model.picker = Some(model::Picker::new(Endpoint::Local, "mac".into()));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Folders(vec![
                "/home/me/api".into(),
                "/home/me/docs".into(),
                "/home/me/web".into(),
            ]),
            now,
        );
        let recent = model.recent_projects(Endpoint::Local, now);
        assert_eq!(
            model.picker.as_ref().unwrap().shown(&recent),
            ["/home/me/web", "/home/me/api", "/home/me/docs"]
        );
        let keys = [
            key(Key::Char('d')),
            key(Key::Char('q')),
            key(Key::Backspace),
            key(Key::Down),
            key(Key::Tab),
        ];
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        let picker = model.picker.clone().unwrap();
        assert_eq!(
            (picker.filter.as_str(), picker.selected, picker.codex),
            ("d", 0, true)
        );
        assert_eq!(picker.shown(&recent), ["/home/me/docs"]);
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.picker, None);
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("couldn't open it: that machine isn't connected", false))
        );
        model.picker = Some(model::Picker::new(Endpoint::Local, "mac".into()));
        handle_batch([key(Key::Escape)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.picker, None);
    }

    #[test]
    fn r_asks_the_agent_to_continue_a_session_outside_rex_and_skips_one_inside() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.selected = 3;
        handle_batch([key(Key::Char('r'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some((
                "couldn't continue it in Rex: that machine isn't connected",
                false
            ))
        );
        let (mut model, mut links) = fleet();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_rex(claude("s1", 0)))),
            now,
        );
        model.selected = 3;
        handle_batch([key(Key::Char('r'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.flashed(Instant::now()), None);
    }

    fn in_project(id: &str, project: &str) -> AgentSession {
        let mut session = claude(id, 0);
        session.project = project.into();
        session.title = id.into();
        session
    }

    #[test]
    fn a_session_magi_opened_is_selected_when_it_appears_and_a_failed_open_selects_nothing() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        model.apply(
            homelab,
            live_sample(1_000, Some(in_project("old", "api"))),
            now,
        );
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        model.await_session(homelab, "api", now);
        let LinkUpdate::Sample(mut sample) = live_sample(1_000, Some(in_project("old", "api")))
        else {
            unreachable!()
        };
        sample.sessions.push(in_project("new", "api"));
        model.apply(homelab, LinkUpdate::Sample(sample.clone()), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        assert_eq!(model.selected_agent(now).unwrap().session.id, "new");

        let (mut model, mut links) = fleet();
        model.apply(
            homelab,
            live_sample(1_000, Some(in_project("old", "api"))),
            now,
        );
        model.await_session(homelab, "api", now);
        model.apply(
            homelab,
            LinkUpdate::Reply(crate::proto::Reply {
                ok: false,
                text: "couldn't open a session: no".into(),
            }),
            now,
        );
        model.apply(homelab, LinkUpdate::Sample(sample), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        assert_ne!(
            model
                .selected_agent(now)
                .map(|agent| agent.session.id.clone()),
            Some("new".into())
        );
    }

    #[test]
    fn the_picker_says_why_it_has_no_folders_and_filters_on_the_project_part_only() {
        let (mut model, _) = fleet();
        let now = Instant::now();
        model.picker = Some(model::Picker::new(Endpoint::Local, "mac".into()));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Reply(crate::proto::Reply {
                ok: false,
                text: "couldn't open a session: cielago isn't installed".into(),
            }),
            now,
        );
        assert!(
            screen(&model).contains("cielago isn't installed"),
            "{}",
            screen(&model)
        );
        let mut picker = model::Picker::new(Endpoint::Local, "mac".into());
        picker.folders = Some(vec![
            "/Users/me/code/shop/api".into(),
            "/Users/me/notes".into(),
        ]);
        picker.filter = "users".into();
        assert!(picker.shown(&[]).is_empty());
        picker.filter = "shop".into();
        assert_eq!(picker.shown(&[]), ["/Users/me/code/shop/api"]);
    }
}
