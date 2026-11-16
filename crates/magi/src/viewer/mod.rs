mod alerts;
mod base64;
mod browse;
mod chart;
mod diacritics;
mod feed;
mod fleet;
mod graphics;
mod input;
mod launcher;
mod link;
mod model;
mod namer;
mod page;
mod projects;
mod rex;
mod style;
mod symbols;
mod terminal;
mod topology;
mod units;
mod view;

use std::collections::{BTreeMap, HashMap};
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

const WHEEL_LINES: isize = 3;
const THUMBNAIL_PIXELS: &str = "1200";
const THUMBNAIL_WITHIN: Duration = Duration::from_secs(10);
const LEARN_EXISTING_FOR: Duration = Duration::from_secs(30);
const ASK_NAMER_EVERY: Duration = Duration::from_secs(60);
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
    Named(Vec<String>, Result<BTreeMap<String, String>, String>),
    Topology(Topology),
    Link(Endpoint, u64, LinkUpdate),
    Launched(u32, Option<launcher::Choice>),
}

pub use feed::run as alerts;
pub use fleet::print as fleet;
pub use launcher::run as launch;
pub use page::{Page, run as page};

pub fn print_document(path: &std::path::Path, width: usize) -> io::Result<()> {
    if let Ok(palette) = std::env::var("MAGI_PALETTE") {
        style::adopt_palette(&palette);
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let shown = match extension.as_str() {
        "md" | "markdown" | "mdx" => {
            view::markdown_ansi(&std::fs::read_to_string(path)?, width.max(20))
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "pdf" | "html" => {
            match picture(path, &extension) {
                Some(png) => picture_lines(&png, path, width),
                None if extension == "svg" || extension == "html" => {
                    plain(&std::fs::read_to_string(path)?)
                }
                None => format!("{name}\n\n↵ opens it\n"),
            }
        }
        _ => plain(&std::fs::read_to_string(path)?),
    };
    let mut out = io::stdout().lock();
    out.write_all(shown.as_bytes())?;
    out.flush()
}

fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

fn picture(path: &std::path::Path, extension: &str) -> Option<Vec<u8>> {
    if extension == "png" {
        return std::fs::read(path).ok();
    }
    let path = path.canonicalize().ok()?;
    let dir = path.parent()?;
    let thumbnail = dir.join(format!("{}.png", path.file_name()?.to_string_lossy()));
    if !thumbnail.exists() {
        let quick_look = crate::handoff::find("qlmanage", &[PathBuf::from("/usr/bin")])?;
        let _ = crate::bounded::output(
            std::process::Command::new(quick_look)
                .args(["-t", "-s", THUMBNAIL_PIXELS, "-o"])
                .arg(dir)
                .arg(&path)
                .stdin(std::process::Stdio::null()),
            THUMBNAIL_WITHIN,
        );
    }
    std::fs::read(thumbnail).ok()
}

fn picture_lines(png: &[u8], path: &std::path::Path, width: usize) -> String {
    let Some((pixels_wide, pixels_tall)) = graphics::png_size(png) else {
        return String::new();
    };
    let lines = std::env::var("FZF_PREVIEW_LINES")
        .ok()
        .and_then(|lines| lines.parse::<usize>().ok())
        .unwrap_or(30)
        .saturating_sub(2)
        .max(4);
    let aspect = f64::from(pixels_tall) / f64::from(pixels_wide);
    let mut cols = width.max(8);
    let mut rows = ((cols as f64 * aspect) / 2.0).round().max(1.0) as usize;
    if rows > lines {
        rows = lines;
        cols = ((rows as f64 * 2.0) / aspect).round().max(1.0) as usize;
    }
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&path, &mut hasher);
    let id = (std::hash::Hasher::finish(&hasher) % 0xff_fffe) as u32 + 1;
    format!(
        "\x1b[2m{pixels_wide}×{pixels_tall}\x1b[0m\n\n{}",
        graphics::inline_png(png, id, cols as u16, rows as u16)
    )
}

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
        floats: true,
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
    floats: bool,
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
    model.host_icons = rex::host_icons();
    model.projects = projects::Projects::open(net::state_dir()?.join("projects.json"));
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
            let mut links = String::new();
            let drawn = terminal
                .draw(|frame| {
                    view::render(frame, &model, now, graphics);
                    links = view::hyperlinks(frame.buffer_mut(), model.talk.links());
                })
                .map(drop)
                .and_then(|()| graphics.flush(&mut out))
                .and_then(|()| {
                    out.write_all(links.as_bytes())?;
                    out.flush()
                });
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
    Mode,
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
    Project,
    RenameProject,
    Tools,
    Zen,
    PageUp,
    PageDown,
    Diff,
    Outputs,
}

fn action(key: &Key) -> Option<Action> {
    Some(match key {
        Key::Char('q') | Key::Interrupt => Action::Quit,
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
        Key::BackTab => Action::Mode,
        Key::Char('r') => Action::Adopt,
        Key::Char('p') => Action::Project,
        Key::Char('P') => Action::RenameProject,
        Key::Char('t') => Action::Tools,
        Key::Char('z') => Action::Zen,
        Key::PageUp => Action::PageUp,
        Key::PageDown => Action::PageDown,
        Key::Char('d') => Action::Diff,
        Key::Char('v') => Action::Outputs,
        Key::Char(digit @ '1'..='9') => Action::Choose(*digit as u8 - b'0'),
        _ => return None,
    })
}

fn on_detail(model: &Model, column: u16, row: u16, now: Instant) -> bool {
    let Ok((width, height)) = ratatui::crossterm::terminal::size() else {
        return false;
    };
    let full = ratatui::layout::Rect::new(0, 0, width, height);
    let areas = view::areas(
        full,
        model.rows(now).len(),
        model.plans(now).len(),
        model.agents(now).len(),
    );
    areas
        .detail
        .contains(ratatui::layout::Position::new(column, row))
}

fn click(model: &mut Model, column: u16, row: u16, now: Instant) {
    if on_detail(model, column, row, now) && model.draft.is_none() {
        start_draft(model, now);
    }
}

fn wheel(model: &mut Model, up: bool, column: u16, row: u16, now: Instant) {
    if on_detail(model, column, row, now) {
        model
            .talk
            .scroll(if up { WHEEL_LINES } else { -WHEEL_LINES });
    } else {
        model.move_selection(if up { -1 } else { 1 }, now);
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
    model.draft = Some(model::Draft {
        endpoint: agent.endpoint,
        session: agent.session.id.clone(),
        title: agent.session.title.clone(),
        text: agent.session.queued.clone().unwrap_or_default(),
        had_queued: agent.session.queued.is_some(),
    });
}

fn edit_draft(model: &mut Model, links: &Links, mut draft: model::Draft, key: Key, now: Instant) {
    match key {
        Key::Char(c) => draft.type_in(c.encode_utf8(&mut [0; 4])),
        Key::Paste(text) => draft.type_in(&text),
        Key::Newline => draft.type_in("\n"),
        Key::Backspace => draft.erase(),
        Key::Escape | Key::Interrupt => return,
        Key::Enter if !draft.text.trim().is_empty() || draft.had_queued => {
            let request = proto::Request::Prompt {
                session: draft.session,
                text: draft.text,
            };
            if let Err(error) = links.request(draft.endpoint, &request) {
                model.flash(format!("couldn't send it: {error}"), false, now);
            }
            return;
        }
        Key::Enter
        | Key::Up
        | Key::Down
        | Key::Left
        | Key::Right
        | Key::First
        | Key::Last
        | Key::Tab
        | Key::BackTab
        | Key::PageUp
        | Key::PageDown => {}
    }
    model.draft = Some(draft);
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
    if !links.floats {
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

fn start_naming(model: &mut Model, whole_project: bool, now: Instant) {
    if let Some(row) = model
        .rows(now)
        .get(model.selected)
        .filter(|_| whole_project)
    {
        let name = row.name.clone();
        let text = model
            .projects
            .machine_label(&name)
            .unwrap_or_default()
            .to_string();
        model.naming = Some(model::Naming {
            what: model::Renaming::Machine(name),
            text,
        });
        return;
    }
    if let Some((key, name, _)) = model.selected_folder(now).filter(|_| whole_project) {
        model.naming = Some(model::Naming {
            what: model::Renaming::Project {
                key,
                name: name.clone(),
            },
            text: name,
        });
        return;
    }
    let Some(agent) = model.selected_agent(now) else {
        return model.flash("select a session to change its project".into(), false, now);
    };
    let repo = agent.session.project.clone();
    let (key, name) = model.project_of(&repo, now);
    model.naming = Some(if whole_project {
        model::Naming {
            what: model::Renaming::Project {
                key,
                name: name.clone(),
            },
            text: name,
        }
    } else {
        model::Naming {
            what: model::Renaming::Repo(repo),
            text: String::new(),
        }
    });
}

fn edit_naming(model: &mut Model, mut naming: model::Naming, key: Key, now: Instant) {
    match key {
        Key::Char(c) if !c.is_control() => naming.text.push(c),
        Key::Paste(text) => naming.text.extend(text.chars().filter(|c| !c.is_control())),
        Key::Backspace => {
            let last = naming.text.grapheme_indices(true).next_back();
            naming.text.truncate(last.map_or(0, |(start, _)| start));
        }
        Key::Tab => {
            let names = model.project_names(now);
            if let Some(name) = naming.completion(&names) {
                naming.text = name.to_string();
            }
        }
        Key::Escape | Key::Interrupt => return,
        Key::Enter => return apply_naming(model, naming, now),
        _ => {}
    }
    model.naming = Some(naming);
}

fn apply_naming(model: &mut Model, naming: model::Naming, now: Instant) {
    let name = projects::clean(&naming.text);
    let (saved, said) = match naming.what {
        model::Renaming::Machine(machine) => (
            model.projects.label_machine(&machine, &name),
            if name.is_empty() {
                format!("{machine} shows its own name again")
            } else {
                format!("{machine} is now labelled {name}")
            },
        ),
        model::Renaming::Repo(repo) => (
            model.projects.move_repo(&repo, &name),
            if name.is_empty() {
                format!("{repo} is grouped automatically again")
            } else {
                format!("Moved {repo} to {name}")
            },
        ),
        model::Renaming::Project { key, name: old } => (
            model.projects.rename(&key, &name),
            if name.is_empty() {
                format!("{old} is grouped automatically again")
            } else {
                format!("Renamed {old} to {name}")
            },
        ),
    };
    match saved {
        Ok(()) => model.flash(said, true, now),
        Err(error) => model.flash(format!("couldn't save projects: {error}"), false, now),
    }
}

fn ask_to_show(model: &mut Model, links: &Links, what: proto::Showing, now: Instant) {
    let found = match model.detail_focus(now) {
        Some(model::Focus::Agent(id) | model::Focus::Child(id, _)) => model
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
    let what_text = match what {
        proto::Showing::Diff => "changes",
        proto::Showing::Outputs => "outputs",
    };
    let request = proto::Request::Show { session, what };
    match links.request(endpoint, &request) {
        Ok(()) => model.flash(format!("Getting {title}'s {what_text}…"), true, now),
        Err(error) => model.flash(format!("couldn't ask for it: {error}"), false, now),
    }
}

fn open_popup(model: &mut Model, links: &Links, shown: proto::Shown, now: Instant) {
    let body = match shown.body {
        Ok(body) => body,
        Err(reason) => return model.flash(reason, false, now),
    };
    let popups = match net::state_dir() {
        Ok(state) => state.join("popups"),
        Err(error) => return model.flash(format!("couldn't save it to show: {error}"), false, now),
    };
    let browsed = match &body {
        proto::Showed::Diff(text) => browse::diff(text, &shown.title, &popups),
        proto::Showed::Outputs(files) => {
            browse::outputs(files, &shown.title, &style::palette_env(), &popups)
        }
    };
    let command = match browsed {
        Ok(Some(command)) => Ok(command),
        Ok(None) => match body {
            proto::Showed::Diff(text) => page_command(&text, &shown.title, true),
            proto::Showed::Outputs(files) => {
                let joined: String = files
                    .iter()
                    .filter(|file| std::str::from_utf8(&file.bytes).is_ok())
                    .map(|file| {
                        format!(
                            "# {}\n\n{}\n\n",
                            file.path,
                            String::from_utf8_lossy(&file.bytes)
                        )
                    })
                    .collect();
                page_command(&joined, &shown.title, false)
            }
        },
        Err(error) => Err(error),
    };
    let command = match command {
        Ok(command) => command,
        Err(error) => return model.flash(format!("couldn't save it to show: {error}"), false, now),
    };
    let label = format!("magi: {}", shown.title);
    let sender = links.sender.clone();
    let _ = thread::Builder::new().name("popup".into()).spawn(move || {
        let _ = sender.send(Event::Opened(rex::float(&label, &command, rex::POPUP)));
    });
}

fn open_alerts(links: &Links) {
    if !links.floats {
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

fn cycle_mode(model: &mut Model, links: &Links, now: Instant) {
    let Some(agent) = model.selected_agent(now) else {
        return;
    };
    if agent.session.agent != proto::Agent::ClaudeCode || agent.session.pane.is_none() {
        return;
    }
    let request = proto::Request::CycleMode {
        session: agent.session.id.clone(),
    };
    if let Err(error) = links.request(agent.endpoint, &request) {
        model.flash(format!("couldn't switch its mode: {error}"), false, now);
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
    model.note_shown(Instant::now());
    sync_watch(model, links);
    ask_namer(model, links);
    refresh_launcher(model, links, Instant::now());
    share_projects(model, links);
    Ok(Flow::Continue)
}

fn edit_search(model: &mut Model, key: Key) {
    let Some(search) = model.search.as_mut() else {
        return;
    };
    match key {
        Key::Char(c) if !c.is_control() => {
            search.query.push(c);
            search.index = 0;
        }
        Key::Paste(text) => {
            search
                .query
                .extend(text.chars().filter(|c| !c.is_control()));
            search.index = 0;
        }
        Key::Backspace => {
            search.query.pop();
            search.index = 0;
        }
        Key::Enter => search.typing = false,
        Key::Escape | Key::Interrupt => model.search = None,
        _ => {}
    }
}

fn step_search(model: &mut Model, step: isize) {
    let found = model.talk.matches();
    if let Some(search) = model.search.as_mut()
        && found > 0
    {
        search.index = (search.index as isize + step).rem_euclid(found as isize) as usize;
    }
}

fn share_projects(model: &mut Model, links: &Links) {
    let updated = model.projects.updated_ms();
    if updated <= model.shared_sent {
        return;
    }
    let request = proto::Request::ShareProjects(model.projects.shared());
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

fn ask_namer(model: &mut Model, links: &Links) {
    let now = Instant::now();
    if model.namer_busy {
        return;
    }
    if !model.projects.has_seen_any() {
        if model.since_start(now) >= LEARN_EXISTING_FOR {
            let repos = model.repos(now);
            let _ = model.projects.learn(repos.iter().map(String::as_str));
        }
        return;
    }
    if model
        .namer_asked_at
        .is_some_and(|at| now.duration_since(at) < ASK_NAMER_EVERY)
    {
        return;
    }
    let Some((asked, request)) = model.naming_request(now) else {
        return;
    };
    model.namer_asked_at = Some(now);
    let Some(claude) = crate::handoff::find("claude", &crate::handoff::search_dirs()) else {
        let _ = model.projects.suggest(&asked, BTreeMap::new());
        return;
    };
    let Ok(workdir) = net::state_dir().map(|dir| dir.join("namer")) else {
        return;
    };
    model.namer_busy = true;
    let sender = links.sender.clone();
    let _ = thread::Builder::new().name("namer".into()).spawn(move || {
        let named = namer::ask(&claude, &workdir, &request);
        let _ = sender.send(Event::Named(asked, named));
    });
}

fn named(model: &mut Model, asked: Vec<String>, named: Result<BTreeMap<String, String>, String>) {
    let now = Instant::now();
    model.namer_busy = false;
    let names = named.unwrap_or_default();
    let said: Vec<String> = names
        .iter()
        .filter(|(repo, name)| !repo.eq_ignore_ascii_case(name))
        .map(|(repo, name)| format!("{repo} in {name}"))
        .collect();
    match model.projects.suggest(&asked, names) {
        Ok(()) if !said.is_empty() => {
            let text = format!("Grouped {} · P renames", said.join(", "));
            model.flash(text, true, now);
        }
        Ok(()) => {}
        Err(error) => model.flash(format!("couldn't save projects: {error}"), false, now),
    }
}

fn sync_watch(model: &mut Model, links: &Links) {
    let wanted = model.wanted_watch(Instant::now());
    if wanted == model.watching {
        return;
    }
    if let Some((endpoint, _, _)) = &model.watching {
        let request = proto::Request::Watch {
            session: None,
            child: None,
        };
        let _ = links.request(*endpoint, &request);
    }
    let started = wanted.filter(|(endpoint, session, child)| {
        let request = proto::Request::Watch {
            session: Some(session.clone()),
            child: child.clone(),
        };
        links.request(*endpoint, &request).is_ok()
    });
    model.watch(started);
}

fn handle(event: Event, model: &mut Model, links: &mut Links) -> io::Result<Flow> {
    let now = Instant::now();
    match event {
        Event::Input(Input::Key(key))
            if model.search.as_ref().is_some_and(|search| search.typing) =>
        {
            edit_search(model, key);
        }
        Event::Input(Input::Key(key @ (Key::Char('n' | 'N') | Key::Escape)))
            if model.search.is_some() =>
        {
            match key {
                Key::Escape => model.search = None,
                Key::Char('n') => step_search(model, 1),
                _ => step_search(model, -1),
            }
        }
        Event::Input(Input::Key(Key::Char('/')))
            if !model.projects.zen()
                && model.draft.is_none()
                && model.selected_agent(now).is_some() =>
        {
            model.search = Some(model::Search {
                typing: true,
                ..model::Search::default()
            });
        }
        Event::Input(Input::Key(key)) if let Some(naming) = model.naming.take() => {
            edit_naming(model, naming, key, now);
        }
        Event::Input(Input::Key(key)) if let Some(draft) = model.draft.take() => {
            edit_draft(model, links, draft, key, now);
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
        Event::Input(Input::Key(
            key @ (Key::Up | Key::Down | Key::Left | Key::Right | Key::Char('j' | 'k' | 'h' | 'l')),
        )) if model.projects.zen() && model.naming.is_none() => {
            let (down, across) = match key {
                Key::Up | Key::Char('k') => (-1, 0),
                Key::Down | Key::Char('j') => (1, 0),
                Key::Left | Key::Char('h') => (0, -1),
                _ => (0, 1),
            };
            model.zen_step(down, across, now);
        }
        Event::Input(Input::Key(key)) => match action(&key).filter(|action| {
            !model.projects.zen()
                || !matches!(
                    action,
                    Action::Allow
                        | Action::Deny
                        | Action::Choose(_)
                        | Action::Stop
                        | Action::Adopt
                        | Action::Tools
                        | Action::PageUp
                        | Action::PageDown
                )
        }) {
            Some(Action::Quit) => return Ok(Flow::Quit),
            Some(Action::Up) => model.move_selection(-1, now),
            Some(Action::Down) => model.move_selection(1, now),
            Some(Action::First) => model.move_selection(isize::MIN, now),
            Some(Action::Last) => model.move_selection(isize::MAX, now),
            Some(Action::Alerts) => open_alerts(links),
            Some(Action::Open) => open_selected(model, links, now)?,
            Some(Action::Move) => propose_move(model, now),
            Some(Action::Message) => match (model.selected_project(now), model.selection(now)) {
                (Some(key), _) => {
                    if let Err(error) = model.projects.toggle_fold(&key) {
                        model.flash(format!("couldn't save projects: {error}"), false, now);
                    }
                }
                (None, Some(model::Focus::NewSession)) => open_launcher(model, links, now),
                (None, _) if model.projects.zen() => open_selected(model, links, now)?,
                (None, _) => start_draft(model, now),
            },
            Some(Action::Allow) => answer(model, links, true, now),
            Some(Action::Deny) => answer(model, links, false, now),
            Some(Action::Choose(option)) => choose(model, links, option, now),
            Some(Action::New) => open_launcher(model, links, now),
            Some(Action::Stop) => stop(model, links, now),
            Some(Action::Mode) => cycle_mode(model, links, now),
            Some(Action::Adopt) => adopt(model, links, now),
            Some(Action::Project) => start_naming(model, false, now),
            Some(Action::RenameProject) => start_naming(model, true, now),
            Some(Action::Tools) => model.tools_expanded = !model.tools_expanded,
            Some(Action::Zen) => {
                let zen = !model.projects.zen();
                if let Err(error) = model.projects.set_zen(zen) {
                    model.flash(format!("couldn't save the view: {error}"), false, now);
                }
            }
            Some(Action::PageUp) => model.talk.page(true),
            Some(Action::PageDown) => model.talk.page(false),
            Some(Action::Diff) => ask_to_show(model, links, proto::Showing::Diff, now),
            Some(Action::Outputs) => ask_to_show(model, links, proto::Showing::Outputs, now),
            None => {}
        },
        Event::Named(asked, result) => named(model, asked, result),
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
        Event::Input(Input::Click { column, row }) => click(model, column, row, now),
        Event::Input(Input::Wheel { up, column, row }) => wheel(model, up, column, row, now),
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
            floats: false,
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
        assert_eq!(selected(&model), "nas");
        handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.selection(Instant::now()),
            Some(model::Focus::NewSession)
        );
    }

    #[test]
    fn the_selection_follows_its_machine_when_rows_reorder() {
        let (mut model, mut links) = fleet();
        handle_batch(
            [key(Key::Last), key(Key::Up)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
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
    fn a_busy_session_takes_a_queued_message_and_shift_tab_asks_to_switch_its_mode() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = in_rex(claude("s1", 0));
        session.state = SessionState::Working { tool: None };
        session.mode = Some(crate::proto::PermissionMode::Plan);
        session.queued = Some("then commit".into());
        model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
        select_first_session(&mut model);
        let shown = screen(&model);
        assert!(shown.contains("Queued: then commit"), "{shown}");
        assert!(shown.contains("plan mode"), "{shown}");
        assert!(footer(&model).contains("⇧tab mode"), "{}", footer(&model));
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        let draft = model.draft.clone().unwrap();
        assert_eq!(
            (draft.text.as_str(), draft.had_queued),
            ("then commit", true)
        );
        assert!(screen(&model).contains("↵ queue"), "{}", screen(&model));
        handle_batch([key(Key::Escape)].into_iter(), &mut model, &mut links).unwrap();
        handle_batch([key(Key::BackTab)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.flashed(Instant::now()),
            Some((
                "couldn't switch its mode: that machine isn't connected",
                false
            ))
        );
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
            parent: None,
            activity: None,
            mode: None,
            queued: None,
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
    fn a_rex_session_asking_you_shows_its_screen_until_the_selection_moves() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut session = claude("s1", 0);
        session.state = SessionState::Waiting(crate::proto::Waiting::Permission(None));
        session.pane = Some(RexPane {
            session: "session:1".into(),
            window: "window:2".into(),
            block: "block:3".into(),
        });
        model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
        select_first_session(&mut model);
        assert_eq!(
            model.wanted_watch(now),
            Some((Endpoint::Local, "s1".to_string(), None))
        );
        model.watch(model.wanted_watch(now));
        let screen = |text: &str| {
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                child: None,
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
        model.watch(Some((Endpoint::Local, "s1".into(), None)));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Screen(crate::proto::Screen {
                session: "other".into(),
                child: None,
                view: Err("old".into()),
            }),
            now,
        );
        assert_eq!(model.screen("other", None), None);
        let identity = Identity {
            hostname: "mac".into(),
            tailscale_node: Some("mac".into()),
            os: String::new(),
            kernel: String::new(),
            arch: String::new(),
            cpu_model: String::new(),
            core_kinds: Vec::new(),
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
        select_first_session(&mut model);
        assert_eq!(
            model.wanted_watch(now),
            Some((Endpoint::Local, "s1".to_string(), None))
        );
        model.watch(model.wanted_watch(now));
        let long = "word ".repeat(60);
        model.apply(
            Endpoint::Local,
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                child: None,
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
        let prompt = screen
            .iter()
            .find(|line| line.contains("run the tests") && !line.contains('✻'))
            .unwrap_or_else(|| panic!("{joined}"));
        assert!(
            prompt.find("run the tests").unwrap() > 100,
            "your message sits on the right\n{joined}"
        );
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
        handle_batch(
            [key(Key::Last), key(Key::Up)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
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

    fn groups(model: &Model) -> Vec<(String, Vec<String>)> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for item in model.listed(Instant::now()) {
            let (model::Group::Project(name), model::Entry::Session(agent)) =
                (&item.group, &item.entry)
            else {
                continue;
            };
            match groups.last_mut() {
                Some((last, ids)) if last == name => ids.push(agent.session.id.clone()),
                _ => groups.push((name.clone(), vec![agent.session.id.clone()])),
            }
        }
        for (_, ids) in &mut groups {
            ids.sort();
        }
        groups
    }

    fn select_first_session(model: &mut Model) {
        let now = Instant::now();
        let index = model
            .listed(now)
            .iter()
            .position(|item| matches!(item.entry, model::Entry::Session(_)))
            .unwrap();
        model.selected = model.rows(now).len() + index;
    }

    fn select(model: &mut Model, id: &str) {
        let now = Instant::now();
        let index = model
            .listed(now)
            .iter()
            .position(|item| matches!(&item.entry, model::Entry::Session(agent) if agent.session.id == id))
            .unwrap();
        model.selected = model.rows(now).len() + index;
    }

    #[test]
    fn the_namer_is_asked_only_about_repos_it_has_not_seen_with_their_session_titles() {
        let (mut model, _links) = fleet();
        let now = Instant::now();
        let mut ping = claude("s2", 0);
        ping.project = "ping".into();
        ping.title = "two player pong".into();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        assert!(model.naming_request(now).is_none());
        model.projects.learn(["api"]).unwrap();
        model.apply(
            Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2)),
            live_sample(1_000, Some(ping)),
            now,
        );
        let (asked, request) = model.naming_request(now).unwrap();
        assert_eq!(asked, ["ping"]);
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "projects": [{"name": "api", "repos": ["api"]}],
                "new": [{"repo": "ping", "sessions": ["two player pong"]}],
            })
        );
        named(
            &mut model,
            asked,
            Ok(BTreeMap::from([("ping".into(), "Network Pong".into())])),
        );
        assert!(model.naming_request(now).is_none());
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("Grouped ping in Network Pong · P renames", true))
        );
    }

    #[test]
    fn a_session_started_by_another_sits_nested_right_under_it() {
        let (mut model, _links) = fleet();
        let now = Instant::now();
        let mut child = claude("kid", 0);
        child.parent = Some("s1".into());
        let mut other = claude("s3", 0);
        other.title = "another".into();
        model.apply(
            Endpoint::Local,
            LinkUpdate::Sample(Box::new(Sample {
                cpu: 1_000,
                cores: vec![0; 8],
                sessions: vec![child, other, claude("s1", 0)],
                ..Sample::default()
            })),
            now,
        );
        let order: Vec<(String, bool)> = model
            .listed(now)
            .into_iter()
            .filter_map(|item| match item.entry {
                model::Entry::Session(agent) => Some((agent.session.id.clone(), item.nested)),
                _ => None,
            })
            .collect();
        let kid = order.iter().position(|(id, _)| id == "kid").unwrap();
        assert_eq!(order[kid - 1].0, "s1");
        assert!(order[kid].1 && !order[kid - 1].1);
    }

    #[test]
    fn a_folder_row_keeps_the_pane_and_shows_its_keys_and_new_session_opens_the_launcher() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        select_first_session(&mut model);
        handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
        handle_batch([key(Key::Up)].into_iter(), &mut model, &mut links).unwrap();
        assert!(model.selected_folder(now).is_some());
        let shown = screen(&model);
        assert!(shown.contains("run the tests"), "{shown}");
        assert!(shown.contains("↵ fold   P rename"), "{shown}");
        handle_batch(
            [key(Key::Last), key(Key::Enter)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert!(asked_and_failed(&model, "couldn't ask for its projects"));
    }

    #[test]
    fn selecting_a_subagent_watches_it_and_shows_what_it_was_asked_and_answered() {
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
        handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            model.wanted_watch(now),
            Some((Endpoint::Local, "s1".into(), Some("toolu_a".into())))
        );
        model.watch(model.wanted_watch(now));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Screen(crate::proto::Screen {
                session: "s1".into(),
                child: Some("toolu_a".into()),
                view: Ok(crate::proto::Content::Conversation(vec![
                    crate::proto::Said::You("Where is the config parser?".into()),
                    crate::proto::Said::Reply("In src/config.rs".into()),
                ])),
            }),
            now,
        );
        let shown = screen(&model);
        for text in [
            "◇ Find the parser",
            "subagent ·",
            "from run the tests · Explore",
            "Where is the config parser?",
            "In src/config.rs",
        ] {
            assert!(shown.contains(text), "missing {text:?}\n{shown}");
        }
    }

    #[test]
    fn enter_on_a_project_folds_it_to_a_count_and_opens_it_again() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.selected = model.rows(now).len();
        assert_eq!(model.selected_project(now).as_deref(), Some("api"));
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        let entries: Vec<String> = model
            .listed(now)
            .iter()
            .map(|item| match &item.entry {
                model::Entry::Project {
                    name, count, open, ..
                } => format!("{name} {count} {open}"),
                model::Entry::Session(agent) => agent.session.id.clone(),
                model::Entry::Fold { .. } => "earlier".into(),
                model::Entry::NewSession => "new".into(),
                model::Entry::Child { child, .. } => child.id.clone(),
            })
            .collect();
        assert_eq!(entries, ["api 1 false", "new"]);
        assert_eq!(model.draft, None);
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(groups(&model), [("api".into(), vec!["s1".into()])]);
    }

    #[test]
    fn p_moves_a_repo_to_a_project_and_capital_p_renames_one_onto_another() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        let mut docs = claude("s2", 0);
        docs.project = "api-docs".into();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(claude("s1", 0))),
            now,
        );
        model.apply(
            Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2)),
            live_sample(1_000, Some(docs)),
            now,
        );
        let typed = |text: &str| text.chars().map(|c| key(Key::Char(c))).collect::<Vec<_>>();
        assert_eq!(
            groups(&model),
            [("api".into(), vec!["s1".into(), "s2".into()])]
        );

        select(&mut model, "s2");
        let mut keys = vec![key(Key::Char('p'))];
        keys.extend(typed("Docs"));
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        assert!(screen(&model).contains("Move api-docs to project › Docs▏"));
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            groups(&model),
            [
                ("api".into(), vec!["s1".into()]),
                ("Docs".into(), vec!["s2".into()])
            ]
        );
        assert_eq!(
            model.flashed(Instant::now()),
            Some(("Moved api-docs to Docs", true))
        );

        select(&mut model, "s1");
        let mut keys = vec![key(Key::Char('P'))];
        keys.extend([Key::Backspace, Key::Backspace, Key::Backspace].map(key));
        keys.extend(typed("d"));
        keys.push(key(Key::Tab));
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.naming.as_ref().unwrap().text, "Docs");
        handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(
            groups(&model),
            [("Docs".into(), vec!["s1".into(), "s2".into()])]
        );

        handle_batch(
            [key(Key::Char('p')), key(Key::Char('x')), key(Key::Escape)].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert_eq!(model.naming, None);
        assert_eq!(
            groups(&model),
            [("Docs".into(), vec!["s1".into(), "s2".into()])]
        );
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
        select_first_session(&mut model);
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
        select_first_session(&mut model);
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
        select_first_session(&mut model);
        assert!(!screen(&model).contains("Allow Bash?"));
        handle_batch([key(Key::Char('y'))].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.flashed(Instant::now()), None);
        let mut asking = in_rex(claude("s1", 0));
        asking.state =
            SessionState::Waiting(crate::proto::Waiting::Permission(Some("Bash".into())));
        model.apply(Endpoint::Local, live_sample(1_000, Some(asking)), now);
        model.reselect(now);
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
        select_first_session(&mut model);
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
        model.selected = 0;
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
        assert_eq!(model.projects.machine_label(&name), Some("Studio"));
        let shown = screen(&model);
        assert!(shown.contains("Studio"), "{shown}");
        assert!(shown.contains(&format!("{name} · this machine")), "{shown}");
        let mut keys = vec![key(Key::Char('P'))];
        keys.extend((0..6).map(|_| key(Key::Backspace)));
        keys.push(key(Key::Enter));
        handle_batch(keys.into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(model.projects.machine_label(&name), None);
        assert!(!screen(&model).contains("Studio"));
    }

    #[test]
    fn zen_shows_a_session_as_a_monitor_reads_no_conversation_and_never_types() {
        let (mut model, mut links) = fleet();
        let now = Instant::now();
        model.apply(
            Endpoint::Local,
            live_sample(1_000, Some(in_rex(claude("s1", 0)))),
            now,
        );
        select_first_session(&mut model);
        assert!(model.wanted_watch(now).is_some());
        handle_batch([key(Key::Char('z'))].into_iter(), &mut model, &mut links).unwrap();
        assert!(model.projects.zen());
        assert_eq!(model.wanted_watch(now), None);
        let shown = screen(&model);
        for text in [
            "all quiet",
            "run the tests",
            "no sessions",
            "↵ open in Rex",
            "z full view",
        ] {
            assert!(shown.contains(text), "missing {text:?}\n{shown}");
        }
        assert!(!shown.contains("show tools"), "{shown}");
        handle_batch(
            [key(Key::Enter), key(Key::Char('y'))].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        assert!(model.draft.is_none());
        handle_batch([key(Key::Char('z'))].into_iter(), &mut model, &mut links).unwrap();
        assert!(!model.projects.zen());
    }

    #[test]
    fn zen_arrows_walk_the_board_across_machines_and_pick_where_a_move_goes() {
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
        handle_batch([key(Key::Char('z'))].into_iter(), &mut model, &mut links).unwrap();
        let selected = |model: &Model| {
            model
                .selected_agent(now)
                .map(|agent| agent.session.id.clone())
        };
        handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model).as_deref(), Some("a"));
        handle_batch([key(Key::Right)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model).as_deref(), Some("b"));
        handle_batch([key(Key::Left)].into_iter(), &mut model, &mut links).unwrap();
        assert_eq!(selected(&model).as_deref(), Some("a"));
        handle_batch([key(Key::Up)].into_iter(), &mut model, &mut links).unwrap();
        let machine = model.selection(now);
        assert!(
            matches!(machine, Some(model::Focus::Machine(_))),
            "{machine:?}"
        );
        handle_batch(
            [key(Key::Left), key(Key::Char('P'))].into_iter(),
            &mut model,
            &mut links,
        )
        .unwrap();
        let Some(model::Naming {
            what: model::Renaming::Machine(name),
            ..
        }) = model.naming.take()
        else {
            panic!("P on a machine in Zen should start labelling it");
        };
        assert!(name.contains("nas"), "{name}");
        handle_batch(
            [key(Key::Right), key(Key::Down)].into_iter(),
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
