mod alerts;
mod base64;
mod browse;
mod chart;
mod cli;
mod client;
mod diacritics;
mod feed;
mod fleet;
mod follow;
mod graphics;
mod input;
mod launcher;
mod link;
mod mcp;
mod model;
mod page;
mod rex;
mod style;
mod terminal;
mod topology;
mod units;
mod view;
mod watch;

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

const MOVED_FOR: Duration = Duration::from_secs(60);
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
    Launched(u32, Option<launcher::Choice>),
}

pub use cli::{Cli, run as cli};
pub use feed::run as alerts;
pub use fleet::print as fleet;
pub use follow::follow;
pub use launcher::run as launch;
pub use mcp::run as mcp;
pub use page::{Page, run as page};

pub fn run(config: Config) -> io::Result<()> {
    if !rex::available() {
        return Err(io::Error::other(
            "magi runs in a Rex pane (https://www.superlogical.com/rex). Open it there, or use `magi fleet --json` for a one-off look.",
        ));
    }
    let (sender, events) = mpsc::sync_channel(EVENT_BACKLOG);
    let mut links = Links {
        handles: HashMap::new(),
        next_generation: 0,
        port: config.port,
        socket: net::local_socket()?,
        sender: sender.clone(),
        rex: true,
        launch: None,
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
    rex: bool,
    launch: Option<launcher::Host>,
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
            close_moved_forwards(&mut model, links, now);
            let mut out = io::stdout();
            execute!(out, BeginSynchronizedUpdate)?;
            let drawn = terminal
                .draw(|frame| view::render(frame, &model, now, graphics))
                .map(drop)
                .and_then(|()| graphics.flush(&mut out))
                .and_then(|()| out.flush());
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
        if let Some(cell) = model.cell_size.take() {
            graphics.set_cell(cell);
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
    if !links.rex {
        return Ok(());
    }
    let elsewhere = (agent.endpoint != model::Endpoint::Local).then(|| agent.machine.clone());
    model.opening = true;
    let sender = links.sender.clone();
    thread::Builder::new().name("open".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::open(&pane, elsewhere.as_deref())));
    })?;
    Ok(())
}

fn propose_move(model: &mut Model, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
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
    Up,
    Down,
    First,
    Last,
    Alerts,
    Move,
    Enter,
    Allow,
    Deny,
    Choose(u8),
    New,
    Stop,
    Adopt,
    Back,
    Diff,
}

fn action(key: &Key) -> Option<Action> {
    Some(match key {
        Key::Char('q') | Key::Interrupt => Action::Quit,
        Key::Char('u') | Key::Up => Action::Up,
        Key::Char('d') | Key::Down => Action::Down,
        Key::Char('g') | Key::First => Action::First,
        Key::Char('G') | Key::Last => Action::Last,
        Key::Char('a') => Action::Alerts,
        Key::Enter => Action::Enter,
        Key::Char('m') => Action::Move,
        Key::Char('y') => Action::Allow,
        Key::Char('n') => Action::Deny,
        Key::Char('c') => Action::New,
        Key::Char('x') => Action::Stop,
        Key::Char('r') => Action::Adopt,
        Key::Escape => Action::Back,
        Key::Char('v') => Action::Diff,
        Key::Char(digit @ '1'..='9') => Action::Choose(*digit as u8 - b'0'),
        _ => return None,
    })
}

fn wheel(model: &mut Model, up: bool, now: Instant) {
    step(model, if up { -1 } else { 1 }, now);
}

fn open_launcher(model: &mut Model, links: &mut Links, now: Instant) {
    if links.launch.is_some() {
        return;
    }
    let live: Vec<Endpoint> = model
        .rows(now)
        .iter()
        .filter(|row| row.state() == Some(&model::LinkState::Live))
        .map(|row| row.endpoint)
        .collect();
    if live.is_empty() {
        return model.flash(
            "no machine is connected to start a session on".into(),
            false,
            now,
        );
    }
    model.end_launch();
    for endpoint in live {
        let asked = match links.request(endpoint, &proto::Request::Folders) {
            Ok(()) => model::Folders::Asking,
            Err(error) => model::Folders::Failed(format!("couldn't ask for its projects: {error}")),
        };
        model.folders.insert(endpoint, asked);
    }
    if !links.rex {
        return;
    }
    let sender = links.sender.clone();
    let host = match launcher::Host::start(move |id, choice| {
        let _ = sender.send(Event::Launched(id, choice));
    }) {
        Ok(host) => host,
        Err(error) => {
            model.end_launch();
            return model.flash(format!("couldn't open the launcher: {error}"), false, now);
        }
    };
    host.send(&model.menu(now));
    let magi = std::env::current_exe().map_or_else(
        |_| "magi".to_string(),
        |path| path.to_string_lossy().into_owned(),
    );
    let command = vec![
        magi,
        "launch".into(),
        host.path().to_string_lossy().into_owned(),
    ];
    links.launch = Some(host);
    let sender = links.sender.clone();
    let _ = thread::Builder::new().name("popup".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::float(
            "magi: new session",
            &command,
            rex::MODAL,
        )));
    });
}

fn start_chosen(model: &mut Model, links: &Links, choice: launcher::Choice, now: Instant) {
    let offered = model.menu(now).offers(&choice);
    model.end_launch();
    if !offered {
        return;
    }
    let rows = model.rows(now);
    let label = choice.machine.clone();
    let Some(endpoint) = rows
        .iter()
        .find(|row| row.name == choice.machine)
        .map(|row| row.endpoint)
    else {
        return;
    };
    let request = proto::Request::Open {
        place: if choice.chat {
            proto::OpenIn::Chats
        } else {
            proto::OpenIn::Folder(choice.dir.clone())
        },
        codex: choice.codex,
        model: choice.model.clone(),
    };
    match links.request(endpoint, &request) {
        Ok(()) => {
            let name = choice.dir.rsplit('/').next().unwrap_or(&choice.dir);
            model.await_session(endpoint, name, now);
            let agent = if choice.codex { "Codex" } else { "Claude" };
            let said = if choice.chat {
                format!("Opening a {agent} chat on {label}…")
            } else {
                format!("Opening {agent} in {name} on {label}…")
            };
            model.flash(said, true, now);
        }
        Err(error) => model.flash(format!("couldn't open it: {error}"), false, now),
    }
}

fn refresh_launcher(model: &mut Model, links: &Links, now: Instant) {
    if let Some(host) = &links.launch {
        host.send(&model.menu(now));
    }
}

fn step(model: &mut Model, delta: isize, now: Instant) {
    model.move_selection(delta, now);
    if model.machine_page.is_some() {
        model.machine_page = match model.selection(now) {
            Some(model::Focus::Machine(name)) => Some(name),
            _ => None,
        };
    }
}

fn ask_for_diff(model: &mut Model, links: &Links, now: Instant) {
    let found = match model.selection(now) {
        Some(model::Focus::Agent(id)) => model
            .agents(now)
            .into_iter()
            .find(|agent| agent.session.id == id)
            .map(|agent| {
                (
                    agent.endpoint,
                    agent.session.id.clone(),
                    agent.session.title.clone(),
                )
            }),
        _ => None,
    };
    let Some((endpoint, session, title)) = found else {
        return model.flash("select a session first".into(), false, now);
    };
    let request = proto::Request::Diff { session };
    match links.request(endpoint, &request) {
        Ok(()) => model.flash(format!("Getting {title}'s changes…"), true, now),
        Err(error) => model.flash(format!("couldn't ask for it: {error}"), false, now),
    }
}

fn open_popup(model: &mut Model, links: &Links, shown: proto::Shown, now: Instant) {
    let diff = match shown.diff {
        Ok(diff) => diff,
        Err(reason) => return model.flash(reason, false, now),
    };
    if !links.rex {
        return;
    }
    let popups = match net::state_dir() {
        Ok(state) => state.join("popups"),
        Err(error) => return model.flash(format!("couldn't save it to show: {error}"), false, now),
    };
    let title = "Changes";
    let command = match browse::diff(&diff, title, &popups) {
        Ok(Some(command)) => Ok(command),
        Ok(None) => page_command(&diff, title, &popups),
        Err(error) => Err(error),
    };
    let command = match command {
        Ok(command) => command,
        Err(error) => return model.flash(format!("couldn't save it to show: {error}"), false, now),
    };
    let label = format!("magi: {title}");
    let sender = links.sender.clone();
    let _ = thread::Builder::new().name("popup".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::float(&label, &command, rex::POPUP)));
    });
}

fn open_alerts(links: &Links) {
    if !links.rex {
        return;
    }
    let command = vec![
        std::env::current_exe().map_or_else(
            |_| "magi".to_string(),
            |path| path.to_string_lossy().into_owned(),
        ),
        "alerts".into(),
    ];
    let sender = links.sender.clone();
    let _ = thread::Builder::new().name("popup".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::float(
            "magi: alerts",
            &command,
            rex::POPUP,
        )));
    });
}

fn page_command(text: &str, title: &str, popups: &Path) -> io::Result<Vec<String>> {
    let path = browse::fresh_dir(popups)?.join("changes.diff");
    browse::write_private(&path, text.as_bytes())?;
    let magi = std::env::current_exe().map_or_else(
        |_| "magi".to_string(),
        |path| path.to_string_lossy().into_owned(),
    );
    let mut command = vec![
        magi,
        "page".into(),
        "--remove".into(),
        "--title".into(),
        title.to_string(),
    ];
    command.push(path.to_string_lossy().into_owned());
    Ok(command)
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
    let tool = match &agent.session.state {
        proto::SessionState::Waiting(proto::Waiting::Permission(tool)) => tool.clone(),
        proto::SessionState::Waiting(proto::Waiting::Trust) => None,
        _ => return,
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
        tool,
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

fn close_moved_forwards(model: &mut Model, links: &Links, now: Instant) {
    let found = model.moved_forwards(now);
    model.moved.retain(|key, _| found.contains(key));
    for key in found {
        let seen = model.moved.entry(key.clone()).or_insert(Some(now));
        if let Some(since) = *seen
            && now.duration_since(since) >= MOVED_FOR
        {
            let request = proto::Request::CloseForward {
                session: key.1.clone(),
            };
            if links.request(key.0, &request).is_ok() {
                *seen = None;
            }
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
    for event in events {
        if handle(event, model, links)? == Flow::Quit {
            return Ok(Flow::Quit);
        }
    }
    model.reselect(Instant::now());
    refresh_launcher(model, links, Instant::now());
    Ok(Flow::Continue)
}

fn handle(event: Event, model: &mut Model, links: &mut Links) -> io::Result<Flow> {
    let now = Instant::now();
    match event {
        Event::Input(Input::Key(key)) if let Some(mut pending) = model.pending_move.take() => {
            match key {
                Key::Char('y') => confirm_move(model, links, pending, now),
                Key::Left | Key::Right => {
                    let targets: Vec<String> = model
                        .rows(now)
                        .iter()
                        .filter(|row| {
                            row.name != pending.from && row.state() == Some(&model::LinkState::Live)
                        })
                        .map(|row| row.name.clone())
                        .collect();
                    if let Some(at) = targets.iter().position(|name| *name == pending.to) {
                        let step = if key == Key::Right {
                            1
                        } else {
                            targets.len() - 1
                        };
                        pending.to = targets[(at + step) % targets.len()].clone();
                    }
                    model.pending_move = Some(pending);
                }
                _ => {}
            }
        }
        Event::Input(Input::Key(key)) => match action(&key) {
            Some(Action::Quit) => return Ok(Flow::Quit),
            Some(Action::Up) => step(model, -1, now),
            Some(Action::Down) => step(model, 1, now),
            Some(Action::First) => step(model, isize::MIN, now),
            Some(Action::Last) => step(model, isize::MAX, now),
            Some(Action::Back) => model.machine_page = None,
            Some(Action::Alerts) => open_alerts(links),
            Some(Action::Move) => propose_move(model, now),
            Some(Action::Enter) => match model.selection(now) {
                Some(model::Focus::Machine(name)) => {
                    model.machine_page = match model.machine_page {
                        Some(_) => None,
                        None => Some(name),
                    };
                }
                Some(model::Focus::Agent(_)) => open_selected(model, links, now)?,
                None => {}
            },
            Some(Action::Allow) => answer(model, links, true, now),
            Some(Action::Deny) => answer(model, links, false, now),
            Some(Action::Choose(option)) => choose(model, links, option, now),
            Some(Action::New) => open_launcher(model, links, now),
            Some(Action::Stop) => stop(model, links, now),
            Some(Action::Adopt) => adopt(model, links, now),
            Some(Action::Diff) => ask_for_diff(model, links, now),
            None => {}
        },
        Event::Launched(id, choice) => {
            if links.launch.as_ref().is_some_and(|host| host.id == id) {
                links.launch = None;
                match choice {
                    Some(choice) => start_chosen(model, links, choice, now),
                    None => model.end_launch(),
                }
            }
        }
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
        Event::Input(Input::Resize) => write_terminal(terminal::CELL_SIZE)?,
        Event::Input(Input::Cells { width, height }) => model.cell_size = Some((width, height)),
        Event::Input(Input::Closed) => return Ok(Flow::Quit),
        Event::Input(Input::Click { .. }) => {}
        Event::Input(Input::Wheel { up, .. }) => wheel(model, up, now),
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
        Event::Link(endpoint, generation, LinkUpdate::Shown(shown)) => {
            if links.is_current(endpoint, generation) {
                open_popup(model, links, shown, now);
            }
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
mod tests;
