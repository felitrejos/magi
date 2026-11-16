use std::collections::HashSet;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde_json::json;

use super::alerts;
use super::client::{Client, Found, short_id};
use super::fleet;
use crate::log;
use crate::proto::{OpenIn, PermissionMode, Request, SessionState, Waiting};

const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const START_WITHIN: Duration = Duration::from_secs(90);
const MOVE_WITHIN: Duration = Duration::from_secs(30 * 60);
const MODES_IN_A_CYCLE: usize = 5;
const EVENTS_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq)]
pub enum Cli {
    Sessions {
        json: bool,
    },
    Send {
        session: String,
        text: String,
    },
    Answer {
        session: String,
        allow: bool,
    },
    Choose {
        session: String,
        option: u8,
    },
    Mode {
        session: String,
        to: Option<PermissionMode>,
    },
    Move {
        session: String,
        to: String,
    },
    Start {
        machine: String,
        place: OpenIn,
        project: Option<String>,
        codex: bool,
        model: Option<String>,
    },
    Events,
}

pub fn run(port: u16, command: Cli) -> io::Result<()> {
    let mut client = Client::connect(port)?;
    let said = match command {
        Cli::Sessions { json } => return sessions(&client, json),
        Cli::Events => return events(&mut client),
        Cli::Send { session, text } => {
            let found = client.session(&session).map_err(io::Error::other)?;
            let request = Request::Prompt {
                session: found.session.id.clone(),
                text,
            };
            client.ask(found.endpoint, &request, ANSWER_WITHIN)
        }
        Cli::Answer { session, allow } => answer(&mut client, &session, allow),
        Cli::Choose { session, option } => choose(&mut client, &session, option),
        Cli::Mode { session, to } => mode(&mut client, &session, to),
        Cli::Move { session, to } => move_to(&mut client, &session, &to),
        Cli::Start {
            machine,
            place,
            project,
            codex,
            model,
        } => {
            let machine = client.machine(&machine).map_err(io::Error::other)?;
            let endpoint = client
                .endpoint(&machine)
                .ok_or_else(|| io::Error::other("that machine isn't connected"))?;
            let request = Request::Open {
                place,
                project,
                codex,
                model,
            };
            client.ask(endpoint, &request, START_WITHIN)
        }
    };
    let text = said.map_err(io::Error::other)?;
    writeln!(io::stdout(), "{text}")
}

fn sessions(client: &Client, as_json: bool) -> io::Result<()> {
    let agents = client.model.agents(Instant::now());
    let mut out = io::stdout().lock();
    if as_json {
        let listed: Vec<_> = agents
            .iter()
            .map(|agent| {
                let mut session = fleet::session(agent.session);
                session["machine"] = json!(agent.machine);
                session["live"] = json!(agent.live);
                session
            })
            .collect();
        serde_json::to_writer(&mut out, &listed)?;
        return writeln!(out);
    }
    for agent in &agents {
        let session = agent.session;
        let state = match &session.state {
            _ if !agent.live => "not answering".to_string(),
            SessionState::Waiting(Waiting::Permission(tool)) => {
                format!("asks to run {}", tool.as_deref().unwrap_or("something"))
            }
            SessionState::Waiting(Waiting::Input(_)) => "asks a question".into(),
            SessionState::Working { .. } => "working".into(),
            SessionState::Idle => "idle".into(),
        };
        writeln!(
            out,
            "{}  {:<6}  {:<18}  {:<14}  {:<20}  {}",
            short_id(&session.id),
            fleet::agent_key(session.agent),
            clip(&agent.machine, 18),
            clip(&session.project, 14),
            clip(&state, 20),
            session.title,
        )?;
    }
    Ok(())
}

fn answer(client: &mut Client, query: &str, allow: bool) -> Result<String, String> {
    let found = client.session(query)?;
    let SessionState::Waiting(Waiting::Permission(tool)) = &found.session.state else {
        return Err(format!(
            "{} isn't asking for permission",
            found.session.title
        ));
    };
    let request = Request::Answer {
        session: found.session.id.clone(),
        allow,
        tool: tool.clone(),
        waiting_secs: found.session.state_secs,
    };
    client.ask(found.endpoint, &request, ANSWER_WITHIN)
}

fn choose(client: &mut Client, query: &str, option: u8) -> Result<String, String> {
    let found = client.session(query)?;
    if !matches!(
        found.session.state,
        SessionState::Waiting(Waiting::Input(_))
    ) {
        return Err(format!("{} isn't asking a question", found.session.title));
    }
    let request = Request::Choose {
        session: found.session.id.clone(),
        option,
        waiting_secs: found.session.state_secs,
    };
    client.ask(found.endpoint, &request, ANSWER_WITHIN)
}

fn mode(client: &mut Client, query: &str, to: Option<PermissionMode>) -> Result<String, String> {
    let Found {
        endpoint, session, ..
    } = client.session(query)?;
    if to.is_some() && session.mode == to {
        return Ok(format!(
            "{} is already in {} mode",
            session.title,
            session.mode.map_or("", PermissionMode::name)
        ));
    }
    let request = Request::CycleMode {
        session: session.id.clone(),
    };
    for _ in 0..MODES_IN_A_CYCLE {
        let said = client.ask(endpoint, &request, ANSWER_WITHIN)?;
        match to {
            Some(to) if !said.ends_with(&format!("in {} mode", to.name())) => continue,
            _ => return Ok(said),
        }
    }
    Err(format!(
        "{} never reached that mode; it may not offer it",
        session.title
    ))
}

fn move_to(client: &mut Client, query: &str, to: &str) -> Result<String, String> {
    let found = client.session(query)?;
    let to = client.machine(to)?;
    let request = Request::Move {
        session: found.session.id.clone(),
        to,
    };
    let first = client.ask(found.endpoint, &request, ANSWER_WITHIN)?;
    let _ = writeln!(io::stdout(), "{first}");
    client.reply(found.endpoint, MOVE_WITHIN)
}

fn events(client: &mut Client) -> io::Result<()> {
    let mut out = io::stdout().lock();
    let mut active: HashSet<String> = HashSet::new();
    loop {
        client.wait(EVENTS_EVERY)?;
        let now = Instant::now();
        let now_secs = log::now_secs();
        let conditions = alerts::conditions(&client.model, now, now_secs);
        let changes = client.model.alerts.update(conditions, now, now_secs);
        for condition in &changes.raised {
            serde_json::to_writer(
                &mut out,
                &json!({
                    "event": if active.insert(condition.key.clone()) { "raised" } else { "updated" },
                    "key": condition.key,
                    "machine": condition.machine,
                    "text": condition.text,
                    "severity": condition.severity,
                    "at": now_secs,
                }),
            )?;
            writeln!(out)?;
        }
        for key in &changes.resolved {
            active.remove(key);
            serde_json::to_writer(
                &mut out,
                &json!({"event": "resolved", "key": key, "at": now_secs}),
            )?;
            writeln!(out)?;
        }
        out.flush()?;
    }
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(width - 1).collect();
    clipped.push('…');
    clipped
}
