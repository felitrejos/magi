use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::{Answer, Client};
use super::{alerts, cli, fleet};
use crate::log;
use crate::proto::{Content, OpenIn, Request, Said};

const PROTOCOLS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const PUMP_EVERY: Duration = Duration::from_millis(100);
const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const SHOWN_WITHIN: Duration = Duration::from_secs(30);
const RECENT_ITEMS: usize = 20;
const MAX_RECENT_ITEMS: usize = 200;
const MAX_TEXT: usize = 60_000;
const INSTRUCTIONS: &str = "magi sees every Claude Code and Codex session on the user's machines \
(those running a magi agent on their tailnet), with each machine's load and the user's plan \
limits. Use machines and sessions to look, recent_work and diff to see what a session did, and \
alerts for what needs the user. Actions type into the user's live sessions: say what you'll do \
and let the user decide before send_message, answer_permission, choose_option, \
move_session or start_session.";

pub fn run(port: u16) -> io::Result<()> {
    let mut client = Client::connect(port)?;
    let (lines, incoming) = mpsc::channel();
    thread::Builder::new().name("stdin".into()).spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if lines.send(line).is_err() {
                return;
            }
        }
    })?;
    let mut out = io::stdout().lock();
    loop {
        let line = match incoming.recv_timeout(PUMP_EVERY) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                client.wait(Duration::ZERO)?;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        client.wait(Duration::ZERO)?;
        if let Some(reply) = respond(&line, |name, arguments| call(&mut client, name, arguments)) {
            serde_json::to_writer(&mut out, &reply)?;
            writeln!(out)?;
            out.flush()?;
        }
    }
}

fn respond(
    line: &str,
    mut call: impl FnMut(&str, &Value) -> Result<String, String>,
) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "not JSON"}}),
        );
    };
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOLS
                .into_iter()
                .find(|known| *known == asked)
                .unwrap_or(PROTOCOLS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "magi", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let (text, failed) = match call(name, &params["arguments"]) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": failed})
        }
        method => {
            return Some(json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("magi has no method {method}")}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
        "annotations": {"readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false},
    })
}

fn tools() -> Vec<Value> {
    let session = json!({"type": "string", "description": "The session: the start of its id, its title, or a part of its title at least four letters long"});
    vec![
        tool(
            "machines",
            "Each machine's CPU, memory, load and sessions, and the plan limits",
            json!({}),
            &[],
            true,
        ),
        tool(
            "sessions",
            "Every Claude Code and Codex session: state, what it's doing or asking, permission mode, queued message, subagents",
            json!({}),
            &[],
            true,
        ),
        tool(
            "recent_work",
            "What a session said and did lately: its messages, replies and tool calls, newest last",
            json!({"session": session, "items": {"type": "integer", "description": "How many of the latest items, 20 by default"}}),
            &["session"],
            true,
        ),
        tool(
            "diff",
            "A session's uncommitted changes, as a diff",
            json!({"session": session}),
            &["session"],
            true,
        ),
        tool(
            "alerts",
            "What needs the user now: sessions asking, machines not answering or overloaded, plans running out",
            json!({}),
            &[],
            true,
        ),
        tool(
            "send_message",
            "Type a message into a session. A busy session gets it once it's idle",
            json!({"session": session, "text": {"type": "string"}}),
            &["session", "text"],
            false,
        ),
        tool(
            "answer_permission",
            "Allow or deny what a session asks to run",
            json!({"session": session, "allow": {"type": "boolean"}}),
            &["session", "allow"],
            false,
        ),
        tool(
            "choose_option",
            "Pick an option of the question a session is asking",
            json!({"session": session, "option": {"type": "integer", "minimum": 1, "maximum": 9}}),
            &["session", "option"],
            false,
        ),
        tool(
            "move_session",
            "Move a Claude Code session, with its conversation and uncommitted work, to another machine once its turn ends",
            json!({"session": session, "machine": {"type": "string"}}),
            &["session", "machine"],
            false,
        ),
        tool(
            "start_session",
            "Start a Claude Code or Codex session on a machine, in a Rex tab for its folder, with Remote Control on",
            json!({
                "machine": {"type": "string"},
                "folder": {"type": "string", "description": "A project folder on that machine; leave out with chat"},
                "chat": {"type": "boolean", "description": "Start a chat in ~/Chats instead of a project"},
                "codex": {"type": "boolean"},
                "model": {"type": "string"},
            }),
            &["machine"],
            false,
        ),
    ]
}

fn call(client: &mut Client, name: &str, arguments: &Value) -> Result<String, String> {
    let text = |key: &str| -> Result<String, String> {
        arguments[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{name} needs {key}"))
    };
    let pretty = |value: Value| serde_json::to_string_pretty(&value).unwrap_or_default();
    let now = Instant::now();
    match name {
        "machines" => {
            let machines: Vec<Value> = client.model.rows(now).iter().map(fleet::describe).collect();
            let plans: Vec<Value> = client
                .model
                .plans(now)
                .into_iter()
                .map(fleet::plan)
                .collect();
            Ok(pretty(json!({"machines": machines, "plans": plans})))
        }
        "sessions" => Ok(pretty(cli::listed(client))),
        "alerts" => {
            let now_secs = log::now_secs();
            let active: Vec<Value> = alerts::conditions(&client.model, now, now_secs)
                .into_iter()
                .map(|condition| json!({"machine": condition.machine, "text": condition.text, "severity": condition.severity}))
                .collect();
            Ok(pretty(Value::Array(active)))
        }
        "recent_work" => {
            let items = arguments["items"].as_u64().map_or(RECENT_ITEMS, |items| {
                (items as usize).clamp(1, MAX_RECENT_ITEMS)
            });
            recent_work(client, &text("session")?, items)
        }
        "diff" => diff(client, &text("session")?),
        "send_message" => {
            let found = client.session(&text("session")?)?;
            let request = Request::Prompt {
                session: found.session.id.clone(),
                text: text("text")?,
            };
            client.ask(found.endpoint, &request, ANSWER_WITHIN)
        }
        "answer_permission" => {
            let allow = arguments["allow"]
                .as_bool()
                .ok_or("answer_permission needs allow")?;
            cli::answer(client, &text("session")?, allow)
        }
        "choose_option" => {
            let option = arguments["option"]
                .as_u64()
                .and_then(|option| u8::try_from(option).ok())
                .filter(|option| (1..=9).contains(option))
                .ok_or("choose_option needs an option from 1 to 9")?;
            cli::choose(client, &text("session")?, option)
        }
        "move_session" => {
            let found = client.session(&text("session")?)?;
            let to = client.machine(&text("machine")?)?;
            let request = Request::Move {
                session: found.session.id.clone(),
                to,
            };
            client.ask(found.endpoint, &request, ANSWER_WITHIN)
        }
        "start_session" => {
            let machine = text("machine")?;
            let place = if arguments["chat"].as_bool() == Some(true) {
                OpenIn::Chats
            } else {
                OpenIn::Folder(text("folder")?)
            };
            let request = Request::Open {
                place,
                codex: arguments["codex"].as_bool() == Some(true),
                model: arguments["model"].as_str().map(str::to_string),
            };
            cli::start(client, &machine, request)
        }
        _ => Err(format!("magi has no tool {name}")),
    }
}

fn recent_work(client: &mut Client, query: &str, items: usize) -> Result<String, String> {
    let found = client.session(query)?;
    let id = found.session.id.clone();
    client.send(
        found.endpoint,
        &Request::Watch {
            session: Some(id.clone()),
            child: None,
        },
    )?;
    let view = client.answer(found.endpoint, ANSWER_WITHIN, |answer| match answer {
        Answer::Screen(screen) if screen.session == id && screen.child.is_none() => {
            Some(screen.view)
        }
        _ => None,
    });
    let _ = client.send(
        found.endpoint,
        &Request::Watch {
            session: None,
            child: None,
        },
    );
    match view?? {
        Content::Conversation(said) => {
            let start = said.len().saturating_sub(items);
            Ok(clip(
                &said[start..]
                    .iter()
                    .map(line)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ))
        }
        Content::Screen(rows) => Ok(clip(
            &rows
                .iter()
                .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

fn line(said: &Said) -> String {
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

fn diff(client: &mut Client, query: &str) -> Result<String, String> {
    let found = client.session(query)?;
    let id = found.session.id.clone();
    client.send(
        found.endpoint,
        &Request::Diff {
            session: id.clone(),
        },
    )?;
    let shown = client.answer(found.endpoint, SHOWN_WITHIN, |answer| match answer {
        Answer::Shown(shown) if shown.session == id => Some(shown.diff),
        _ => None,
    })??;
    if shown.trim().is_empty() {
        Ok("No uncommitted changes".into())
    } else {
        Ok(clip(&shown))
    }
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_string();
    }
    let mut end = text.len() - MAX_TEXT;
    while !text.is_char_boundary(end) {
        end += 1;
    }
    format!("…(earlier part left out)\n{}", &text[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(line: &str) -> Option<Value> {
        respond(line, |name, arguments| match name {
            "sessions" => Ok(format!("listed {arguments}")),
            _ => Err(format!("magi has no tool {name}")),
        })
    }

    #[test]
    fn it_speaks_the_clients_protocol_version_lists_its_tools_and_reports_errors_as_tool_results() {
        let started = answer(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert_eq!(started["result"]["protocolVersion"], "2025-03-26");
        let unknown = answer(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOLS[0]);
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
        let listed = answer(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#).unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert!(
            tools
                .iter()
                .all(|tool| tool["inputSchema"]["type"] == "object")
        );
        let read_only: Vec<&str> = tools
            .iter()
            .filter(|tool| tool["annotations"]["readOnlyHint"] == true)
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            read_only,
            ["machines", "sessions", "recent_work", "diff", "alerts"]
        );
        let called = answer(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"sessions","arguments":{"x":1}}}"#).unwrap();
        assert_eq!(called["result"]["content"][0]["text"], r#"listed {"x":1}"#);
        assert_eq!(called["result"]["isError"], false);
        let failed =
            answer(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#)
                .unwrap();
        assert_eq!(failed["result"]["isError"], true);
        let method = answer(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#).unwrap();
        assert_eq!(method["error"]["code"], -32601);
        assert_eq!(answer("{not json").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn long_output_keeps_its_end() {
        let long = format!("{}end", "x".repeat(MAX_TEXT));
        let clipped = clip(&long);
        assert!(clipped.ends_with("end"));
        assert!(clipped.starts_with("…(earlier part left out)"));
    }
}
