use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{Answer, Client, Found, pick, short_id};
use crate::proto::{
    AgentSession, Child, ChildKind, ChildState, Content, Request, Said, SessionState,
};

const READ_WITHIN: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(1);

pub fn subagents(client: &Client) -> Vec<Value> {
    client
        .model
        .agents(Instant::now())
        .iter()
        .flat_map(|agent| {
            running(&agent.session.children).map(|child| {
                json!({
                    "machine": agent.machine,
                    "session": agent.session.id,
                    "session_title": agent.session.title,
                    "id": child.id,
                    "label": child.label,
                    "doing": child.doing,
                    "secs": child.secs,
                })
            })
        })
        .collect()
}

pub fn subagent_lines(client: &Client) -> Vec<String> {
    subagents(client)
        .iter()
        .map(|found| {
            let text = |key: &str| found[key].as_str().unwrap_or_default().to_string();
            format!(
                "{}  {}  {} ↳ {}{}",
                short_id(&text("session")),
                text("machine"),
                text("session_title"),
                text("label"),
                found["doing"]
                    .as_str()
                    .map_or_else(String::new, |doing| format!(": {doing}")),
            )
        })
        .collect()
}

pub fn recent(
    client: &mut Client,
    session: &str,
    subagent: Option<&str>,
    items: usize,
) -> Result<String, String> {
    let found = client.session(session)?;
    let child = subagent
        .map(|query| child(&found, query).map(|child| child.id.clone()))
        .transpose()?;
    let id = found.session.id.clone();
    client.send(
        found.endpoint,
        &Request::Read {
            session: id.clone(),
            child: child.clone(),
        },
    )?;
    let view = client.answer(found.endpoint, READ_WITHIN, |answer| match answer {
        Answer::Read(read) if read.session == id && read.child == child => Some(read.view),
        _ => None,
    })?;
    Ok(match view? {
        Content::Conversation(said) => said[said.len().saturating_sub(items)..]
            .iter()
            .map(step)
            .collect::<Vec<_>>()
            .join("\n"),
        Content::Screen(screen) => screen,
    })
}

pub fn wait(
    client: &mut Client,
    session: &str,
    subagent: Option<&str>,
    within: Duration,
) -> Result<String, String> {
    let found = client.session(session)?;
    let id = found.session.id.clone();
    let child = subagent
        .map(|query| child(&found, query).map(|child| child.id.clone()))
        .transpose()?;
    let deadline = Instant::now() + within;
    loop {
        let agents = client.model.agents(Instant::now());
        let Some(now) = agents.iter().find(|agent| agent.session.id == id) else {
            return Ok(format!("{} has ended", found.session.title));
        };
        let settled = settled(now.session, child.as_deref());
        if let Some(settled) = settled {
            return Ok(settled);
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| format!("{} is still working", found.session.title))?;
        client
            .wait(left.min(CHECK_EVERY))
            .map_err(|error| error.to_string())?;
    }
}

fn settled(session: &AgentSession, child: Option<&str>) -> Option<String> {
    match child {
        Some(child) => {
            let known = session.children.iter().find(|known| known.id == child);
            known
                .filter(|known| known.state == ChildState::Running)
                .is_none()
                .then(|| {
                    let label = known.map_or("its subagent", |known| known.label.as_str());
                    format!("{} ↳ {label} has finished", session.title)
                })
        }
        None => match &session.state {
            SessionState::Working { .. } => None,
            SessionState::Idle => Some(format!("{} is idle", session.title)),
            SessionState::Waiting(_) => Some(format!("{} needs you", session.title)),
        },
    }
}

fn running(children: &[Child]) -> impl Iterator<Item = &Child> {
    children
        .iter()
        .filter(|child| child.kind == ChildKind::Subagent && child.state == ChildState::Running)
}

fn child<'a>(found: &'a Found, query: &str) -> Result<&'a Child, String> {
    let subagents: Vec<&Child> = found
        .session
        .children
        .iter()
        .filter(|child| child.kind == ChildKind::Subagent)
        .collect();
    let candidates: Vec<(&str, &str)> = subagents
        .iter()
        .map(|child| {
            (
                child.task.as_deref().unwrap_or(&child.id),
                child.label.as_str(),
            )
        })
        .collect();
    pick(&candidates, query)
        .map(|index| subagents[index])
        .map_err(|error| error.replace("session", "subagent"))
}

pub fn step(said: &Said) -> String {
    match said {
        Said::You(text) => format!("User: {text}"),
        Said::Reply(text) => format!("Agent: {text}"),
        Said::Tool {
            name,
            detail,
            change,
            ..
        } => {
            let change = change.map_or_else(String::new, |change| {
                format!(" (+{} −{})", change.added, change.removed)
            });
            let detail = detail.lines().next().unwrap_or_default();
            format!("Tool {name}: {detail}{change}")
        }
        Said::Plan(steps) => {
            let steps: Vec<String> = steps
                .iter()
                .map(|step| format!("- {}", step.text))
                .collect();
            format!("Plan:\n{}", steps.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Waiting;

    fn subagent(id: &str, label: &str, state: ChildState) -> Child {
        Child {
            id: id.into(),
            kind: ChildKind::Subagent,
            label: label.into(),
            detail: String::new(),
            state,
            secs: 0,
            task: Some(format!("agent-{id}")),
            doing: None,
        }
    }

    #[test]
    fn a_wait_ends_when_the_session_stops_working_or_its_subagent_finishes() {
        let mut session = AgentSession::example("s1");
        session.title = "fix login".into();
        session.state = SessionState::Working { tool: None };
        session.children = vec![subagent("t1", "find auth", ChildState::Running)];
        assert_eq!(settled(&session, None), None);
        assert_eq!(settled(&session, Some("t1")), None);
        session.children[0].state = ChildState::Done;
        assert_eq!(
            settled(&session, Some("t1")).as_deref(),
            Some("fix login ↳ find auth has finished")
        );
        session.state = SessionState::Waiting(Waiting::Permission(None));
        assert_eq!(
            settled(&session, None).as_deref(),
            Some("fix login needs you")
        );
        session.state = SessionState::Idle;
        assert_eq!(
            settled(&session, None).as_deref(),
            Some("fix login is idle")
        );
    }

    #[test]
    fn a_step_takes_one_line() {
        let tool = Said::Tool {
            name: "Bash".into(),
            detail: "cargo test\n--release".into(),
            change: None,
            running_since: None,
        };
        assert_eq!(step(&tool), "Tool Bash: cargo test");
    }
}
