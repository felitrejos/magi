use std::thread;
use std::time::Duration;

use crate::bounded;
use crate::proto::{Agent, AgentSession, MAX_PROMPT_CHARS, SessionState, Waiting};
use crate::sessions::rex::Location;

const REX_LIMIT: Duration = Duration::from_secs(3);
const SUBMIT_AFTER: Duration = Duration::from_millis(300);
const ALLOW: &str = "1";
const DENY: &str = "\x1b";

pub fn prompt(
    session: &AgentSession,
    pane: Option<&Location>,
    text: &str,
) -> Result<String, String> {
    let pane = pane.ok_or_else(|| not_in_rex(session))?;
    let text = prompt_text(text)?;
    ready_for_prompt(session)?;
    send(pane, &text)?;
    thread::sleep(SUBMIT_AFTER);
    send(pane, "\r")?;
    Ok(format!("Sent to {}", session.title))
}

pub fn answer(
    session: &AgentSession,
    pane: Option<&Location>,
    allow: bool,
) -> Result<String, String> {
    let pane = pane.ok_or_else(|| not_in_rex(session))?;
    let SessionState::Waiting(Waiting::Permission(tool)) = &session.state else {
        return Err(format!("{} isn't asking for permission now", session.title));
    };
    send(pane, if allow { ALLOW } else { DENY })?;
    let what = tool.as_deref().unwrap_or("it");
    let verb = if allow { "Allowed" } else { "Denied" };
    Ok(format!("{verb} {what} in {}", session.title))
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
    match (&session.state, session.agent) {
        (SessionState::Idle, _) | (SessionState::Working { .. }, Agent::ClaudeCode) => Ok(()),
        (SessionState::Working { .. }, Agent::Codex) => Err(format!(
            "{} is still working; send it when it's idle",
            session.title
        )),
        (SessionState::Waiting(Waiting::Permission(_)), _) => Err(format!(
            "{} is asking for permission: allow or deny it first",
            session.title
        )),
        (SessionState::Waiting(Waiting::Input(_)), _) => Err(format!(
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
    fn claude_takes_prompts_while_working_codex_only_when_idle_and_neither_while_asking() {
        let working = SessionState::Working { tool: None };
        let ready = |agent, state| ready_for_prompt(&session(agent, state)).is_ok();
        assert!(ready(Agent::ClaudeCode, SessionState::Idle));
        assert!(ready(Agent::ClaudeCode, working.clone()));
        assert!(ready(Agent::Codex, SessionState::Idle));
        assert!(!ready(Agent::Codex, working));
        assert!(!ready(
            Agent::ClaudeCode,
            SessionState::Waiting(Waiting::Permission(None))
        ));
        assert!(!ready(
            Agent::ClaudeCode,
            SessionState::Waiting(Waiting::Input("question".into()))
        ));
    }

    #[test]
    fn nothing_is_typed_into_a_session_outside_rex_or_not_asking() {
        let asking = session(
            Agent::ClaudeCode,
            SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
        );
        assert_eq!(
            answer(&asking, None, true),
            Err("fix the build isn't in a Rex pane, so magi can't type into it".into())
        );
        assert_eq!(
            prompt(&session(Agent::ClaudeCode, SessionState::Idle), None, "hi"),
            Err("fix the build isn't in a Rex pane, so magi can't type into it".into())
        );
    }
}
