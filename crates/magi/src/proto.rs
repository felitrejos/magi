use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 5;
pub const DEFAULT_PORT: u16 = 2015;
const MAX_FRAME_BYTES: usize = 1 << 20;

pub type BasisPoints = u16;
pub const FULL: BasisPoints = 10_000;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Hello {
    pub version: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StatusReport {
    pub session_id: String,
    pub context_used: Option<BasisPoints>,
    pub windows: Vec<Window>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Agent {
    ClaudeCode,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Codex => "Codex",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Plan {
    pub agent: Agent,
    pub windows: Vec<Window>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub minutes: u32,
    pub used: BasisPoints,
    pub resets_at: u64,
}

pub const FIVE_HOURS: u32 = 5 * 60;
pub const ONE_WEEK: u32 = 7 * 24 * 60;

impl Window {
    pub fn label(&self) -> String {
        match self.minutes {
            ONE_WEEK => "wk".into(),
            minutes if minutes % (24 * 60) == 0 => format!("{}d", minutes / (24 * 60)),
            minutes if minutes % 60 == 0 => format!("{}h", minutes / 60),
            minutes => format!("{minutes}m"),
        }
    }

    pub fn long_label(&self) -> String {
        match self.minutes {
            ONE_WEEK => "week".into(),
            minutes if minutes % (24 * 60) == 0 => format!("{}-day", minutes / (24 * 60)),
            minutes if minutes % 60 == 0 => format!("{}-hour", minutes / 60),
            minutes => format!("{minutes}-minute"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AgentSession {
    pub agent: Agent,
    pub id: String,
    pub title: String,
    pub project: String,
    pub state: SessionState,
    pub state_secs: u32,
    pub model: String,
    pub context_tokens: u64,
    pub context_used: Option<BasisPoints>,
    pub tokens: Tokens,
    pub cost_micros: u64,
    pub cost_complete: bool,
    pub age_secs: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SessionState {
    Working { tool: Option<String> },
    Waiting(Waiting),
    Idle,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Waiting {
    Permission(Option<String>),
    Input(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl std::ops::Add for Tokens {
    type Output = Tokens;

    fn add(self, other: Tokens) -> Tokens {
        Tokens {
            input: self.input + other.input,
            output: self.output + other.output,
            cache_read: self.cache_read + other.cache_read,
            cache_write: self.cache_write + other.cache_write,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AgentMessage {
    Welcome(Welcome),
    Sample(Sample),
    Rejected(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Welcome {
    pub interval_ms: u32,
    pub identity: Identity,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Identity {
    pub hostname: String,
    pub tailscale_node: Option<String>,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_model: String,
    pub agent_version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Sample {
    pub uptime_secs: u64,
    pub load: [f32; 3],
    pub cpu: BasisPoints,
    pub cores: Vec<BasisPoints>,
    pub memory: Memory,
    pub filesystems: Vec<Filesystem>,
    pub disk: DiskIo,
    pub network: NetIo,
    pub sessions: Vec<AgentSession>,
    pub plans: Vec<Plan>,
    pub notices: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Filesystem {
    pub mount: String,
    pub total: u64,
    pub used: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
pub struct DiskIo {
    pub read_per_sec: u64,
    pub write_per_sec: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Default)]
pub struct NetIo {
    pub rx_per_sec: u64,
    pub tx_per_sec: u64,
}

pub fn encode_frame<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    let body = postcard::to_stdvec(message).map_err(invalid_data)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&frame_len(body.len())?.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn write_frame<T: Serialize, W: Write + ?Sized>(writer: &mut W, message: &T) -> io::Result<()> {
    writer.write_all(&encode_frame(message)?)?;
    writer.flush()
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    postcard::from_bytes(bytes).map_err(invalid_data)
}

pub fn read_frame<T: DeserializeOwned, R: Read + ?Sized>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> io::Result<T> {
    let mut len = [0u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(invalid_data(format!(
            "frame of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit"
        )));
    }
    buf.resize(len, 0);
    reader.read_exact(buf)?;
    postcard::from_bytes(buf).map_err(invalid_data)
}

fn frame_len(len: usize) -> io::Result<u32> {
    if len > MAX_FRAME_BYTES {
        return Err(invalid_data(format!(
            "message of {len} bytes exceeds the {MAX_FRAME_BYTES} byte limit"
        )));
    }
    Ok(len as u32)
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Sample {
        Sample {
            uptime_secs: 86_400,
            load: [0.5, 1.25, 2.0],
            cpu: 4_210,
            cores: vec![10_000, 0, 5_000, 1_234],
            memory: Memory {
                total: 32 << 30,
                used: 12 << 30,
                swap_total: 2 << 30,
                swap_used: 1 << 20,
            },
            filesystems: vec![Filesystem {
                mount: "/".into(),
                total: 1 << 40,
                used: 1 << 39,
            }],
            disk: DiskIo {
                read_per_sec: 1_000,
                write_per_sec: 25_000_000,
            },
            network: NetIo {
                rx_per_sec: 12_345,
                tx_per_sec: 678,
            },
            sessions: Vec::new(),
            plans: Vec::new(),
            notices: Vec::new(),
        }
    }

    #[test]
    fn frames_round_trip_back_to_back() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Hello {
                version: PROTOCOL_VERSION,
            },
        )
        .unwrap();
        write_frame(&mut wire, &AgentMessage::Sample(sample())).unwrap();

        let mut reader = wire.as_slice();
        let mut buf = Vec::new();
        let hello: Hello = read_frame(&mut reader, &mut buf).unwrap();
        let message: AgentMessage = read_frame(&mut reader, &mut buf).unwrap();

        assert_eq!(hello.version, PROTOCOL_VERSION);
        assert_eq!(message, AgentMessage::Sample(sample()));
        assert!(reader.is_empty());
    }

    #[test]
    fn sample_frame_stays_small() {
        let frame = encode_frame(&AgentMessage::Sample(sample())).unwrap();
        assert!(frame.len() < 128, "sample frame is {} bytes", frame.len());
    }

    #[test]
    fn oversized_length_prefix_is_rejected_before_allocating() {
        let mut wire = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
        wire.extend_from_slice(&[0; 8]);
        let error = read_frame::<Hello, _>(&mut wire.as_slice(), &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn windows_are_labelled_by_their_length() {
        let label = |minutes| {
            Window {
                minutes,
                used: 0,
                resets_at: 0,
            }
            .label()
        };
        assert_eq!(
            [
                label(FIVE_HOURS),
                label(ONE_WEEK),
                label(2 * 24 * 60),
                label(90)
            ],
            ["5h", "wk", "2d", "90m"]
        );
        let long = |minutes| {
            Window {
                minutes,
                used: 0,
                resets_at: 0,
            }
            .long_label()
        };
        assert_eq!(
            [long(FIVE_HOURS), long(ONE_WEEK), long(2 * 24 * 60)],
            ["5-hour", "week", "2-day"]
        );
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut wire = encode_frame(&AgentMessage::Sample(sample())).unwrap();
        wire.truncate(wire.len() - 3);
        let error =
            read_frame::<AgentMessage, _>(&mut wire.as_slice(), &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
