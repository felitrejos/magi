use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::input::{self, Input, Key};
use super::{style, terminal, units, view};
use crate::net;
use crate::proto::CodexModel;

const CONNECT_WAIT: Duration = Duration::from_secs(10);
const ACCEPT_POLL: Duration = Duration::from_millis(50);
const ROOM: u64 = 2 << 30;
const MAX_LISTED: usize = 8;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Menu {
    pub machines: Vec<Place>,
    pub projects: Vec<Project>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub tag: String,
    pub cpu: f32,
    pub free: u64,
    pub codex_models: Option<Vec<CodexModel>>,
    pub listing: Listing,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Listing {
    Asking,
    Listed,
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Project {
    pub key: String,
    pub name: String,
    pub group: Option<String>,
    pub active: bool,
    pub recent: bool,
    pub chat: bool,
    pub folders: Vec<Folder>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Folder {
    pub machine: usize,
    pub dir: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Choice {
    pub machine: String,
    pub project: Option<String>,
    pub dir: String,
    pub chat: bool,
    pub codex: bool,
    pub model: Option<String>,
}

pub const CLAUDE_MODELS: [(&str, &str); 4] = [
    ("fable", "Fable"),
    ("opus", "Opus"),
    ("sonnet", "Sonnet"),
    ("haiku", "Haiku"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub codex: bool,
    pub model: Option<String>,
    pub label: String,
}

impl Pick {
    fn new(codex: bool, model: Option<&str>, label: &str) -> Self {
        Self {
            codex,
            model: model.map(str::to_string),
            label: label.to_string(),
        }
    }
}

pub fn picks(place: Option<&Place>) -> Vec<Pick> {
    let mut picks = vec![Pick::new(false, None, "default")];
    picks.extend(
        CLAUDE_MODELS
            .iter()
            .map(|(alias, name)| Pick::new(false, Some(alias), name)),
    );
    let models = place
        .and_then(|place| place.codex_models.as_deref())
        .unwrap_or_default();
    let default = models.iter().find(|model| model.default);
    picks.push(Pick::new(
        true,
        None,
        default.map_or("default", |model| model.name.as_str()),
    ));
    picks.extend(
        models
            .iter()
            .filter(|model| !model.default)
            .map(|model| Pick::new(true, Some(&model.id), &model.name)),
    );
    picks
}

impl Menu {
    pub fn offers(&self, choice: &Choice) -> bool {
        self.projects
            .iter()
            .filter(|project| project.chat == choice.chat)
            .flat_map(|project| &project.folders)
            .filter(|folder| folder.dir == choice.dir)
            .map(|folder| &self.machines[folder.machine])
            .filter(|place| place.name == choice.machine)
            .any(|place| {
                picks(Some(place))
                    .iter()
                    .any(|pick| pick.codex == choice.codex && pick.model == choice.model)
            })
    }

    pub fn suggested(&self, project: &Project) -> Option<usize> {
        let place = |folder: &Folder| &self.machines[folder.machine];
        let folders = project.folders.iter().enumerate();
        folders
            .clone()
            .filter(|(_, folder)| place(folder).free >= ROOM)
            .min_by(|(_, a), (_, b)| place(a).cpu.total_cmp(&place(b).cpu))
            .or_else(|| folders.max_by_key(|(_, folder)| place(folder).free))
            .map(|(index, _)| index)
    }

    fn checked(mut self) -> Self {
        let machines = self.machines.len();
        for project in &mut self.projects {
            project.folders.retain(|folder| folder.machine < machines);
        }
        self.projects.retain(|project| !project.folders.is_empty());
        self
    }
}

pub fn load(place: &Place) -> String {
    format!(
        "{} CPU · {} free",
        units::percent(place.cpu).trim(),
        units::bytes(place.free)
    )
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Stay,
    Cancel,
    Start(Choice),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    pub menu: Menu,
    pub filter: String,
    pub selected: usize,
    pub machine: Option<usize>,
    pub codex: bool,
    pub model: Option<String>,
}

impl Launcher {
    pub fn new(menu: Menu) -> Self {
        Self {
            menu: menu.checked(),
            filter: String::new(),
            selected: 0,
            machine: None,
            codex: false,
            model: None,
        }
    }

    pub fn update(&mut self, menu: Menu) {
        let menu = menu.checked();
        if menu == self.menu {
            return;
        }
        let project = self.current().map(|project| project.key.clone());
        let picked = self
            .machine
            .and_then(|_| self.place())
            .map(|place| place.name.clone());
        self.menu = menu;
        let Some(project) = project else {
            return;
        };
        if let Some(index) = self.shown().iter().position(|shown| shown.key == project) {
            self.selected = index;
        }
        self.machine = picked.and_then(|name| {
            self.current()?
                .folders
                .iter()
                .position(|folder| self.menu.machines[folder.machine].name == name)
        });
    }

    pub fn shown(&self) -> Vec<&Project> {
        let mut shown = self.matching();
        shown.truncate(MAX_LISTED);
        shown
    }

    pub fn hidden(&self) -> usize {
        self.matching().len().saturating_sub(MAX_LISTED)
    }

    fn matching(&self) -> Vec<&Project> {
        let filter = self.filter.to_lowercase();
        self.menu
            .projects
            .iter()
            .filter(|project| {
                project.name.to_lowercase().contains(&filter)
                    || project
                        .group
                        .as_ref()
                        .is_some_and(|group| group.to_lowercase().contains(&filter))
                    || project
                        .folders
                        .iter()
                        .any(|folder| project_part(&folder.dir).to_lowercase().contains(&filter))
            })
            .collect()
    }

    pub fn current(&self) -> Option<&Project> {
        self.shown().get(self.selected).copied()
    }

    pub fn folder(&self) -> Option<usize> {
        let project = self.current()?;
        self.machine
            .filter(|index| *index < project.folders.len())
            .or_else(|| self.menu.suggested(project))
    }

    pub fn place(&self) -> Option<&Place> {
        let project = self.current()?;
        let folder = project.folders.get(self.folder()?)?;
        self.menu.machines.get(folder.machine)
    }

    pub fn pick(&self) -> Pick {
        let picks = picks(self.place());
        let same = |pick: &&Pick| pick.codex == self.codex;
        picks
            .iter()
            .filter(same)
            .find(|pick| pick.model == self.model)
            .or_else(|| picks.iter().find(same))
            .cloned()
            .unwrap_or_else(|| Pick::new(self.codex, None, "default"))
    }

    pub fn choice(&self) -> Option<Choice> {
        let project = self.current()?;
        let folder = project.folders.get(self.folder()?)?;
        let pick = self.pick();
        Some(Choice {
            machine: self.menu.machines[folder.machine].name.clone(),
            project: (!project.chat).then(|| project.name.clone()),
            dir: folder.dir.clone(),
            chat: project.chat,
            codex: pick.codex,
            model: pick.model,
        })
    }

    fn step_pick(&mut self, forward: bool) {
        let picks = picks(self.place());
        let at = picks
            .iter()
            .position(|pick| *pick == self.pick())
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % picks.len()
        } else {
            (at + picks.len() - 1) % picks.len()
        };
        self.codex = picks[next].codex;
        self.model = picks[next].model.clone();
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        let count = self.shown().len();
        match key {
            Key::Char(c) if !c.is_control() => self.refilter(|filter| filter.push(c)),
            Key::Paste(text) => {
                self.refilter(|filter| filter.extend(text.chars().filter(|c| !c.is_control())))
            }
            Key::Backspace => self.refilter(|filter| {
                filter.pop();
            }),
            Key::Up => self.reselect(self.selected.saturating_sub(1)),
            Key::Down => self.reselect((self.selected + 1).min(count.saturating_sub(1))),
            Key::First => self.reselect(0),
            Key::Last => self.reselect(count.saturating_sub(1)),
            Key::Left => self.step_machine(false),
            Key::Right => self.step_machine(true),
            Key::Tab => self.step_pick(true),
            Key::BackTab => self.step_pick(false),
            Key::Escape | Key::Interrupt => return Outcome::Cancel,
            Key::Enter => {
                if let Some(choice) = self.choice() {
                    return Outcome::Start(choice);
                }
            }
            Key::Char(_) | Key::Newline | Key::PageUp | Key::PageDown => {}
        }
        Outcome::Stay
    }

    fn refilter(&mut self, edit: impl FnOnce(&mut String)) {
        edit(&mut self.filter);
        self.reselect(0);
    }

    fn reselect(&mut self, index: usize) {
        if index != self.selected {
            self.machine = None;
        }
        self.selected = index;
    }

    fn step_machine(&mut self, forward: bool) {
        let Some(count) = self.current().map(|project| project.folders.len()) else {
            return;
        };
        let Some(at) = self.folder() else {
            return;
        };
        self.machine = Some(if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        });
    }
}

pub const CHAT_FOLDER: &str = "~/Chats";

pub fn project_part(folder: &str) -> &str {
    ["/Users/", "/home/"]
        .iter()
        .find_map(|home| {
            let rest = folder.strip_prefix(home)?;
            Some(&rest[rest.find('/')? + 1..])
        })
        .unwrap_or(folder)
}

pub fn home_relative(folder: &str) -> String {
    let rest = project_part(folder);
    if rest.len() == folder.len() {
        folder.to_string()
    } else {
        format!("~/{rest}")
    }
}

pub struct Host {
    pub id: u32,
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    stream: Option<UnixStream>,
    menu: String,
}

impl Host {
    pub fn start(answered: impl FnOnce(u32, Option<Choice>) + Send + 'static) -> io::Result<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = net::state_dir()?.join(format!("launcher-{}-{id}.sock", std::process::id()));
        let listener = net::bind_local(&path)?;
        listener.set_nonblocking(true)?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let serving = Arc::clone(&shared);
        thread::Builder::new()
            .name("launcher".into())
            .spawn(move || answered(id, serve(&listener, &serving)))?;
        Ok(Self { id, path, shared })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn send(&self, menu: &Menu) {
        let Ok(line) = serde_json::to_string(menu) else {
            return;
        };
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if shared.menu == line {
            return;
        }
        if let Some(stream) = &mut shared.stream {
            let _ = writeln!(stream, "{line}");
        }
        shared.menu = line;
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = shared.stream.take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn serve(listener: &UnixListener, shared: &Mutex<Shared>) -> Option<Choice> {
    let started = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    && started.elapsed() < CONNECT_WAIT =>
            {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => return None,
        }
    };
    stream.set_nonblocking(false).ok()?;
    let reader = stream.try_clone().ok()?;
    {
        let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        if !shared.menu.is_empty() {
            writeln!(stream, "{}", shared.menu).ok()?;
        }
        shared.stream = Some(stream);
    }
    let line = BufReader::new(reader).lines().next()?.ok()?;
    serde_json::from_str(&line).ok()
}

enum Message {
    Input(Input),
    Menu(Menu),
    Closed,
}

pub fn run(socket: &Path) -> io::Result<()> {
    let stream = UnixStream::connect(socket)?;
    let mut answer = stream.try_clone()?;
    let mut terminal = ratatui::try_init()?;
    let reply = terminal::probe(&mut io::stdout())?;
    style::update(&reply);
    let (sender, messages) = mpsc::channel();
    let early = terminal::unanswered(&reply).to_vec();
    let keys = sender.clone();
    thread::Builder::new().name("input".into()).spawn(move || {
        let _ = input::read(&early, |input| keys.send(Message::Input(input)).is_ok());
    })?;
    thread::Builder::new().name("menu".into()).spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(menu) = line.map(|line| serde_json::from_str::<Menu>(&line)) else {
                break;
            };
            let Ok(menu) = menu else {
                break;
            };
            if sender.send(Message::Menu(menu)).is_err() {
                return;
            }
        }
        let _ = sender.send(Message::Closed);
    })?;
    let mut launcher: Option<Launcher> = None;
    let result = (|| -> io::Result<()> {
        loop {
            terminal.draw(|frame| view::launcher::float(frame, launcher.as_ref()))?;
            let outcome = match messages.recv() {
                Ok(Message::Menu(menu)) => {
                    match &mut launcher {
                        Some(launcher) => launcher.update(menu),
                        None => launcher = Some(Launcher::new(menu)),
                    }
                    Outcome::Stay
                }
                Ok(Message::Input(Input::Key(key))) => match &mut launcher {
                    Some(launcher) => launcher.key(key),
                    None if matches!(key, Key::Escape | Key::Interrupt) => Outcome::Cancel,
                    None => Outcome::Stay,
                },
                Ok(Message::Input(Input::Closed) | Message::Closed) | Err(_) => Outcome::Cancel,
                Ok(Message::Input(_)) => Outcome::Stay,
            };
            match outcome {
                Outcome::Stay => {}
                Outcome::Cancel => return Ok(()),
                Outcome::Start(choice) => {
                    let line = serde_json::to_string(&choice).map_err(io::Error::other)?;
                    return writeln!(answer, "{line}");
                }
            }
        }
    })();
    ratatui::try_restore()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, cpu: f32, free_gb: u64) -> Place {
        Place {
            name: name.into(),
            tag: name.into(),
            cpu,
            free: free_gb << 30,
            codex_models: Some(vec![
                CodexModel {
                    id: "gpt-6-luna".into(),
                    name: "GPT-6-Luna".into(),
                    default: true,
                },
                CodexModel {
                    id: "gpt-5.6-terra".into(),
                    name: "GPT-5.6-Terra".into(),
                    default: false,
                },
            ]),
            listing: Listing::Listed,
        }
    }

    fn project(name: &str, active: bool, machines: &[usize]) -> Project {
        Project {
            key: name.into(),
            name: name.into(),
            group: None,
            active,
            recent: active,
            chat: false,
            folders: machines
                .iter()
                .map(|machine| Folder {
                    machine: *machine,
                    dir: format!("/home/me/{machine}/{name}"),
                })
                .collect(),
        }
    }

    fn menu() -> Menu {
        Menu {
            machines: vec![place("mac", 0.6, 4), place("homelab", 0.1, 9)],
            projects: vec![
                project("magi", true, &[0, 1]),
                project("stampr", false, &[1]),
                project("notes", false, &[0]),
            ],
        }
    }

    #[test]
    fn the_suggested_machine_is_the_least_busy_with_room_and_otherwise_the_one_with_most_free() {
        let menu = menu();
        assert_eq!(menu.suggested(&menu.projects[0]), Some(1));
        let mut tight = menu.clone();
        tight.machines[1].free = 1 << 30;
        assert_eq!(tight.suggested(&tight.projects[0]), Some(0));
        tight.machines[0].free = 1 << 29;
        assert_eq!(tight.suggested(&tight.projects[0]), Some(1));
        assert_eq!(load(&menu.machines[1]), "10% CPU · 9.0G free");
    }

    #[test]
    fn claude_offers_its_aliases_and_codex_the_machines_own_models_default_first() {
        let menu = menu();
        let labels = |place: Option<&Place>| -> Vec<(bool, String)> {
            picks(place)
                .into_iter()
                .map(|pick| (pick.codex, pick.label))
                .collect()
        };
        let mut expected: Vec<(bool, String)> = ["default", "Fable", "Opus", "Sonnet", "Haiku"]
            .iter()
            .map(|label| (false, label.to_string()))
            .collect();
        expected.push((true, "default".into()));
        assert_eq!(labels(None), expected);
        expected.pop();
        expected.push((true, "GPT-6-Luna".into()));
        expected.push((true, "GPT-5.6-Terra".into()));
        assert_eq!(labels(Some(&menu.machines[0])), expected);
    }

    #[test]
    fn keys_filter_pick_a_machine_and_an_agent_then_start_the_choice() {
        let mut launcher = Launcher::new(menu());
        assert_eq!(launcher.place().unwrap().name, "homelab");
        assert_eq!(launcher.key(Key::Right), Outcome::Stay);
        assert_eq!(launcher.place().unwrap().name, "mac");
        launcher.key(Key::Right);
        assert_eq!(launcher.place().unwrap().name, "homelab");
        launcher.key(Key::Left);
        launcher.key(Key::Tab);
        assert_eq!(launcher.pick().label, "Fable");
        launcher.key(Key::BackTab);
        launcher.key(Key::BackTab);
        assert_eq!(
            launcher.key(Key::Enter),
            Outcome::Start(Choice {
                machine: "mac".into(),
                project: Some("magi".into()),
                dir: "/home/me/0/magi".into(),
                chat: false,
                codex: true,
                model: Some("gpt-5.6-terra".into()),
            })
        );
        launcher.key(Key::Tab);
        assert_eq!(launcher.pick(), Pick::new(false, None, "default"));
        launcher.key(Key::Down);
        assert_eq!(launcher.place().unwrap().name, "homelab");
        for c in "note".chars() {
            launcher.key(Key::Char(c));
        }
        let shown: Vec<&str> = launcher.shown().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(shown, ["notes"]);
        launcher.key(Key::Char('z'));
        assert_eq!(launcher.key(Key::Enter), Outcome::Stay);
        assert_eq!(launcher.key(Key::Escape), Outcome::Cancel);
    }

    #[test]
    fn a_new_menu_keeps_the_chosen_project_and_machine_and_drops_folders_on_unknown_machines() {
        let mut launcher = Launcher::new(menu());
        launcher.key(Key::Right);
        let mut more = menu();
        more.projects.insert(0, project("api", true, &[1]));
        more.projects.push(project("ghost", false, &[7]));
        launcher.update(more);
        assert_eq!(launcher.current().unwrap().name, "magi");
        assert_eq!(launcher.place().unwrap().name, "mac");
        assert!(launcher.menu.projects.iter().all(|p| p.name != "ghost"));
    }

    #[test]
    fn only_the_first_few_matches_are_listed_and_the_rest_are_counted() {
        let mut many = menu();
        many.projects = (0..12)
            .map(|index| project(&format!("repo-{index:02}"), false, &[0]))
            .collect();
        let mut launcher = Launcher::new(many);
        assert_eq!((launcher.shown().len(), launcher.hidden()), (MAX_LISTED, 4));
        launcher.key(Key::Last);
        assert_eq!(launcher.current().unwrap().name, "repo-07");
        for c in "repo-1".chars() {
            launcher.key(Key::Char(c));
        }
        assert_eq!((launcher.shown().len(), launcher.hidden()), (2, 0));
    }

    #[test]
    fn only_a_folder_the_menu_offered_is_a_valid_choice() {
        let menu = menu();
        let choice = |machine: &str, dir: &str, codex: bool, model: Option<&str>| Choice {
            machine: machine.into(),
            project: None,
            dir: dir.into(),
            chat: false,
            codex,
            model: model.map(str::to_string),
        };
        let stampr = "/home/me/1/stampr";
        assert!(menu.offers(&choice("homelab", stampr, false, None)));
        assert!(menu.offers(&choice("homelab", stampr, false, Some("opus"))));
        assert!(menu.offers(&choice("homelab", stampr, true, Some("gpt-5.6-terra"))));
        assert!(!menu.offers(&choice("homelab", stampr, false, Some("gpt-5.6-terra"))));
        assert!(!menu.offers(&choice("homelab", stampr, true, Some("--yolo"))));
        assert!(!menu.offers(&choice("mac", stampr, false, None)));
        assert!(!menu.offers(&choice("homelab", "/etc", false, None)));
        let chat = Choice {
            chat: true,
            ..choice("homelab", CHAT_FOLDER, false, None)
        };
        assert!(!menu.offers(&chat));
        let mut with_chat = menu.clone();
        with_chat.projects.push(Project {
            chat: true,
            folders: vec![Folder {
                machine: 1,
                dir: CHAT_FOLDER.into(),
            }],
            ..project("No project", false, &[])
        });
        assert!(with_chat.offers(&chat));
        assert!(!with_chat.offers(&Choice {
            chat: false,
            ..chat.clone()
        }));
    }

    #[test]
    fn the_host_sends_the_menu_to_the_float_and_hears_its_choice() {
        let (sender, answers) = mpsc::channel();
        let host = Host::start(move |id, choice| {
            let _ = sender.send((id, choice));
        })
        .unwrap();
        host.send(&menu());
        let stream = UnixStream::connect(host.path()).unwrap();
        let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
        let first: Menu = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(first, menu());
        let mut changed = menu();
        changed.machines[0].cpu = 0.9;
        host.send(&changed);
        let second: Menu = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(second.machines[0].cpu, 0.9);
        let choice = Choice {
            machine: "homelab".into(),
            project: Some("Stampr".into()),
            dir: "/home/me/1/stampr".into(),
            chat: false,
            codex: false,
            model: None,
        };
        writeln!(&stream, "{}", serde_json::to_string(&choice).unwrap()).unwrap();
        assert_eq!(
            answers.recv_timeout(Duration::from_secs(2)).unwrap(),
            (host.id, Some(choice))
        );
        let path = host.path().to_path_buf();
        drop(host);
        assert!(!path.exists());
    }
}
