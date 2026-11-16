use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::collect::{self, Collector, SystemInfo};
use crate::conversation::Reader;
use crate::handoff;
use crate::net::{self, Conn};
use crate::proto::{
    self, AgentMessage, AgentSession, ChildKind, Content, Hello, Identity, PROTOCOL_VERSION,
    PermissionMode, Reply, Request, Said, Screen, SessionState, SharedProjects, Showed, Showing,
    Shown, StatusReport, Welcome,
};
use crate::screen::Capturer;
use crate::sessions::rex::Location;
use crate::sessions::{Board, Sessions, Snapshot};
use crate::{children, log, opener, show, tailscale, typing};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const TAILNET_RETRY: Duration = Duration::from_secs(10);
const TAILNET_CHECK: Duration = Duration::from_secs(30);
const IDENTITY_REFRESH: Duration = Duration::from_secs(60);
const MAX_CONNECTIONS: usize = 32;
const FRAME_BACKLOG: usize = 4;
const HELLO_MAX_BYTES: usize = 8;
const STATUS_MAX_BYTES: usize = 4096;
const REQUEST_MAX_BYTES: usize = 16 * 1024;
const SCREEN_EVERY: Duration = Duration::from_millis(500);
const STATUS_RETRY: Duration = Duration::from_millis(10);
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(1);
const SESSIONS_EVERY: Duration = Duration::from_secs(1);

type Frame = Arc<[u8]>;

pub struct Config {
    pub port: u16,
    pub interval: Duration,
}

pub fn run(config: Config) -> io::Result<()> {
    let collector = Collector::new()?;
    let info = collector.info();
    let board = Board::saved_at(net::state_dir()?.join("plans.json"));
    let agent = Arc::new(Agent::new(
        identity_of(&info),
        config.interval,
        board,
        shared_path().ok(),
    ));
    let socket = net::local_socket()?;
    let local = net::bind_local(&socket)?;
    log::info(format!(
        "magi agent {} sampling every {:?}, local viewers on {}",
        env!("CARGO_PKG_VERSION"),
        config.interval,
        socket.display()
    ));
    let status = net::bind_local(&net::status_socket()?)?;
    spawn_sampler(collector, Arc::clone(&agent))?;
    spawn_sessions(Sessions::for_this_user()?, Arc::clone(&agent))?;
    spawn_identity_refresh(info, Arc::clone(&agent))?;
    {
        let agent = Arc::clone(&agent);
        thread::Builder::new()
            .name("status".into())
            .spawn(move || accept_status(&status, &agent))?;
    }
    {
        let agent = Arc::clone(&agent);
        thread::Builder::new()
            .name("local".into())
            .spawn(move || accept_local(&local, &agent))?;
    }
    serve_tailnet(config.port, &agent)
}

fn identity_of(info: &SystemInfo) -> Identity {
    Identity {
        hostname: collect::hostname(),
        tailscale_node: tailscale::status().ok().map(|status| status.node),
        os: info.os.clone(),
        kernel: info.kernel.clone(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: info.cpu_model.clone(),
        core_kinds: info.core_kinds.clone(),
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn spawn_identity_refresh(info: SystemInfo, agent: Arc<Agent>) -> io::Result<()> {
    thread::Builder::new()
        .name("identity".into())
        .spawn(move || {
            loop {
                thread::sleep(IDENTITY_REFRESH);
                let identity = identity_of(&info);
                *agent
                    .identity
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = identity;
            }
        })?;
    Ok(())
}

#[derive(Default)]
struct Watcher {
    screen: Capturer,
    conversation: Reader,
    subagents: HashMap<String, PathBuf>,
}

type Target = (String, Option<String>);

struct Agent {
    identity: RwLock<Identity>,
    interval: Duration,
    hub: Hub,
    connections: AtomicUsize,
    notices: log::Throttle,
    board: Board,
    sessions: Mutex<Snapshot>,
    fresh: Nudge,
    shared: Mutex<Option<SharedProjects>>,
    shared_file: Option<PathBuf>,
    modes: Mutex<HashMap<String, Switched>>,
    queued: Mutex<HashMap<String, String>>,
}

struct Switched {
    to: PermissionMode,
    transcript_said: Option<PermissionMode>,
}

impl Agent {
    fn remember_mode(&self, session: &AgentSession, to: PermissionMode) {
        let mut modes = self.modes.lock().unwrap_or_else(PoisonError::into_inner);
        let transcript_said = modes
            .get(&session.id)
            .map_or(session.mode, |switched| switched.transcript_said);
        modes.insert(
            session.id.clone(),
            Switched {
                to,
                transcript_said,
            },
        );
    }

    fn queue(
        &self,
        session: &AgentSession,
        pane: Option<&Location>,
        text: &str,
    ) -> Result<String, String> {
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        if text.trim().is_empty() {
            return match queued.remove(&session.id) {
                Some(_) => Ok(format!("Dropped the message queued for {}", session.title)),
                None => Err("write something to send".into()),
            };
        }
        if session.state == SessionState::Idle {
            drop(queued);
            return typing::prompt(session, pane, text);
        }
        if pane.is_none() {
            return typing::prompt(session, pane, text);
        }
        queued.insert(session.id.clone(), text.to_string());
        Ok(format!(
            "Queued for {}: it goes in once the session is idle",
            session.title
        ))
    }

    fn settle(&self, snapshot: &mut Snapshot) {
        let mut modes = self.modes.lock().unwrap_or_else(PoisonError::into_inner);
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        modes.retain(|id, _| snapshot.sessions.iter().any(|session| session.id == *id));
        queued.retain(|id, _| snapshot.sessions.iter().any(|session| session.id == *id));
        for session in &mut snapshot.sessions {
            session.queued = None;
            if let Some(switched) = modes.get(&session.id) {
                if switched.transcript_said == session.mode {
                    session.mode = Some(switched.to);
                } else {
                    modes.remove(&session.id);
                }
            }
            if session.state == SessionState::Idle
                && let Some(text) = queued.remove(&session.id)
            {
                let (found, pane) = (session.clone(), snapshot.panes.get(&session.id).cloned());
                let spawned = thread::Builder::new().name("queued".into()).spawn(move || {
                    if let Err(error) = typing::prompt(&found, pane.as_ref(), &text) {
                        log::info(format!(
                            "couldn't send the message queued for {}: {error}",
                            found.title
                        ));
                    }
                });
                if let Err(error) = spawned {
                    log::info(format!(
                        "could not start a thread for a queued message: {error}"
                    ));
                }
                continue;
            }
            session.queued = queued.get(&session.id).cloned();
        }
    }

    fn show(&self, session: String, what: Showing) -> Shown {
        let (cwd, transcript) = {
            let snapshot = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            (
                snapshot.cwds.get(&session).cloned().unwrap_or_default(),
                snapshot
                    .transcripts
                    .get(&session)
                    .map(|(_, path)| path.clone()),
            )
        };
        let (title, body) = match what {
            Showing::Diff => ("Changes", show::diff(&cwd).map(Showed::Diff)),
            Showing::Outputs => (
                "Outputs",
                transcript
                    .ok_or_else(|| "that session's transcript isn't on this machine".to_string())
                    .and_then(|path| show::outputs(&path, &cwd))
                    .map(Showed::Outputs),
            ),
        };
        Shown {
            session,
            what,
            title: title.to_string(),
            body,
        }
    }

    fn capture_child(
        &self,
        session: &str,
        child: &str,
        watcher: &mut Watcher,
    ) -> Result<Content, String> {
        let (found, transcript) = {
            let snapshot = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            let found = snapshot
                .sessions
                .iter()
                .find(|found| found.id == session)
                .and_then(|found| found.children.iter().find(|known| known.id == child))
                .map(|known| (known.kind, known.task.clone()));
            (found, snapshot.transcripts.get(session).cloned())
        };
        let (kind, task) = found.ok_or("that has finished and isn't listed any more")?;
        let (_, parent) = transcript.ok_or("that session's transcript isn't on this machine")?;
        match kind {
            ChildKind::Subagent => {
                let path = match watcher.subagents.get(child) {
                    Some(path) => path.clone(),
                    None => {
                        let path = children::subagent_transcript(&parent, child)
                            .ok_or("its transcript isn't there yet")?;
                        watcher.subagents.insert(child.to_string(), path.clone());
                        path
                    }
                };
                watcher
                    .conversation
                    .read(&path, proto::Agent::ClaudeCode)
                    .map(Content::Conversation)
            }
            ChildKind::Shell => {
                let task = task.ok_or("it hasn't started yet")?;
                let session_id = parent
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or("that session's transcript has an odd name")?;
                children::shell_output(session_id, &task).map(|output| {
                    Content::Conversation(vec![Said::Reply(format!("````\n{output}\n````"))])
                })
            }
            ChildKind::Task => Err("cielago keeps this task's result: run cielago tasks".into()),
        }
    }

    fn capture(
        &self,
        session: &str,
        child: Option<&str>,
        watcher: &mut Watcher,
    ) -> Result<Content, String> {
        if let Some(child) = child {
            return self.capture_child(session, child, watcher);
        }
        let (pane, transcript, asking) = {
            let snapshot = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            let asking = snapshot
                .sessions
                .iter()
                .any(|found| found.id == session && found.state.is_waiting());
            (
                snapshot.panes.get(session).cloned(),
                snapshot.transcripts.get(session).cloned(),
                asking,
            )
        };
        match (pane, transcript) {
            (Some(pane), _) if asking => watcher
                .screen
                .capture(&pane, Instant::now())
                .map(Content::Screen),
            (_, Some((agent, path))) => watcher
                .conversation
                .read(&path, agent)
                .map(Content::Conversation),
            (Some(pane), None) => watcher
                .screen
                .capture(&pane, Instant::now())
                .map(Content::Screen),
            (None, None) => Err("that session has ended".into()),
        }
    }

    fn new(
        identity: Identity,
        interval: Duration,
        board: Board,
        shared_file: Option<PathBuf>,
    ) -> Self {
        Self {
            identity: RwLock::new(identity),
            interval,
            hub: Hub::default(),
            connections: AtomicUsize::new(0),
            notices: log::Throttle::default(),
            board,
            sessions: Mutex::new(Snapshot::default()),
            fresh: Nudge::default(),
            shared: Mutex::new(shared_file.as_deref().and_then(load_shared)),
            shared_file,
            modes: Mutex::default(),
            queued: Mutex::default(),
        }
    }

    fn share_projects(&self, shared: SharedProjects) {
        {
            let mut known = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            if known
                .as_ref()
                .is_some_and(|known| known.updated_ms >= shared.updated_ms)
            {
                return;
            }
            *known = Some(shared.clone());
        }
        if let Some(path) = &self.shared_file
            && let Err(error) = save_shared(path, &shared)
        {
            self.notices
                .info(format!("couldn't keep the shared projects: {error}"));
        }
        if let Ok(frame) = proto::encode_frame(&AgentMessage::Projects(shared)) {
            self.hub.publish(frame.into());
        }
    }

    fn welcome(&self) -> Welcome {
        Welcome {
            interval_ms: self.interval.as_millis() as u32,
            identity: self
                .identity
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        }
    }
}

fn shared_path() -> io::Result<PathBuf> {
    Ok(net::state_dir()?.join("shared-projects.json"))
}

fn load_shared(path: &std::path::Path) -> Option<SharedProjects> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_shared(path: &std::path::Path, shared: &SharedProjects) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(shared).map_err(io::Error::other)?,
    )?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&temporary, path)
}

#[derive(Default)]
struct Nudge {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Nudge {
    fn raise(&self) {
        *self.raised.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    fn wait(&self, longest: Duration) -> bool {
        let raised = self.raised.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut raised, _) = self
            .changed
            .wait_timeout_while(raised, longest, |raised| !*raised)
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *raised)
    }
}

fn spawn_sessions(mut sessions: Sessions, agent: Arc<Agent>) -> io::Result<()> {
    thread::Builder::new()
        .name("sessions".into())
        .spawn(move || {
            let store = |snapshot| {
                *agent
                    .sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = snapshot;
            };
            let mut woken = false;
            loop {
                if agent.hub.is_idle() {
                    store(Default::default());
                    agent.hub.wait_for_subscribers();
                }
                let mut snapshot = sessions.poll(&agent.board);
                agent.settle(&mut snapshot);
                store(snapshot);
                if woken {
                    agent.fresh.raise();
                }
                woken = sessions.wait(SESSIONS_EVERY);
            }
        })?;
    Ok(())
}

fn spawn_sampler(mut collector: Collector, agent: Arc<Agent>) -> io::Result<()> {
    thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            loop {
                agent.hub.wait_for_subscribers();
                if let Err(error) = collector.sample() {
                    agent.notices.info(format!("sampling failed: {error}"));
                }
                stream_samples(&mut collector, &agent);
            }
        })?;
    Ok(())
}

fn stream_samples(collector: &mut Collector, agent: &Agent) {
    let mut next = Instant::now() + agent.interval;
    loop {
        let early = agent
            .fresh
            .wait(next.saturating_duration_since(Instant::now()));
        let now = Instant::now();
        next = if early {
            now + agent.interval
        } else if next + agent.interval > now {
            next + agent.interval
        } else {
            now + agent.interval
        };
        let frame = collector.sample().and_then(|mut sample| {
            {
                let snapshot = agent
                    .sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                sample.sessions = snapshot.sessions.clone();
                sample.plans = snapshot.plans.clone();
                sample.notices = snapshot.notices.clone();
                sample.forwards = snapshot.forwards.clone();
                sample.processes = snapshot.processes.clone();
            }
            proto::encode_frame(&AgentMessage::Sample(sample))
        });
        let watched = match frame {
            Ok(frame) => agent.hub.publish(frame.into()),
            Err(error) => {
                agent.notices.info(format!("sampling failed: {error}"));
                agent.hub.has_subscribers()
            }
        };
        if !watched {
            return;
        }
    }
}

fn accept_local(listener: &UnixListener, agent: &Arc<Agent>) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                agent.notices.info(format!("local accept failed: {error}"));
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let Some(slot) = ConnectionSlot::claim(agent) else {
            continue;
        };
        spawn_connection(move || {
            stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
            serve(&mut stream, &slot.agent, Origin::Local)
        });
    }
}

fn serve_tailnet(port: u16, agent: &Arc<Agent>) -> io::Result<()> {
    loop {
        match bind_tailnet(port) {
            Ok((listener, ip, owner)) => {
                log::info(format!(
                    "tailnet viewers on {ip}:{port}, allowed for {owner}"
                ));
                let watch = Arc::new(TailnetWatch {
                    owner: RwLock::new(owner),
                    moved: AtomicBool::new(false),
                });
                spawn_tailnet_watch(ip, port, Arc::clone(&watch))?;
                accept_tailnet(&listener, &watch, agent);
                log::info("tailnet address changed, listening again");
            }
            Err(error) => agent.notices.info(format!(
                "tailnet listener unavailable, retrying every {TAILNET_RETRY:?}: {error}"
            )),
        }
        thread::sleep(TAILNET_RETRY);
    }
}

fn bind_tailnet(port: u16) -> io::Result<(TcpListener, Ipv4Addr, String)> {
    let status = tailscale::status()?;
    if status.tagged {
        return Err(io::Error::other(
            "this machine is tagged, and MAGI only serves machines owned by a person",
        ));
    }
    let ip = status
        .ipv4
        .ok_or_else(|| io::Error::other("this machine has no tailscale IPv4 address"))?;
    let owner = status
        .login
        .ok_or_else(|| io::Error::other("tailscale did not report this machine's owner"))?;
    Ok((TcpListener::bind((ip, port))?, ip, owner))
}

struct TailnetWatch {
    owner: RwLock<String>,
    moved: AtomicBool,
}

fn spawn_tailnet_watch(ip: Ipv4Addr, port: u16, watch: Arc<TailnetWatch>) -> io::Result<()> {
    thread::Builder::new()
        .name("tailnet watch".into())
        .spawn(move || {
            loop {
                thread::sleep(TAILNET_CHECK);
                let Ok(status) = tailscale::status() else {
                    continue;
                };
                match status.login {
                    Some(owner) if status.ipv4 == Some(ip) && !status.tagged => {
                        *watch.owner.write().unwrap_or_else(PoisonError::into_inner) = owner;
                    }
                    _ => {
                        watch.moved.store(true, Ordering::SeqCst);
                        let _ = TcpStream::connect((ip, port));
                        return;
                    }
                }
            }
        })?;
    Ok(())
}

fn accept_tailnet(listener: &TcpListener, watch: &Arc<TailnetWatch>, agent: &Arc<Agent>) {
    for stream in listener.incoming() {
        if watch.moved.load(Ordering::SeqCst) {
            return;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                agent
                    .notices
                    .info(format!("tailnet accept failed: {error}"));
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let Some(slot) = ConnectionSlot::claim(agent) else {
            agent.notices.info(format!(
                "refusing tailnet connections: {MAX_CONNECTIONS} already open"
            ));
            continue;
        };
        let watch = Arc::clone(watch);
        spawn_connection(move || admit_tailnet(stream, &watch, &slot.agent));
    }
}

fn admit_tailnet(
    mut stream: TcpStream,
    watch: &TailnetWatch,
    agent: &Arc<Agent>,
) -> io::Result<()> {
    let peer = stream.peer_addr()?;
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let owner = watch
        .owner
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Err(reason) = authorize(peer, &owner) {
        agent
            .notices
            .info(format!("refused {}: {reason}", peer.ip()));
        return proto::write_frame(
            &mut stream,
            &AgentMessage::Rejected("not authorized".into()),
        );
    }
    net::keepalive(&stream)?;
    serve(&mut stream, agent, Origin::Tailnet(peer))
}

fn authorize(peer: SocketAddr, owner: &str) -> Result<(), String> {
    match tailscale::whois_login(peer) {
        Ok(login) if login == owner => Ok(()),
        Ok(login) => Err(format!("it belongs to {login}")),
        Err(error) => Err(format!("could not identify it: {error}")),
    }
}

fn spawn_connection(serve: impl FnOnce() -> io::Result<()> + Send + 'static) {
    let spawned = thread::Builder::new().name("viewer".into()).spawn(move || {
        if let Err(error) = serve() {
            log::info(format!("connection ended: {error}"));
        }
    });
    if let Err(error) = spawned {
        log::info(format!("could not start a connection thread: {error}"));
    }
}

#[derive(Clone, Copy)]
enum Origin {
    Local,
    Tailnet(SocketAddr),
}

fn serve(conn: &mut dyn Conn, agent: &Arc<Agent>, origin: Origin) -> io::Result<()> {
    let hello: Hello = read_limited(conn, HELLO_MAX_BYTES, Instant::now() + HANDSHAKE_TIMEOUT)?;
    if hello.version != PROTOCOL_VERSION {
        let reason = format!(
            "this agent speaks protocol {PROTOCOL_VERSION}, the viewer speaks {}",
            hello.version
        );
        return proto::write_frame(conn, &AgentMessage::Rejected(reason));
    }
    proto::write_frame(conn, &AgentMessage::Welcome(agent.welcome()))?;
    match origin {
        Origin::Local => log::info("viewer on this machine connected"),
        Origin::Tailnet(peer) => log::info(format!("viewer {peer} connected")),
    }
    let (replies, frames) = agent.hub.subscribe_with_replies();
    let shared = agent
        .shared
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(frame) =
        shared.and_then(|shared| proto::encode_frame(&AgentMessage::Projects(shared)).ok())
    {
        let _ = replies.try_send(frame.into());
    }
    spawn_requests(conn.try_clone()?, replies, Arc::clone(agent))?;
    let streamed = frames
        .into_iter()
        .try_for_each(|frame| conn.write_all(&frame));
    let _ = conn.shutdown();
    streamed
}

fn spawn_requests(
    mut conn: Box<dyn Conn>,
    replies: SyncSender<Frame>,
    agent: Arc<Agent>,
) -> io::Result<()> {
    conn.set_read_timeout(None)?;
    thread::Builder::new()
        .name("requests".into())
        .spawn(move || {
            let mut buf = Vec::new();
            let mut watcher: Option<mpsc::Sender<Option<Target>>> = None;
            while let Ok(request) =
                proto::read_frame_within::<Request, _>(&mut *conn, &mut buf, REQUEST_MAX_BYTES)
            {
                match request {
                    Request::Move { session, to } => move_session(&session, &to, &agent, &replies),
                    Request::Prompt { session, text } => {
                        type_into(&session, &agent, &replies, |found, pane| {
                            agent.queue(found, pane, &text)
                        });
                    }
                    Request::CycleMode { session } => {
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| {
                            type_into(&session, &agent, replies, |found, pane| {
                                let mode = typing::cycle_mode(found, pane)?;
                                agent.remember_mode(found, mode);
                                Ok(format!("{} is in {} mode", found.title, mode.name()))
                            });
                        });
                    }
                    Request::Answer {
                        session,
                        allow,
                        tool,
                        waiting_secs,
                    } => {
                        let seen = typing::Seen { tool, waiting_secs };
                        type_into(&session, &agent, &replies, |found, pane| {
                            typing::answer(found, pane, allow, &seen)
                        });
                    }
                    Request::Choose {
                        session,
                        option,
                        waiting_secs,
                    } => {
                        type_into(&session, &agent, &replies, |found, pane| {
                            typing::choose(found, pane, option, waiting_secs)
                        });
                    }
                    Request::Stop { session } => {
                        type_into(&session, &agent, &replies, typing::stop);
                    }
                    Request::Show { session, what } => {
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| {
                            let shown = agent.show(session, what);
                            if let Ok(frame) = proto::encode_frame(&AgentMessage::Shown(shown)) {
                                let _ = replies.send(frame.into());
                            }
                        });
                    }
                    Request::ShareProjects(shared) => {
                        agent.share_projects(shared);
                    }
                    Request::Folders => {
                        in_background(&replies, |replies| {
                            let models =
                                crate::handoff::find("codex", &crate::handoff::search_dirs())
                                    .and_then(|program| {
                                        crate::sessions::codex_app::models(&program)
                                    })
                                    .unwrap_or_default();
                            if let Ok(frame) = proto::encode_frame(&AgentMessage::Models(models)) {
                                let _ = replies.send(frame.into());
                            }
                        });
                        in_background(&replies, |replies| match opener::folders() {
                            Ok(folders) => {
                                if let Ok(frame) =
                                    proto::encode_frame(&AgentMessage::Folders(folders))
                                {
                                    let _ = replies.send(frame.into());
                                }
                            }
                            Err(text) => reply(replies, false, text),
                        });
                    }
                    Request::Adopt { session } => {
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| match adopt(&session, &agent) {
                            Ok(text) => reply(replies, true, text),
                            Err(text) => reply(replies, false, text),
                        });
                    }
                    Request::Open {
                        place,
                        codex,
                        model,
                    } => {
                        in_background(&replies, move |replies| {
                            match opener::open(&place, codex, model.as_deref()) {
                                Ok(text) => reply(replies, true, text),
                                Err(text) => reply(replies, false, text),
                            }
                        });
                    }
                    Request::Watch { session, child } => {
                        let target = session.map(|session| (session, child));
                        let delivered = watcher
                            .as_ref()
                            .is_some_and(|watcher| watcher.send(target.clone()).is_ok());
                        if !delivered && target.is_some() {
                            watcher =
                                spawn_watcher(target, replies.clone(), Arc::clone(&agent)).ok();
                        }
                    }
                }
            }
        })?;
    Ok(())
}

fn spawn_watcher(
    first: Option<Target>,
    replies: SyncSender<Frame>,
    agent: Arc<Agent>,
) -> io::Result<mpsc::Sender<Option<Target>>> {
    let (watch, changes) = mpsc::channel();
    thread::Builder::new()
        .name("screen".into())
        .spawn(move || {
            let mut watching = first;
            let mut capturer = Watcher::default();
            let mut last: Option<Screen> = None;
            loop {
                if let Some((session, child)) = &watching {
                    let screen = Screen {
                        session: session.clone(),
                        child: child.clone(),
                        view: agent.capture(session, child.as_deref(), &mut capturer),
                    };
                    if last.as_ref() != Some(&screen) {
                        let Ok(frame) = proto::encode_frame(&AgentMessage::Screen(screen.clone()))
                        else {
                            return;
                        };
                        if replies.send(frame.into()).is_err() {
                            return;
                        }
                        last = Some(screen);
                    }
                }
                let next = if watching.is_some() {
                    match changes.recv_timeout(SCREEN_EVERY) {
                        Ok(next) => next,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match changes.recv() {
                        Ok(next) => next,
                        Err(_) => return,
                    }
                };
                let next = changes.try_iter().last().unwrap_or(next);
                if next != watching {
                    last = None;
                }
                watching = next;
            }
        })?;
    Ok(watch)
}

fn move_session(session: &str, to: &str, agent: &Agent, replies: &SyncSender<Frame>) {
    let found = agent
        .sessions
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .sessions
        .iter()
        .find(|known| known.id == session)
        .cloned();
    let Some(found) = found else {
        return reply(replies, false, "that session has ended".into());
    };
    let later = replies.clone();
    let started = handoff::start(&found, to, move |outcome| {
        reply(&later, outcome.ok, outcome.text);
    });
    match started {
        Ok(text) => reply(replies, true, text),
        Err(text) => reply(replies, false, text),
    }
}

fn in_background(
    replies: &SyncSender<Frame>,
    work: impl FnOnce(&SyncSender<Frame>) + Send + 'static,
) {
    let replies = replies.clone();
    let spawned = thread::Builder::new()
        .name("open".into())
        .spawn(move || work(&replies));
    if let Err(error) = spawned {
        log::info(format!("could not start a thread for the request: {error}"));
    }
}

fn adopt(session: &str, agent: &Agent) -> Result<String, String> {
    let (found, cwd, in_rex) = {
        let snapshot = agent
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (
            snapshot
                .sessions
                .iter()
                .find(|known| known.id == session)
                .cloned(),
            snapshot.cwds.get(session).cloned(),
            snapshot.panes.contains_key(session),
        )
    };
    let found = found.ok_or("that session has ended")?;
    if found.agent != proto::Agent::ClaudeCode {
        return Err(format!(
            "{} is a {} session; magi continues Claude Code sessions in Rex",
            found.title,
            found.agent.name()
        ));
    }
    if in_rex {
        return Err(format!("{} is already in Rex", found.title));
    }
    let cwd = cwd.ok_or_else(|| format!("magi doesn't know which folder {} is in", found.title))?;
    opener::adopt(&cwd, session)
}

fn type_into(
    session: &str,
    agent: &Agent,
    replies: &SyncSender<Frame>,
    act: impl FnOnce(&AgentSession, Option<&Location>) -> Result<String, String>,
) {
    let (found, pane) = {
        let snapshot = agent
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (
            snapshot
                .sessions
                .iter()
                .find(|known| known.id == session)
                .cloned(),
            snapshot.panes.get(session).cloned(),
        )
    };
    let Some(found) = found else {
        return reply(replies, false, "that session has ended".into());
    };
    match act(&found, pane.as_ref()) {
        Ok(text) => reply(replies, true, text),
        Err(text) => reply(replies, false, text),
    }
}

fn reply(replies: &SyncSender<Frame>, ok: bool, text: String) {
    if let Ok(frame) = proto::encode_frame(&AgentMessage::Reply(Reply { ok, text })) {
        let _ = replies.send(frame.into());
    }
}

fn accept_status(listener: &UnixListener, agent: &Agent) {
    for stream in listener.incoming() {
        let report = stream.and_then(|mut stream| {
            read_limited::<StatusReport>(
                &mut stream,
                STATUS_MAX_BYTES,
                Instant::now() + STATUS_READ_TIMEOUT,
            )
        });
        match report {
            Ok(report) => agent.board.record(report),
            Err(error) => {
                agent
                    .notices
                    .info(format!("ignored a status report: {error}"));
                thread::sleep(STATUS_RETRY);
            }
        }
    }
}

fn read_limited<T: serde::de::DeserializeOwned>(
    conn: &mut dyn Conn,
    max: usize,
    deadline: Instant,
) -> io::Result<T> {
    let no_frame = |error: io::Error| match error.kind() {
        io::ErrorKind::UnexpectedEof => io::Error::new(
            error.kind(),
            "closed before sending anything, likely a port probe",
        ),
        _ => io::Error::new(error.kind(), format!("no first frame: {error}")),
    };
    let mut len = [0u8; 4];
    net::read_exact_before(conn, &mut len, deadline).map_err(no_frame)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {len} byte first frame is too big"),
        ));
    }
    let mut body = vec![0; len];
    net::read_exact_before(conn, &mut body, deadline).map_err(no_frame)?;
    proto::decode(&body)
}

struct ConnectionSlot {
    agent: Arc<Agent>,
}

impl ConnectionSlot {
    fn claim(agent: &Arc<Agent>) -> Option<Self> {
        let previous = agent.connections.fetch_add(1, Ordering::SeqCst);
        if previous >= MAX_CONNECTIONS {
            agent.connections.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Self {
            agent: Arc::clone(agent),
        })
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.agent.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Hub {
    subscribers: Mutex<Vec<SyncSender<Frame>>>,
    joined: Condvar,
}

impl Hub {
    #[cfg(test)]
    fn subscribe(&self) -> Receiver<Frame> {
        self.subscribe_with_replies().1
    }

    fn subscribe_with_replies(&self) -> (SyncSender<Frame>, Receiver<Frame>) {
        let (sender, receiver) = mpsc::sync_channel(FRAME_BACKLOG);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sender.clone());
        self.joined.notify_all();
        (sender, receiver)
    }

    fn is_idle(&self) -> bool {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    fn wait_for_subscribers(&self) {
        let subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        drop(
            self.joined
                .wait_while(subscribers, |subscribers| subscribers.is_empty())
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    fn publish(&self, frame: Frame) -> bool {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.retain(|subscriber| {
            !matches!(
                subscriber.try_send(Arc::clone(&frame)),
                Err(TrySendError::Disconnected(_))
            )
        });
        !subscribers.is_empty()
    }

    fn has_subscribers(&self) -> bool {
        !self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;

    use super::*;

    fn working(id: &str) -> AgentSession {
        AgentSession {
            agent: proto::Agent::ClaudeCode,
            id: id.into(),
            title: "fix the build".into(),
            project: String::new(),
            state: SessionState::Working { tool: None },
            state_secs: 0,
            model: String::new(),
            context_tokens: 0,
            context_used: None,
            tokens: Default::default(),
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
            mode: Some(PermissionMode::Auto),
            queued: None,
        }
    }

    #[test]
    fn a_message_for_a_busy_session_waits_until_it_is_idle_and_can_be_dropped() {
        let agent = Agent::new(identity(), Duration::from_secs(1), Board::default(), None);
        let pane = Location::in_test("session:1", "block:1");
        let busy = working("s1");
        assert!(
            agent
                .queue(&busy, Some(&pane), "then run the tests")
                .unwrap()
                .starts_with("Queued")
        );
        let mut snapshot = Snapshot {
            sessions: vec![busy.clone()],
            ..Snapshot::default()
        };
        agent.settle(&mut snapshot);
        assert_eq!(
            snapshot.sessions[0].queued.as_deref(),
            Some("then run the tests")
        );
        assert!(
            agent
                .queue(&busy, Some(&pane), "  ")
                .unwrap()
                .starts_with("Dropped")
        );
        assert!(agent.queue(&busy, Some(&pane), "").is_err());
        agent.queue(&busy, Some(&pane), "again").unwrap();
        snapshot.sessions[0].state = SessionState::Idle;
        agent.settle(&mut snapshot);
        assert_eq!(snapshot.sessions[0].queued, None);
        assert!(agent.queued.lock().unwrap().is_empty());
        assert!(agent.queue(&busy, None, "no pane").is_err());
    }

    #[test]
    fn a_mode_magi_switched_to_holds_until_the_transcript_records_another() {
        let agent = Agent::new(identity(), Duration::from_secs(1), Board::default(), None);
        let session = working("s1");
        agent.remember_mode(&session, PermissionMode::Plan);
        let mut snapshot = Snapshot {
            sessions: vec![session.clone()],
            ..Snapshot::default()
        };
        agent.settle(&mut snapshot);
        assert_eq!(snapshot.sessions[0].mode, Some(PermissionMode::Plan));
        agent.remember_mode(&snapshot.sessions[0].clone(), PermissionMode::Auto);
        let mut later = Snapshot {
            sessions: vec![session.clone()],
            ..Snapshot::default()
        };
        agent.settle(&mut later);
        assert_eq!(later.sessions[0].mode, Some(PermissionMode::Auto));
        let mut recorded = session;
        recorded.mode = Some(PermissionMode::AcceptEdits);
        let mut after_a_message = Snapshot {
            sessions: vec![recorded],
            ..Snapshot::default()
        };
        agent.settle(&mut after_a_message);
        assert_eq!(
            after_a_message.sessions[0].mode,
            Some(PermissionMode::AcceptEdits)
        );
        assert!(agent.modes.lock().unwrap().is_empty());
    }

    #[test]
    fn an_agent_keeps_the_newest_shared_projects_and_hands_them_to_every_viewer() {
        let mut agent = Agent::new(identity(), Duration::from_secs(1), Board::default(), None);
        let dir = std::env::temp_dir().join(format!("magi-shared-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared-projects.json");
        agent.shared_file = Some(path.clone());
        let viewer = agent.hub.subscribe();
        let mut newer = SharedProjects {
            updated_ms: 20,
            ..SharedProjects::default()
        };
        newer.repos.insert("ping".into(), "Games".into());
        agent.share_projects(newer.clone());
        let frame = viewer.try_recv().unwrap();
        let mut buf = Vec::new();
        let message: AgentMessage = proto::read_frame(&mut &frame[..], &mut buf).unwrap();
        assert_eq!(message, AgentMessage::Projects(newer.clone()));
        agent.share_projects(SharedProjects {
            updated_ms: 10,
            ..SharedProjects::default()
        });
        assert!(viewer.try_recv().is_err());
        assert_eq!(load_shared(&path), Some(newer));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn identity() -> Identity {
        Identity {
            hostname: "test".into(),
            tailscale_node: None,
            os: String::new(),
            kernel: String::new(),
            arch: String::new(),
            cpu_model: String::new(),
            core_kinds: Vec::new(),
            agent_version: String::new(),
        }
    }

    struct LocalAgent {
        socket: PathBuf,
    }

    impl Drop for LocalAgent {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.socket.parent().unwrap());
        }
    }

    fn start_local_agent(name: &str, interval: Duration) -> LocalAgent {
        let socket = std::env::temp_dir()
            .join(format!("magi-agent-{}-{name}", std::process::id()))
            .join("agent.sock");
        let agent = Arc::new(Agent::new(identity(), interval, Board::default(), None));
        let listener = net::bind_local(&socket).unwrap();
        spawn_sampler(Collector::new().unwrap(), Arc::clone(&agent)).unwrap();
        thread::spawn(move || accept_local(&listener, &agent));
        LocalAgent { socket }
    }

    fn connect(agent: &LocalAgent, version: u16) -> (UnixStream, AgentMessage) {
        let mut stream = UnixStream::connect(&agent.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        proto::write_frame(&mut stream, &Hello { version }).unwrap();
        let reply = proto::read_frame(&mut stream, &mut Vec::new()).unwrap();
        (stream, reply)
    }

    #[test]
    fn a_viewer_asking_to_move_a_session_that_ended_is_told_so_alongside_its_samples() {
        let agent = start_local_agent("requests", Duration::from_millis(100));
        let (mut stream, reply) = connect(&agent, PROTOCOL_VERSION);
        assert!(matches!(reply, AgentMessage::Welcome(_)), "{reply:?}");
        proto::write_frame(
            &mut stream,
            &Request::Move {
                session: "gone".into(),
                to: "homelab".into(),
            },
        )
        .unwrap();
        let mut buf = Vec::new();
        let reply = (0..50)
            .find_map(
                |_| match proto::read_frame(&mut stream, &mut buf).unwrap() {
                    AgentMessage::Reply(reply) => Some(reply),
                    _ => None,
                },
            )
            .expect("a reply among the samples");
        assert_eq!(
            reply,
            Reply {
                ok: false,
                text: "that session has ended".into()
            }
        );
        let next: AgentMessage = proto::read_frame(&mut stream, &mut buf).unwrap();
        assert!(matches!(next, AgentMessage::Sample(_)), "{next:?}");
    }

    #[test]
    fn watching_a_session_the_agent_does_not_know_says_it_has_ended() {
        let agent = start_local_agent("watch", Duration::from_millis(100));
        let (mut stream, reply) = connect(&agent, PROTOCOL_VERSION);
        assert!(matches!(reply, AgentMessage::Welcome(_)), "{reply:?}");
        proto::write_frame(
            &mut stream,
            &Request::Watch {
                session: Some("elsewhere".into()),
                child: None,
            },
        )
        .unwrap();
        let mut buf = Vec::new();
        let screen = (0..50)
            .find_map(
                |_| match proto::read_frame(&mut stream, &mut buf).unwrap() {
                    AgentMessage::Screen(screen) => Some(screen),
                    _ => None,
                },
            )
            .expect("a screen among the samples");
        assert_eq!(
            screen,
            Screen {
                session: "elsewhere".into(),
                child: None,
                view: Err("that session has ended".into()),
            }
        );
        proto::write_frame(
            &mut stream,
            &Request::Watch {
                session: None,
                child: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn streams_samples_after_welcoming_a_local_viewer() {
        let agent = start_local_agent("streams", Duration::from_millis(100));
        let (mut stream, reply) = connect(&agent, PROTOCOL_VERSION);
        let AgentMessage::Welcome(welcome) = reply else {
            panic!("expected a welcome, got {reply:?}")
        };
        assert_eq!(welcome.interval_ms, 100);
        assert_eq!(welcome.identity.hostname, "test");

        let mut buf = Vec::new();
        for _ in 0..3 {
            let message: AgentMessage = proto::read_frame(&mut stream, &mut buf).unwrap();
            let AgentMessage::Sample(sample) = message else {
                panic!("expected a sample, got {message:?}")
            };
            assert!(sample.memory.total > 0);
        }
    }

    #[test]
    fn viewers_share_one_sampler_and_disconnects_are_pruned() {
        let agent = start_local_agent("shared", Duration::from_millis(50));
        let (first, _) = connect(&agent, PROTOCOL_VERSION);
        let (mut second, _) = connect(&agent, PROTOCOL_VERSION);
        drop(first);
        let mut buf = Vec::new();
        for _ in 0..5 {
            let message: AgentMessage = proto::read_frame(&mut second, &mut buf).unwrap();
            assert!(matches!(message, AgentMessage::Sample(_)));
        }
    }

    #[test]
    fn rejects_a_viewer_speaking_another_protocol_version() {
        let agent = start_local_agent("version", Duration::from_millis(100));
        let (_, reply) = connect(&agent, PROTOCOL_VERSION + 1);
        assert!(matches!(reply, AgentMessage::Rejected(reason) if reason.contains("protocol")));
    }

    #[test]
    fn hostile_clients_are_cut_off_without_hurting_real_viewers() {
        let agent = start_local_agent("hostile", Duration::from_millis(50));
        let (mut viewer, _) = connect(&agent, PROTOCOL_VERSION);

        let mut oversized = UnixStream::connect(&agent.socket).unwrap();
        oversized.write_all(&(1u32 << 20).to_le_bytes()).unwrap();
        let mut trickle = UnixStream::connect(&agent.socket).unwrap();
        trickle
            .set_read_timeout(Some(HANDSHAKE_TIMEOUT * 2))
            .unwrap();
        let trickler = {
            let mut trickle = trickle.try_clone().unwrap();
            thread::spawn(move || {
                for byte in [1u8, 0, 0, 0] {
                    if trickle.write_all(&[byte]).is_err() {
                        return;
                    }
                    thread::sleep(HANDSHAKE_TIMEOUT / 3);
                }
            })
        };

        let started = Instant::now();
        let mut buf = Vec::new();
        assert!(proto::read_frame::<AgentMessage, _>(&mut oversized, &mut buf).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(proto::read_frame::<AgentMessage, _>(&mut trickle, &mut buf).is_err());
        let waited = started.elapsed();
        assert!(
            waited < HANDSHAKE_TIMEOUT + Duration::from_secs(1),
            "trickling client held on for {waited:?}"
        );

        for _ in 0..3 {
            let message: AgentMessage = proto::read_frame(&mut viewer, &mut buf).unwrap();
            assert!(matches!(message, AgentMessage::Sample(_)));
        }
        trickler.join().unwrap();
    }

    #[test]
    fn connections_beyond_the_cap_are_closed() {
        let agent = start_local_agent("cap", Duration::from_millis(200));
        let held: Vec<UnixStream> = (0..MAX_CONNECTIONS)
            .map(|_| connect(&agent, PROTOCOL_VERSION).0)
            .collect();
        let mut extra = UnixStream::connect(&agent.socket).unwrap();
        let _ = extra.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = proto::write_frame(
            &mut extra,
            &Hello {
                version: PROTOCOL_VERSION,
            },
        );
        assert!(proto::read_frame::<AgentMessage, _>(&mut extra, &mut Vec::new()).is_err());
        drop(held);
    }

    #[test]
    fn status_reports_are_recorded_and_oversized_ones_dropped() {
        let agent = Agent::new(identity(), Duration::from_secs(1), Board::default(), None);
        let dir = std::env::temp_dir().join(format!("magi-status-{}", std::process::id()));
        let path = dir.join("status.sock");
        let listener = net::bind_local(&path).unwrap();
        let agent = Arc::new(agent);
        {
            let agent = Arc::clone(&agent);
            thread::spawn(move || accept_status(&listener, &agent));
        }
        let report = StatusReport {
            session_id: "s".into(),
            context_used: Some(4_200),
            windows: Vec::new(),
        };
        proto::write_frame(&mut UnixStream::connect(&path).unwrap(), &report).unwrap();
        let oversized = StatusReport {
            session_id: "x".repeat(STATUS_MAX_BYTES),
            ..report.clone()
        };
        proto::write_frame(&mut UnixStream::connect(&path).unwrap(), &oversized).unwrap();
        let silent = UnixStream::connect(&path).unwrap();
        let started = Instant::now();
        proto::write_frame(
            &mut UnixStream::connect(&path).unwrap(),
            &StatusReport {
                session_id: "t".into(),
                ..report.clone()
            },
        )
        .unwrap();
        while agent.board.context("t").is_none() {
            assert!(
                started.elapsed() < STATUS_READ_TIMEOUT * 3,
                "a silent client blocked status reports"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(agent.board.context("s"), Some(4_200));
        assert_eq!(agent.board.context(&oversized.session_id), None);
        drop(silent);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hub_drops_subscribers_whose_receiver_is_gone() {
        let hub = Hub::default();
        let kept = hub.subscribe();
        drop(hub.subscribe());
        assert!(hub.publish(Arc::from(&b"frame"[..])));
        assert_eq!(&*kept.recv().unwrap(), b"frame");
        drop(kept);
        assert!(!hub.publish(Arc::from(&b"frame"[..])));
    }
}
