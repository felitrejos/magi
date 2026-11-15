use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::bounded;
use crate::proto::{AgentSession, MAX_PROMPT_CHARS, SessionState, Waiting};
use crate::sessions::rex::Location;

const REX_LIMIT: Duration = Duration::from_secs(3);
const SUBMIT_AFTER: Duration = Duration::from_millis(300);
const ALLOW: &str = "1";
const DENY: &str = "\x1b";

static TYPING: Mutex<()> = Mutex::new(());

pub struct Seen {
    pub tool: Option<String>,
    pub waiting_secs: u32,
}

pub fn prompt(
    session: &AgentSession,
    pane: Option<&Location>,
    text: &str,
) -> Result<String, String> {
    let pane = pane.ok_or_else(|| not_in_rex(session))?;
    let text = prompt_text(text)?;
    ready_for_prompt(session)?;
    let _typing = TYPING.lock().unwrap_or_else(PoisonError::into_inner);
    send(pane, &text)?;
    thread::sleep(SUBMIT_AFTER);
    send(pane, "\r")?;
    Ok(format!("Sent to {}", session.title))
}

pub fn answer(
    session: &AgentSession,
    pane: Option<&Location>,
    allow: bool,
    seen: &Seen,
) -> Result<String, String> {
    let pane = pane.ok_or_else(|| not_in_rex(session))?;
    let tool = same_prompt(session, seen)?;
    let _typing = TYPING.lock().unwrap_or_else(PoisonError::into_inner);
    send(pane, if allow { ALLOW } else { DENY })?;
    let what = tool.as_deref().unwrap_or("it");
    let verb = if allow { "Allowed" } else { "Denied" };
    Ok(format!("{verb} {what} in {}", session.title))
}

fn same_prompt<'a>(session: &'a AgentSession, seen: &Seen) -> Result<&'a Option<String>, String> {
    match &session.state {
        SessionState::Waiting(Waiting::Permission(tool))
            if *tool == seen.tool && session.state_secs + 1 >= seen.waiting_secs =>
        {
            Ok(tool)
        }
        SessionState::Waiting(Waiting::Permission(_)) => Err(format!(
            "{} is asking something new: look again before answering",
            session.title
        )),
        _ => Err(format!("{} isn't asking for permission now", session.title)),
    }
}

fn not_in_rex(session: &AgentSession) -> String {
    format!(
        "{} isn't in a Rex pane, so magi can't type into it",
        session.title
    )
}

fn prompt_text(text: &str) -> Result<String, String> {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return Err("there's nothing to send".into());
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "a message can be at most {MAX_PROMPT_CHARS} characters"
        ));
    }
    Ok(text.to_string())
}

fn ready_for_prompt(session: &AgentSession) -> Result<(), String> {
    match &session.state {
        SessionState::Idle => Ok(()),
        SessionState::Working { .. } => Err(format!(
            "{} is still working: send it when it's idle",
            session.title
        )),
        SessionState::Waiting(Waiting::Permission(_)) => Err(format!(
            "{} is asking for permission: allow or deny it first",
            session.title
        )),
        SessionState::Waiting(Waiting::Input(_)) => Err(format!(
            "{} is waiting on a question in its pane: open it to answer",
            session.title
        )),
    }
}

fn send(pane: &Location, text: &str) -> Result<(), String> {
    let output = bounded::output(
        &mut pane.rex(
            "2s",
            &["send", "-s", &pane.session, "-b", &pane.block, "--", text],
        ),
        REX_LIMIT,
    )
    .map_err(|error| format!("can't run rex: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("rex failed")
        .trim_start_matches("Error: ");
    Err(format!("Rex couldn't type into the pane: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Agent;

    fn session(agent: Agent, state: SessionState) -> AgentSession {
        AgentSession {
            agent,
            id: "s1".into(),
            title: "fix the build".into(),
            project: String::new(),
            state,
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
        }
    }

    #[test]
    fn prompts_become_one_clean_line_within_the_limit() {
        assert_eq!(
            prompt_text("  run\nthe\ttests\x1b[2J "),
            Ok("run the tests [2J".into())
        );
        assert!(prompt_text(" \n ").is_err());
        assert!(prompt_text(&"x".repeat(MAX_PROMPT_CHARS)).is_ok());
        assert!(prompt_text(&"x".repeat(MAX_PROMPT_CHARS + 1)).is_err());
    }

    #[test]
    fn only_an_idle_session_takes_a_message() {
        let ready = |state| ready_for_prompt(&session(Agent::ClaudeCode, state)).is_ok();
        assert!(ready(SessionState::Idle));
        assert!(!ready(SessionState::Working { tool: None }));
        assert!(!ready(SessionState::Waiting(Waiting::Permission(None))));
        assert!(!ready(SessionState::Waiting(Waiting::Input(
            "question".into()
        ))));
    }

    #[test]
    fn an_answer_goes_only_to_the_prompt_the_viewer_saw() {
        let mut asking = session(
            Agent::ClaudeCode,
            SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
        );
        asking.state_secs = 9;
        let seen = |tool: &str, waiting_secs| Seen {
            tool: Some(tool.into()),
            waiting_secs,
        };
        assert!(same_prompt(&asking, &seen("Bash", 10)).is_ok());
        assert!(same_prompt(&asking, &seen("Bash", 7)).is_ok());
        assert_eq!(
            same_prompt(&asking, &seen("Edit", 9)),
            Err("fix the build is asking something new: look again before answering".into())
        );
        assert!(same_prompt(&asking, &seen("Bash", 30)).is_err());
        let idle = session(Agent::ClaudeCode, SessionState::Idle);
        assert_eq!(
            same_prompt(&idle, &seen("Bash", 0)),
            Err("fix the build isn't asking for permission now".into())
        );
    }

    #[test]
    fn nothing_is_typed_into_a_session_outside_rex() {
        let asking = session(
            Agent::ClaudeCode,
            SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
        );
        let seen = Seen {
            tool: Some("Bash".into()),
            waiting_secs: 0,
        };
        assert_eq!(
            answer(&asking, None, true, &seen),
            Err("fix the build isn't in a Rex pane, so magi can't type into it".into())
        );
        assert_eq!(
            prompt(&session(Agent::ClaudeCode, SessionState::Idle), None, "hi"),
            Err("fix the build isn't in a Rex pane, so magi can't type into it".into())
        );
    }
}
