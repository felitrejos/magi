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
mod labels;
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

use std::collections::HashMap;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

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
    Launched(u32, Option<launcher::Choice>),
}

pub use cli::{Cli, run as cli};
pub use feed::run as alerts;
pub use fleet::print as fleet;
pub use follow::run as follow;
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
    model.labels = labels::Labels::open(net::state_dir()?.join("labels.json"));
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
    Label,
    Back,
    Diff,
}

fn action(key: &Key) -> Option<Action> {
    Some(match key {
        Key::Char('q') | Key::Interrupt => Action::Quit,
        Key::Char('k') | Key::Up => Action::Up,
        Key::Char('j') | Key::Down => Action::Down,
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
        Key::Char('P') => Action::Label,
        Key::Escape => Action::Back,
        Key::Char('d') => Action::Diff,
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
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let label = model.machine_label(&choice.machine, &names);
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

fn start_labelling(model: &mut Model, now: Instant) {
    let Some(model::Focus::Machine(machine)) = model.selection(now) else {
        return model.flash("select a machine to label it".into(), false, now);
    };
    let text = model
        .labels
        .machine_label(&machine)
        .unwrap_or_default()
        .to_string();
    model.naming = Some(model::Naming { machine, text });
}

fn edit_naming(model: &mut Model, mut naming: model::Naming, key: Key, now: Instant) {
    match key {
        Key::Char(c) if !c.is_control() => naming.text.push(c),
        Key::Paste(text) => naming.text.extend(text.chars().filter(|c| !c.is_control())),
        Key::Backspace => {
            let last = naming.text.grapheme_indices(true).next_back();
            naming.text.truncate(last.map_or(0, |(start, _)| start));
        }
        Key::Escape | Key::Interrupt => return,
        Key::Enter => return apply_naming(model, naming, now),
        _ => {}
    }
    model.naming = Some(naming);
}

fn apply_naming(model: &mut Model, naming: model::Naming, now: Instant) {
    let machine = naming.machine;
    match model.labels.label_machine(&machine, &naming.text) {
        Ok(()) => {
            let said = match model.labels.machine_label(&machine) {
                Some(label) => format!("{machine} is now labelled {label}"),
                None => format!("{machine} shows its own name again"),
            };
            model.flash(said, true, now);
        }
        Err(error) => model.flash(format!("couldn't save the labels: {error}"), false, now),
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
        Ok(None) => page_command(&diff, title, true),
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

fn page_command(text: &str, title: &str, diff: bool) -> io::Result<Vec<String>> {
    let path = popup_file(text, if diff { "diff" } else { "md" })?;
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
    if diff {
        command.push("--diff".into());
    }
    command.push(path.to_string_lossy().into_owned());
    Ok(command)
}

fn popup_file(text: &str, extension: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = net::state_dir()?.join("popups");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let path = dir.join(format!(
        "{}-{}.{extension}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    Ok(path)
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
    for event in events {
        if handle(event, model, links)? == Flow::Quit {
            return Ok(Flow::Quit);
        }
    }
    model.reselect(Instant::now());
    refresh_launcher(model, links, Instant::now());
    share_labels(model, links);
    Ok(Flow::Continue)
}

fn share_labels(model: &mut Model, links: &Links) {
    let updated = model.labels.updated_ms();
    if updated <= model.shared_sent {
        return;
    }
    let request = proto::Request::ShareLabels(model.labels.shared());
    let sent = links
        .handles
        .keys()
        .filter(|endpoint| {
            model
                .links
                .get(endpoint)
                .is_some_and(|link| link.state == model::LinkState::Live)
        })
        .filter(|endpoint| links.request(**endpoint, &request).is_ok())
        .count();
    if sent > 0 {
        model.shared_sent = updated;
    }
}

fn handle(event: Event, model: &mut Model, links: &mut Links) -> io::Result<Flow> {
    let now = Instant::now();
    match event {
        Event::Input(Input::Key(key)) if let Some(naming) = model.naming.take() => {
            edit_naming(model, naming, key, now);
        }
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
            Some(Action::Label) => start_labelling(model, now),
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
            core_kinds: Vec::new(),
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
            rex: false,
            launch: None,
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
        assert_eq!(
            model.selection(Instant::now()),
            Some(model::Focus::Machine("nas".into()))
        );
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
            parent: None,
            activity: None,
            mode: None,
            queued: None,
            remote: None,
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
        select_first_session(model);
    }

    fn asked_and_failed(model: &Model, reason: &str) -> bool {
        model
            .folders
            .values()
            .any(|folders| matches!(folders, model::Folders::Failed(said) if said.contains(reason)))
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
    fn enter_explains_when_a_session_is_not_in_rex() {
        let (mut model, mut links) = fleet();
        with_session(&mut model, None);
        assert!(!footer(&model).contains("open"));
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("fix login isn't running in a Rex pane", false))
        );
        assert!(footer(&model).contains("fix login isn't running in a Rex pane"));
    }

    #[test]
    fn a_session_shows_its_mode_and_what_is_queued_and_magi_leaves_the_mode_alone() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = in_rex(claude("s1", 0));
        session.state = SessionState::Working { tool: None };
        session.mode = Some(crate::proto::PermissionMode::Plan);
        session.queued = Some("then commit".into());
        model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
        select_first_session(&mut model);
        let shown = screen(&model);
        assert!(shown.contains("plan · queued"), "{shown}");
        assert!(!footer(&model).contains("mode"), "{}", footer(&model));
        handle_batch([key(Key::BackTab)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.flashed(Instant::now()), None);
    }

    #[test]
    fn the_footer_offers_to_open_a_session_that_is_in_rex() {
        let (mut model, _) = fleet();
        with_session(
            &mut model,
            Some(RexPane {
                session: "session:1".into(),
                window: "window:2".into(),
                block: "block:3".into(),
            }),
        );
        assert!(
            footer(&model).contains("↵ open in Rex"),
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
            parent: None,
            activity: None,
            mode: None,
            queued: None,
            remote: None,
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
        select_first_session(&mut model);
        let agent = model.selected_agent(now).unwrap();
        assert_eq!(
            model
                .move_suggestion(&agent, now)
                .map(|suggestion| suggestion.target.machine),
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
    fn a_move_is_suggested_when_a_session_weighs_on_a_machine_under_sustained_pressure() {
        struct Machine {
            cpu: u16,
            memory: f32,
            load: f32,
        }
        let quiet = Machine {
            cpu: 1_000,
            memory: 0.3,
            load: 1.0,
        };
        let sample = |machine: &Machine, session: Option<AgentSession>| {
            LinkUpdate::Sample(Box::new(Sample {
                cpu: machine.cpu,
                cores: vec![0; 8],
                load: [machine.load; 3],
                memory: crate::proto::Memory {
                    total: 16 << 30,
                    used: ((16u64 << 30) as f32 * machine.memory) as u64,
                    ..Default::default()
                },
                sessions: session.into_iter().collect(),
                ..Sample::default()
            }))
        };
        let suggestion = |here: Machine, millicores: u32, gigabytes: u64, there: &Machine| {
            let (mut model, _) = fleet();
            let now = Instant::now();
            let mut session = claude("s1", millicores);
            session.usage = Some(crate::proto::Usage {
                cpu: Some(millicores),
                memory: gigabytes << 30,
            });
            model.apply(Endpoint::Local, sample(&here, Some(session)), now);
            let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
            model.apply(homelab, sample(there, None), now);
            select_first_session(&mut model);
            let agent = model.selected_agent(now).unwrap();
            model
                .move_suggestion(&agent, now)
                .map(|suggestion| suggestion.pressure.describe("mac"))
        };
        let busy_cpu = || Machine {
            cpu: 9_000,
            ..quiet
        };
        assert_eq!(
            suggestion(busy_cpu(), 2_000, 0, &quiet).as_deref(),
            Some("mac's CPU has averaged 90% for a minute")
        );
        assert_eq!(suggestion(busy_cpu(), 500, 4, &quiet), None);
        let crowded = Machine {
            cpu: 6_000,
            ..quiet
        };
        assert_eq!(suggestion(busy_cpu(), 2_000, 0, &crowded), None);
        let full = || Machine {
            memory: 0.95,
            ..quiet
        };
        assert_eq!(
            suggestion(full(), 0, 4, &quiet).as_deref(),
            Some("mac's memory is 95% full")
        );
        assert_eq!(suggestion(full(), 3_000, 0, &quiet), None);
        let loaded = Machine {
            load: 12.0,
            ..quiet
        };
        assert_eq!(
            suggestion(loaded, 1_500, 0, &quiet).as_deref(),
            Some("mac has a load of 12 on 8 cores")
        );
        assert_eq!(suggestion(Machine { ..quiet }, 4_000, 8, &quiet), None);
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
        select(&mut model, "b");
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
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
            block: "block:3".into(),
        });
        session
    }

    fn select_first_session(model: &mut Model) {
        let index = model
            .board(Instant::now())
            .iter()
            .position(|item| matches!(item, model::Focus::Agent(_)))
            .unwrap();
        model.selected = index;
    }

    fn select(model: &mut Model, id: &str) {
        let index = model
            .board(Instant::now())
            .iter()
            .position(|item| *item == model::Focus::Agent(id.into()))
            .unwrap();
        model.selected = index;
    }

    #[test]
    fn a_session_counts_its_running_subagents_whose_work_has_tabs_of_their_own() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut parent = claude("s1", 0);
        parent.children = vec![crate::proto::Child {
            id: "toolu_a".into(),
            kind: crate::proto::ChildKind::Subagent,
            label: "Find the parser".into(),
            detail: "Explore".into(),
            state: crate::proto::ChildState::Running,
            secs: 12,
            task: None,
        }];
        model.apply(Endpoint::Local, live_sample(1_000, Some(parent)), now);
        select_first_session(&mut model);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        let shown = screen(&model);
        assert!(shown.contains("↳ 1"), "{shown}");
    }

    #[test]
    fn a_session_outside_rex_offers_to_continue_in_rex() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        select_first_session(&mut model);
        assert!(screen(&model).contains("not in Rex"), "{}", screen(&model));
        assert!(
            footer(&model).contains("r continue in Rex"),
            "{}",
            footer(&model)
        );
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("run the tests isn't running in a Rex pane", false))
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
        select_first_session(&mut model);
        assert!(!screen(&model).contains("Needs you"));
        handle_batch([key(Key::Char('y'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.flashed(Instant::now()), None);
        let mut asking = in_rex(claude("s1", 0));
        asking.state =
            SessionState::Waiting(crate::proto::Waiting::Permission(Some("Bash".into())));
        model.apply(Endpoint::Local, live_sample(1_000, Some(asking)), now);
        model.reselect(now);
        let shown = screen(&model);
        assert!(
            shown.contains("Needs you") && shown.contains("needs you · Bash"),
            "{shown}"
        );
        assert!(
            footer(&model).contains("y allow   n deny"),
            "{}",
            footer(&model)
        );
        handle_batch([key(Key::Char('n'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("couldn't answer it: that machine isn't connected", false))
        );
    }

    #[test]
    fn escape_never_quits() {
        let (mut model, mut links) = fleet();
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
            crate::proto::Forward {
                session: "gone".into(),
                host: "homelab".into(),
                ssh: "homelab".into(),
                ports: vec![5000],
                state: crate::proto::ForwardState::Connected,
            },
        ];
        model.apply(Endpoint::Local, LinkUpdate::Sample(here), now);
        model.apply(homelab, live_sample(1_000, Some(claude("s1", 0))), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        select_first_session(&mut model);
        assert_eq!(model.selected_agent(now).unwrap().session.id, "s1");
        let shown = screen(&model);
        assert!(shown.contains("localhost:3000 waiting"), "{shown}");
        assert!(!shown.contains("localhost:4000"), "{shown}");
        assert!(!shown.contains("localhost:5000"), "{shown}");
        model.machine_page = Some(model.rows(now)[0].name.clone());
        let shown = screen(&model);
        assert!(
            shown.contains("localhost:5000 → homelab for a session that left"),
            "{shown}"
        );
        assert!(shown.contains("cielago forwards close gone"), "{shown}");
        assert!(!shown.contains("localhost:4000 →"), "{shown}");
    }

    #[test]
    fn the_launcher_groups_a_project_across_machines_lists_active_ones_first_and_starts_the_choice()
    {
        let (mut model, links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_project("s1", "web"))),
            now,
        );
        model.apply(homelab, live_sample(1_000, None), now);
        model
            .folders
            .insert(Endpoint::Local, model::Folders::Asking);
        model.folders.insert(homelab, model::Folders::Asking);
        model.apply(
            Endpoint::Local,
            LinkUpdate::Folders(vec![
                "/home/me/api".into(),
                "/home/me/clones/web".into(),
                "/home/me/web".into(),
            ]),
            now,
        );
        let menu = model.menu(now);
        assert_eq!(menu.machines[1].listing, launcher::Listing::Asking);
        model.apply(
            homelab,
            LinkUpdate::Folders(vec!["/home/felipe/dev/web".into()]),
            now,
        );
        let menu = model.menu(now);
        let projects: Vec<(&str, bool, usize)> = menu
            .projects
            .iter()
            .map(|project| (project.name.as_str(), project.active, project.folders.len()))
            .collect();
        assert_eq!(
            projects,
            [
                ("No project", false, 2),
                ("web", true, 2),
                ("api", false, 1)
            ]
        );
        assert!(menu.projects[0].chat);
        assert_eq!(menu.projects[1].folders[0].dir, "/home/me/web");
        let mut launcher = launcher::Launcher::new(menu);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| view::launcher::float(frame, Some(&launcher)))
            .unwrap();
        let shown = format!("{:?}", terminal.backend().buffer());
        for text in [
            "New session",
            "No project",
            "~/Chats",
            "web",
            "active",
            "suggested",
            "Codex",
        ] {
            assert!(shown.contains(text), "missing {text:?}\n{shown}");
        }
        launcher.key(Key::Tab);
        let launcher::Outcome::Start(choice) = launcher.key(Key::Enter) else {
            panic!("enter should start the chosen session");
        };
        start_chosen(&mut model, &links, choice, now);
        assert!(model.folders.is_empty());
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("couldn't open it: that machine isn't connected", false))
        );
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
        select_first_session(&mut model);
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
        select_first_session(&mut model);
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
    fn p_on_a_machine_labels_it_everywhere_and_an_empty_label_brings_its_name_back() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.selected = 0;
        let name = model.rows(now)[0].name.clone();
        assert!(screen(&model).contains("P label"), "{}", screen(&model));
        let mut keys = vec![key(Key::Char('P'))];
        keys.extend("Studio".chars().map(|c| key(Key::Char(c))));
        keys.push(key(Key::Enter));
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.labels.machine_label(&name), Some("Studio"));
        let shown = screen(&model);
        assert!(shown.contains("Studio"), "{shown}");
        let mut keys = vec![key(Key::Char('P'))];
        keys.extend((0..6).map(|_| key(Key::Backspace)));
        keys.push(key(Key::Enter));
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.labels.machine_label(&name), None);
        assert!(!screen(&model).contains("Studio"));
    }

    #[test]
    fn arrows_walk_machines_and_their_sessions_in_order_and_pick_where_a_move_goes() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        let nas = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3));
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_project("a", "api"))),
            now,
        );
        model.apply(
            homelab,
            live_sample(1_000, Some(in_project("b", "web"))),
            now,
        );
        model.apply(nas, live_sample(1_000, None), now);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        let selected = |model: &Model| {
            model
                .selected_agent(now)
                .map(|agent| agent.session.id.clone())
        };
        handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model).as_deref(), Some("a"));
        handle_batch(
            [key(Key::Down), key(Key::Down)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(selected(&model).as_deref(), Some("b"));
        handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
        handle_batch([key(Key::Char('P'))].into_iter(), &mut model, &mut links).unwrap();
        let Some(model::Naming { machine: name, .. }) = model.naming.take() else {
            panic!("P on a machine should start labelling it");
        };
        assert!(name.contains("nas"), "{name}");
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.machine_page.as_deref(), Some(name.as_str()));
        handle_batch([key(Key::Escape)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.machine_page, None);
        handle_batch(
            [key(Key::First), key(Key::Down)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(selected(&model).as_deref(), Some("a"));
        handle_batch([key(Key::Char('m'))].into_iter(), &mut model, &mut links).unwrap();
        let first = model.pending_move.clone().unwrap().to;
        handle_batch([key(Key::Right)].into_iter(), &mut model, &mut links).unwrap();
        let second = model.pending_move.clone().unwrap().to;
        assert_ne!(first, second);
        assert!(["homelab", "nas"].contains(&second.as_str()), "{second}");
        handle_batch([key(Key::Right)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.pending_move.clone().unwrap().to, first);
    }

    #[test]
    #[ignore = "prints the screen: cargo test -- --ignored --nocapture print_zen"]
    fn print_zen() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = in_rex(claude("s1", 0));
        session.state = crate::proto::SessionState::Working { tool: None };
        session.activity = Some("running the test suite before the commit".into());
        let LinkUpdate::Sample(mut sample) = live_sample(1_000, Some(session)) else {
            unreachable!()
        };
        let mut asking = in_project("s2", "stampr");
        asking.title = "fix the checkout".into();
        asking.state = crate::proto::SessionState::Waiting(crate::proto::Waiting::Permission(
            Some("Bash(pnpm test)".into()),
        ));
        sample.sessions.push(asking);
        sample.sessions.push(in_project("s3", "notes"));
        model.apply(Endpoint::Local, LinkUpdate::Sample(sample), now);
        select_first_session(&mut model);
        handle_batch([key(Key::Char('z'))].into_iter(), &mut model, &mut links).unwrap();
        println!("{}", screen(&model));
    }

    #[test]
    fn the_launcher_says_why_a_machine_listed_no_projects() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        model.apply(homelab, live_sample(1_000, None), now);
        handle_batch([key(Key::Char('c'))].into_iter(), &mut model, &mut links).unwrap();
        assert!(asked_and_failed(
            &model,
            "couldn't ask for its projects: that machine isn't connected"
        ));
        model.folders.insert(homelab, model::Folders::Asking);
        model.apply(
            homelab,
            LinkUpdate::Reply(crate::proto::Reply {
                ok: false,
                text: "cielago isn't installed".into(),
            }),
            now,
        );
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        assert!(asked_and_failed(&model, "cielago isn't installed"));
    }
}
