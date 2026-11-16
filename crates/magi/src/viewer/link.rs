use std::io;
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::Event;
use super::model::{Endpoint, LinkUpdate};
use crate::net::Conn;
use crate::proto::{self, AgentMessage, Hello, PROTOCOL_VERSION, Request};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const MIN_SILENCE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_SILENCE_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const REJECTED_BACKOFF: Duration = Duration::from_secs(60);
const STOP_POLL: Duration = Duration::from_millis(200);

pub enum Dial {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

impl Dial {
    fn connect(&self) -> io::Result<Box<dyn Conn>> {
        match self {
            Dial::Unix(path) => Ok(Box::new(UnixStream::connect(path)?)),
            Dial::Tcp(addr) => {
                let stream = TcpStream::connect_timeout(addr, CONNECT_TIMEOUT)?;
                stream.set_nodelay(true)?;
                Ok(Box::new(stream))
            }
        }
    }
}

pub struct LinkHandle {
    stop: Arc<AtomicBool>,
    writer: Writer,
}

type Writer = Arc<Mutex<Option<Box<dyn Conn>>>>;

impl LinkHandle {
    pub fn request(&self, request: &Request) -> io::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let conn = writer
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "not connected"))?;
        proto::write_frame(&mut **conn, request)
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn spawn(
    endpoint: Endpoint,
    dial: Dial,
    generation: u64,
    events: SyncSender<Event>,
) -> io::Result<LinkHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let writer = Writer::default();
    let handle = LinkHandle {
        stop: Arc::clone(&stop),
        writer: Arc::clone(&writer),
    };
    let link = Link {
        endpoint,
        generation,
        events,
        stop,
        writer,
    };
    thread::Builder::new()
        .name(format!("link {endpoint:?}"))
        .spawn(move || link.maintain(&dial))?;
    Ok(handle)
}

struct Link {
    endpoint: Endpoint,
    generation: u64,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    writer: Writer,
}

enum Ended {
    Lost { reason: String, was_live: bool },
    Rejected(String),
    Stopped,
}

impl Link {
    fn maintain(&self, dial: &Dial) {
        let mut backoff = MIN_BACKOFF;
        loop {
            let ended = self.session(dial);
            *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
            let (update, wait) = match ended {
                Ended::Stopped => return,
                Ended::Rejected(reason) => (LinkUpdate::Rejected(reason), REJECTED_BACKOFF),
                Ended::Lost { reason, was_live } => {
                    if was_live {
                        backoff = MIN_BACKOFF;
                    }
                    let wait = backoff;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    (LinkUpdate::Lost(reason), wait)
                }
            };
            if !self.send(update) || self.sleep_unless_stopped(wait) {
                return;
            }
        }
    }

    fn session(&self, dial: &Dial) -> Ended {
        let lost = |error: io::Error, was_live| Ended::Lost {
            reason: describe(&error),
            was_live,
        };
        let mut conn = match dial.connect() {
            Ok(conn) => conn,
            Err(error) => return lost(error, false),
        };
        let handshake = conn
            .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
            .and_then(|()| conn.set_write_timeout(Some(HANDSHAKE_TIMEOUT)))
            .and_then(|()| {
                proto::write_frame(
                    &mut *conn,
                    &Hello {
                        version: PROTOCOL_VERSION,
                    },
                )
            });
        if let Err(error) = handshake {
            return lost(error, false);
        }
        let mut buf = Vec::new();
        let welcome = match proto::read_frame::<AgentMessage, _>(&mut *conn, &mut buf) {
            Ok(AgentMessage::Welcome(welcome)) => welcome,
            Ok(AgentMessage::Rejected(reason)) => return Ended::Rejected(reason),
            Ok(
                AgentMessage::Sample(_)
                | AgentMessage::Reply(_)
                | AgentMessage::Screen(_)
                | AgentMessage::Folders(_)
                | AgentMessage::Models(_)
                | AgentMessage::Labels(_)
                | AgentMessage::Shown(_),
            ) => {
                return lost(
                    io::Error::other("agent sent something before its welcome"),
                    false,
                );
            }
            Err(error) => return lost(error, false),
        };
        let silence = (Duration::from_millis(u64::from(welcome.interval_ms)) * 3)
            .clamp(MIN_SILENCE_TIMEOUT, MAX_SILENCE_TIMEOUT);
        if let Err(error) = conn.set_read_timeout(Some(silence)) {
            return lost(error, false);
        }
        match conn.try_clone() {
            Ok(writer) => {
                *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = Some(writer)
            }
            Err(error) => return lost(error, false),
        }
        if !self.send(LinkUpdate::Welcome(Box::new(welcome))) {
            return Ended::Stopped;
        }
        loop {
            let update = match proto::read_frame::<AgentMessage, _>(&mut *conn, &mut buf) {
                Ok(AgentMessage::Sample(sample)) => LinkUpdate::Sample(Box::new(sample)),
                Ok(AgentMessage::Reply(reply)) => LinkUpdate::Reply(reply),
                Ok(AgentMessage::Screen(screen)) => LinkUpdate::Screen(screen),
                Ok(AgentMessage::Folders(folders)) => LinkUpdate::Folders(folders),
                Ok(AgentMessage::Models(models)) => LinkUpdate::Models(models),
                Ok(AgentMessage::Labels(shared)) => LinkUpdate::Labels(shared),
                Ok(AgentMessage::Shown(shown)) => LinkUpdate::Shown(shown),
                Ok(AgentMessage::Rejected(reason)) => return Ended::Rejected(reason),
                Ok(AgentMessage::Welcome(_)) => {
                    return lost(io::Error::other("agent sent a second welcome"), true);
                }
                Err(error) => return lost(error, true),
            };
            if !self.send(update) {
                return Ended::Stopped;
            }
        }
    }

    fn send(&self, update: LinkUpdate) -> bool {
        !self.stop.load(Ordering::Relaxed)
            && self
                .events
                .send(Event::Link(self.endpoint, self.generation, update))
                .is_ok()
    }

    fn sleep_unless_stopped(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            if self.stop.load(Ordering::Relaxed) {
                return true;
            }
            thread::sleep(STOP_POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
        self.stop.load(Ordering::Relaxed)
    }
}

fn describe(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => "agent not running".into(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "no answer".into(),
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe => "agent closed the connection".into(),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::mpsc::{self, Receiver};

    use super::*;
    use crate::proto::{Identity, Sample, Welcome};

    const ENDPOINT: Endpoint = Endpoint::Tailnet(Ipv4Addr::LOCALHOST);

    fn welcome(interval_ms: u32) -> AgentMessage {
        AgentMessage::Welcome(Welcome {
            interval_ms,
            identity: Identity {
                hostname: "fake".into(),
                tailscale_node: None,
                os: String::new(),
                kernel: String::new(),
                arch: String::new(),
                cpu_model: String::new(),
                core_kinds: Vec::new(),
                agent_version: String::new(),
            },
        })
    }

    fn accept_hello(listener: &TcpListener) -> TcpStream {
        let (mut stream, _) = listener.accept().unwrap();
        let hello: Hello = proto::read_frame(&mut stream, &mut Vec::new()).unwrap();
        assert_eq!(hello.version, PROTOCOL_VERSION);
        stream
    }

    fn next(events: &Receiver<Event>) -> LinkUpdate {
        match events
            .recv_timeout(Duration::from_secs(10))
            .expect("link went quiet")
        {
            Event::Link(endpoint, 7, update) if endpoint == ENDPOINT => update,
            _ => panic!("unexpected event"),
        }
    }

    fn start(listener: &TcpListener) -> (LinkHandle, Receiver<Event>) {
        let (sender, events) = mpsc::sync_channel(64);
        let link = spawn(
            ENDPOINT,
            Dial::Tcp(listener.local_addr().unwrap()),
            7,
            sender,
        )
        .unwrap();
        (link, events)
    }

    #[test]
    fn reports_drops_and_garbage_then_reconnects_by_itself() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (_link, events) = start(&listener);
        let server = thread::spawn(move || {
            let mut stream = accept_hello(&listener);
            proto::write_frame(&mut stream, &welcome(100)).unwrap();
            proto::write_frame(&mut stream, &AgentMessage::Sample(Sample::default())).unwrap();
            drop(stream);

            let mut stream = accept_hello(&listener);
            stream.write_all(&u32::MAX.to_le_bytes()).unwrap();
            drop(stream);

            let mut stream = accept_hello(&listener);
            stream.write_all(&[3, 0, 0, 0, 0xff, 0xff, 0xff]).unwrap();
            drop(stream);

            let mut stream = accept_hello(&listener);
            proto::write_frame(&mut stream, &welcome(100)).unwrap();
            thread::sleep(Duration::from_secs(1));
        });

        assert!(matches!(next(&events), LinkUpdate::Welcome(_)));
        assert!(matches!(next(&events), LinkUpdate::Sample(_)));
        assert!(
            matches!(next(&events), LinkUpdate::Lost(reason) if reason == "agent closed the connection")
        );
        assert!(matches!(next(&events), LinkUpdate::Lost(reason) if reason.contains("exceeds")));
        assert!(matches!(next(&events), LinkUpdate::Lost(_)));
        assert!(matches!(next(&events), LinkUpdate::Welcome(_)));
        server.join().unwrap();
    }

    #[test]
    fn a_silent_agent_is_declared_lost_even_if_it_claims_a_huge_interval() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (_link, events) = start(&listener);
        let server = thread::spawn(move || {
            let mut stream = accept_hello(&listener);
            proto::write_frame(&mut stream, &welcome(1_000)).unwrap();
            thread::sleep(MIN_SILENCE_TIMEOUT + Duration::from_secs(1));
        });
        let started = Instant::now();
        assert!(matches!(next(&events), LinkUpdate::Welcome(_)));
        assert!(matches!(next(&events), LinkUpdate::Lost(reason) if reason == "no answer"));
        assert!(started.elapsed() < MIN_SILENCE_TIMEOUT + Duration::from_secs(2));
        server.join().unwrap();
        assert_eq!(
            (Duration::from_millis(u64::from(u32::MAX)) * 3)
                .clamp(MIN_SILENCE_TIMEOUT, MAX_SILENCE_TIMEOUT),
            MAX_SILENCE_TIMEOUT
        );
    }

    #[test]
    fn a_stopped_link_sends_nothing_more() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (link, events) = start(&listener);
        let server = thread::spawn(move || {
            let mut stream = accept_hello(&listener);
            proto::write_frame(&mut stream, &welcome(100)).unwrap();
            for _ in 0..20 {
                if proto::write_frame(&mut stream, &AgentMessage::Sample(Sample::default()))
                    .is_err()
                {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        assert!(matches!(next(&events), LinkUpdate::Welcome(_)));
        drop(link);
        thread::sleep(Duration::from_millis(100));
        while events.try_recv().is_ok() {}
        thread::sleep(Duration::from_millis(400));
        assert!(events.try_recv().is_err());
        server.join().unwrap();
    }
}
