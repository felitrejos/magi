use std::collections::HashSet;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::alerts;
use super::client::{Client, short_id};
use super::{fleet, watch};
use crate::log;
use crate::proto::{OpenIn, Request, SessionState, Waiting};

const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const START_WITHIN: Duration = Duration::from_secs(90);
const LINK_WITHIN: Duration = Duration::from_secs(40);
const LINK_EVERY: Duration = Duration::from_millis(500);
const MOVE_WITHIN: Duration = Duration::from_secs(30 * 60);
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
    Move {
        session: String,
        to: String,
    },
    Start {
        machine: String,
        place: OpenIn,
        codex: bool,
        model: Option<String>,
    },
    Events,
    Subagents {
        json: bool,
    },
    Recent {
        session: String,
        subagent: Option<String>,
        items: usize,
    },
    Wait {
        session: String,
        subagent: Option<String>,
        within: Duration,
    },
}

pub fn run(port: u16, command: Cli) -> io::Result<()> {
    let mut client = Client::connect(port)?;
    let said = match command {
        Cli::Sessions { json } => return sessions(&client, json),
        Cli::Events => return events(&mut client),
        Cli::Subagents { json } => return subagents(&client, json),
        Cli::Recent {
            session,
            subagent,
            items,
        } => watch::recent(&mut client, &session, subagent.as_deref(), items),
        Cli::Wait {
            session,
            subagent,
            within,
        } => watch::wait(&mut client, &session, subagent.as_deref(), within),
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
        Cli::Move { session, to } => move_to(&mut client, &session, &to),
        Cli::Start {
            machine,
            place,
            codex,
            model,
        } => start(
            &mut client,
            &machine,
            Request::Open {
                place,
                codex,
                model,
            },
        ),
    };
    let text = said.map_err(io::Error::other)?;
    writeln!(io::stdout(), "{text}")
}

fn subagents(client: &Client, as_json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if as_json {
        serde_json::to_writer(&mut out, &watch::subagents(client))?;
        return writeln!(out);
    }
    for line in watch::subagent_lines(client) {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn sessions(client: &Client, as_json: bool) -> io::Result<()> {
    let agents = client.model.agents(Instant::now());
    let mut out = io::stdout().lock();
    if as_json {
        serde_json::to_writer(&mut out, &listed(client))?;
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
            SessionState::Waiting(Waiting::Trust) => "asks to trust its folder".into(),
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

pub(super) fn start(
    client: &mut Client,
    machine: &str,
    request: Request,
) -> Result<String, String> {
    let machine = client.machine(machine)?;
    let endpoint = client
        .endpoint(&machine)
        .ok_or("that machine isn't connected")?;
    let known: HashSet<String> = client
        .model
        .agents(Instant::now())
        .iter()
        .map(|agent| agent.session.id.clone())
        .collect();
    let said = client.ask(endpoint, &request, START_WITHIN)?;
    let deadline = Instant::now() + LINK_WITHIN;
    let mut started = None;
    while Instant::now() < deadline {
        let now = Instant::now();
        let fresh = client
            .model
            .agents(now)
            .into_iter()
            .find(|agent| agent.endpoint == endpoint && !known.contains(&agent.session.id))
            .map(|agent| agent.session.clone());
        if let Some(session) = fresh {
            let linked = session.remote.is_some();
            started = Some(session);
            if linked {
                break;
            }
        }
        client.wait(LINK_EVERY).map_err(|error| error.to_string())?;
    }
    Ok(match started {
        Some(session) if session.state == SessionState::Waiting(Waiting::Trust) => format!(
            "{said}\n{} is asking you to trust its folder: magi answer {} yes",
            session.title, session.id
        ),
        Some(session) => match session.remote_url() {
            Some(url) => format!(
                "{said}\n{} ({}): {url}",
                session.title,
                short_id(&session.id)
            ),
            None => format!(
                "{said}\n{} ({}) has no Remote Control link yet; magi sessions shows it once it has",
                session.title,
                short_id(&session.id)
            ),
        },
        None => format!("{said}\nIt hasn't shown up yet; magi sessions lists it once it does"),
    })
}

pub(super) fn listed(client: &Client) -> Value {
    let agents = client.model.agents(Instant::now());
    Value::Array(
        agents
            .iter()
            .map(|agent| {
                let mut session = fleet::session(agent.session);
                session["machine"] = json!(agent.machine);
                session["live"] = json!(agent.live);
                session
            })
            .collect(),
    )
}

pub(super) fn answer(client: &mut Client, query: &str, allow: bool) -> Result<String, String> {
    let found = client.session(query)?;
    let tool = match &found.session.state {
        SessionState::Waiting(Waiting::Permission(tool)) => tool.clone(),
        SessionState::Waiting(Waiting::Trust) => None,
        _ => {
            return Err(format!(
                "{} isn't asking for permission",
                found.session.title
            ));
        }
    };
    let request = Request::Answer {
        session: found.session.id.clone(),
        allow,
        tool,
        waiting_secs: found.session.state_secs,
    };
    client.ask(found.endpoint, &request, ANSWER_WITHIN)
}

pub(super) fn choose(client: &mut Client, query: &str, option: u8) -> Result<String, String> {
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

pub(super) fn move_to(client: &mut Client, query: &str, to: &str) -> Result<String, String> {
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
