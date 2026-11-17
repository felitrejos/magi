use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::collect::{self, Collector, SystemInfo};
use crate::conversation::Reader;
use crate::handoff;
use crate::net::{self, Conn};
use crate::proto::{
    self, AgentMessage, AgentSession, Content, Hello, Identity, OpenIn, PROTOCOL_VERSION, Reply,
    Request, SessionState, Shown, StatusReport, View, Welcome,
};
use crate::sessions::rex::Location;
use crate::sessions::{Board, Sessions, Snapshot};
use crate::{children, log, mirrors, opener, prs, show, tailscale, tools, typing};

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
const STATUS_RETRY: Duration = Duration::from_millis(10);
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(1);
const SESSIONS_EVERY: Duration = Duration::from_secs(1);
const MIN_POLL_GAP: Duration = Duration::from_millis(250);

type Frame = Arc<[u8]>;

pub struct Config {
    pub port: u16,
    pub interval: Duration,
}

pub fn run(config: &Config) -> io::Result<()> {
    let collector = Collector::new()?;
    let info = collector.info();
    let board = Board::saved_at(net::state_dir()?.join("plans.json"));
    let agent = Arc::new(Agent::new(identity_of(&info), config.interval, board));
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
        arch: std::env::consts::ARCH.to_string(),
        tools: tools::available(),
        cpu_model: info.cpu_model.clone(),
        core_kinds: info.core_kinds.clone(),
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

struct Agent {
    identity: RwLock<Identity>,
    interval: Duration,
    hub: Hub,
    connections: AtomicUsize,
    notices: log::Throttle,
    board: Board,
    sessions: Mutex<Snapshot>,
    fresh: Nudge,
    queued: Mutex<HashMap<String, String>>,
    mirrored: Mutex<HashSet<(String, String)>>,
    starting: Mutex<Vec<Starting>>,
    prs: prs::Prs,
}

struct Starting {
    pane: opener::OpenedPane,
    title: String,
    since: Instant,
    asking: bool,
}

const TRUST_PREFIX: &str = "trust:";
const WATCH_START_FOR: Duration = Duration::from_secs(90);

impl Agent {
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
        let pane = pane.ok_or_else(|| typing::not_in_rex(session))?;
        if session.state == SessionState::Waiting(proto::Waiting::Trust) {
            return Err(format!(
                "{} is asking to trust its folder: answer that first",
                session.title
            ));
        }
        if session.state == SessionState::Idle {
            drop(queued);
            return typing::prompt(session, Some(pane), text);
        }
        queued.insert(session.id.clone(), text.to_string());
        Ok(format!(
            "Queued for {}: it goes in once the session is idle",
            session.title
        ))
    }

    fn mirror_subagents(&self, snapshot: &Snapshot) {
        let wanted = mirrors::wanted(
            snapshot,
            &mut self.mirrored.lock().unwrap_or_else(PoisonError::into_inner),
        );
        if wanted.is_empty() {
            return;
        }
        let Ok(magi) = std::env::current_exe() else {
            return;
        };
        let spawned = thread::Builder::new()
            .name("mirrors".into())
            .spawn(move || {
                for mirror in &wanted {
                    if let Err(error) = mirrors::open(mirror, &magi) {
                        log::info(format!(
                            "couldn't open a tab for subagent {}: {error}",
                            mirror.key.1
                        ));
                    }
                }
            });
        if let Err(error) = spawned {
            log::info(format!(
                "could not start a thread for subagent tabs: {error}"
            ));
        }
    }

    fn expect_start(&self, pane: opener::OpenedPane, title: String) {
        self.starting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Starting {
                pane,
                title,
                since: Instant::now(),
                asking: false,
            });
    }

    fn list_starting(&self, snapshot: &mut Snapshot) {
        let mut starting = self.starting.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let running: HashSet<&str> = snapshot
            .panes
            .values()
            .map(|location| location.block.as_str())
            .collect();
        starting.retain(|start| !running.contains(start.pane.block.as_str()));
        for start in starting.iter_mut() {
            let location = Location::here(&start.pane.session, &start.pane.block);
            start.asking = location.as_ref().is_some_and(typing::asks_to_trust);
            let (true, Some(location)) = (start.asking, location) else {
                continue;
            };
            let id = format!("{TRUST_PREFIX}{}", start.pane.block);
            snapshot.sessions.push(trusting(
                &id,
                &start.title,
                &start.pane,
                now.duration_since(start.since),
            ));
            snapshot.panes.insert(id, location);
        }
        starting.retain(|start| start.asking || now.duration_since(start.since) < WATCH_START_FOR);
    }

    fn settle(&self, snapshot: &mut Snapshot) {
        self.list_starting(snapshot);
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        queued.retain(|id, _| snapshot.sessions.iter().any(|session| session.id == *id));
        for session in &mut snapshot.sessions {
            session.queued = None;
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

    fn diff(&self, session: String) -> Shown {
        let cwd = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cwds
            .get(&session)
            .cloned()
            .unwrap_or_default();
        Shown {
            session,
            diff: show::diff(&cwd),
        }
    }

    fn read(&self, session: &str, child: Option<&str>) -> Result<Content, String> {
        if let Some(child) = child {
            return self.read_subagent(session, child);
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
            (Some(pane), _) if asking => pane.screen().map(Content::Screen),
            (_, Some((agent, path))) => Reader::default()
                .read(&path, agent)
                .map(Content::Conversation),
            (Some(pane), None) => pane.screen().map(Content::Screen),
            (None, None) => Err("that session has ended".into()),
        }
    }

    fn read_subagent(&self, session: &str, child: &str) -> Result<Content, String> {
        let parent = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .transcripts
            .get(session)
            .map(|(_, path)| path.clone())
            .ok_or("that session's transcript isn't on this machine")?;
        let transcript = children::subagent_transcript(&parent, child)
            .ok_or("that subagent's transcript isn't there")?;
        Reader::default()
            .read(&transcript, proto::Agent::ClaudeCode)
            .map(Content::Conversation)
    }

    fn new(identity: Identity, interval: Duration, board: Board) -> Self {
        Self {
            identity: RwLock::new(identity),
            interval,
            hub: Hub::default(),
            connections: AtomicUsize::new(0),
            notices: log::Throttle::default(),
            board,
            sessions: Mutex::new(Snapshot::default()),
            fresh: Nudge::default(),
            queued: Mutex::default(),
            mirrored: Mutex::default(),
            starting: Mutex::default(),
            prs: prs::Prs::default(),
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

    fn lower(&self) {
        *self.raised.lock().unwrap_or_else(PoisonError::into_inner) = false;
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
            let mut woken = true;
            let mut was_watched = false;
            let mut polled = Instant::now() - MIN_POLL_GAP;
            loop {
                let watched = agent.hub.has_subscribers();
                let joined = watched && !was_watched;
                was_watched = watched;
                if watched || woken {
                    if !joined {
                        thread::sleep(MIN_POLL_GAP.saturating_sub(polled.elapsed()));
                    }
                    polled = Instant::now();
                    let mut snapshot = sessions.poll(&agent.board);
                    if watched {
                        agent.prs.fill(&mut snapshot.sessions, &snapshot.cwds);
                    }
                    agent.settle(&mut snapshot);
                    agent.mirror_subagents(&snapshot);
                    store(snapshot);
                    if watched && (woken || joined) {
                        agent.fresh.raise();
                    }
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
                agent.fresh.lower();
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
                    Request::Answer {
                        session,
                        allow,
                        tool,
                        waiting_secs,
                    } => {
                        let seen = typing::Seen { tool, waiting_secs };
                        let trust = session.starts_with(TRUST_PREFIX);
                        type_into(&session, &agent, &replies, |found, pane| {
                            if trust {
                                typing::trust(found, pane, allow)
                            } else {
                                typing::answer(found, pane, allow, &seen)
                            }
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
                    Request::CloseForward { session } => {
                        in_background(&replies, move |replies| {
                            match opener::close_forward(&session) {
                                Ok(text) => reply(replies, true, text),
                                Err(text) => reply(replies, false, text),
                            }
                        });
                    }
                    Request::Diff { session } => {
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| {
                            let shown = agent.diff(session);
                            if let Ok(frame) = proto::encode_frame(&AgentMessage::Shown(shown)) {
                                let _ = replies.send(frame.into());
                            }
                        });
                    }
                    Request::Folders => {
                        in_background(&replies, |replies| {
                            let models = handoff::find("codex", &handoff::search_dirs())
                                .and_then(|program| crate::sessions::codex_app::models(&program))
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
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| {
                            match opener::open(&place, codex, model.as_deref()) {
                                Ok(opened) => {
                                    if let (Some(pane), false) = (opened.pane, codex) {
                                        agent.expect_start(pane, place_title(&place));
                                    }
                                    reply(replies, true, opened.message);
                                }
                                Err(text) => reply(replies, false, text),
                            }
                        });
                    }
                    Request::Read { session, child } => {
                        let agent = Arc::clone(&agent);
                        in_background(&replies, move |replies| {
                            let read = View {
                                view: agent.read(&session, child.as_deref()),
                                session,
                                child,
                            };
                            if let Ok(frame) = proto::encode_frame(&AgentMessage::Read(read)) {
                                let _ = replies.send(frame.into());
                            }
                        });
                    }
                }
            }
        })?;
    Ok(())
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

fn place_title(place: &OpenIn) -> String {
    match place {
        OpenIn::Folder(dir) => dir
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or(dir)
            .to_string(),
        OpenIn::Chats => "a new chat".into(),
    }
}

fn trusting(id: &str, title: &str, pane: &opener::OpenedPane, waiting: Duration) -> AgentSession {
    AgentSession {
        agent: proto::Agent::ClaudeCode,
        id: id.into(),
        title: title.into(),
        project: title.into(),
        state: SessionState::Waiting(proto::Waiting::Trust),
        state_secs: u32::try_from(waiting.as_secs()).unwrap_or(u32::MAX),
        model: String::new(),
        context_tokens: 0,
        context_used: None,
        cost_micros: 0,
        cost_complete: true,
        pane: Some(proto::RexPane {
            session: pane.session.clone(),
            window: pane.window.clone(),
            block: pane.block.clone(),
        }),
        usage: None,
        children: Vec::new(),
        branch: None,
        parent: None,
        activity: None,
        mode: None,
        queued: None,
        remote: None,
        pr: None,
    }
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
mod tests;
