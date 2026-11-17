use std::fs;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use super::*;
use crate::proto::PermissionMode;

fn working(id: &str) -> AgentSession {
    AgentSession {
        title: "fix the build".into(),
        state: SessionState::Working { tool: None },
        mode: Some(PermissionMode::Auto),
        ..AgentSession::example(id)
    }
}

#[test]
fn a_message_for_a_session_asking_to_trust_its_folder_is_refused_not_queued() {
    let agent = Agent::new(identity(), Duration::from_secs(1), Board::default());
    let pane = Location::in_test("session:1", "block:1");
    let mut trusting = working("trust:block:1");
    trusting.state = SessionState::Waiting(proto::Waiting::Trust);
    assert!(
        agent
            .queue(&trusting, Some(&pane), "fix the login")
            .unwrap_err()
            .contains("trust its folder")
    );
    assert!(agent.queued.lock().unwrap().is_empty());
}

#[test]
fn a_message_for_a_busy_session_waits_until_it_is_idle_and_can_be_dropped() {
    let agent = Agent::new(identity(), Duration::from_secs(1), Board::default());
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

fn identity() -> Identity {
    Identity {
        hostname: "test".into(),
        tailscale_node: None,
        os: String::new(),
        arch: String::new(),
        tools: Vec::new(),
        cpu_model: String::new(),
        core_kinds: Vec::new(),
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
    let agent = Arc::new(Agent::new(identity(), interval, Board::default()));
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
fn reading_a_session_the_agent_does_not_know_says_it_has_ended() {
    let agent = start_local_agent("read", Duration::from_millis(100));
    let (mut stream, reply) = connect(&agent, PROTOCOL_VERSION);
    assert!(matches!(reply, AgentMessage::Welcome(_)), "{reply:?}");
    proto::write_frame(
        &mut stream,
        &Request::Read {
            session: "elsewhere".into(),
            child: None,
        },
    )
    .unwrap();
    let mut buf = Vec::new();
    let read = (0..50)
        .find_map(
            |_| match proto::read_frame(&mut stream, &mut buf).unwrap() {
                AgentMessage::Read(read) => Some(read),
                _ => None,
            },
        )
        .expect("a read among the samples");
    assert_eq!(
        read,
        View {
            session: "elsewhere".into(),
            child: None,
            view: Err("that session has ended".into()),
        }
    );
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
    let agent = Agent::new(identity(), Duration::from_secs(1), Board::default());
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
