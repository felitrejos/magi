use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::client::Client;
use super::{alerts, cli, watch};
use crate::log;
use crate::proto::{OpenIn, Request};

const PROTOCOLS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const PUMP_EVERY: Duration = Duration::from_millis(100);
const ANSWER_WITHIN: Duration = Duration::from_secs(15);
const RECENT_ITEMS: usize = 20;
const MAX_RECENT_ITEMS: usize = 200;
const WAIT_SECS: u64 = 300;
const MAX_WAIT_SECS: u64 = 1800;
const MAX_TEXT: usize = 60_000;

pub fn run(port: u16) -> io::Result<()> {
    let mut client: Option<Client> = None;
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
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        if let Some(client) = client.as_mut() {
            client.wait(Duration::ZERO)?;
        }
        let Some(line) = line else {
            continue;
        };
        if let Some(reply) = respond(&line, |name, arguments| {
            let connected = match client.take() {
                Some(connected) => connected,
                None => Client::connect(port).map_err(|error| {
                    format!("magi's overview isn't reachable ({error}). It needs magi-overview agent running on your machines and Tailscale; the tools that send work elsewhere still work without it")
                })?,
            };
            call(client.insert(connected), name, arguments)
        }) {
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
            "recent_work",
            "What a session, or one of its subagents, said and did lately: messages, replies and one line per tool call, newest last",
            json!({"session": session, "subagent": {"type": "string", "description": "A subagent's id or a unique part of its label, from subagents"}, "items": {"type": "integer", "description": "How many of the latest items, 20 by default"}}),
            &["session"],
            true,
        ),
        tool(
            "subagents",
            "Every running subagent on every machine, with its session and the step it's on now",
            json!({}),
            &[],
            true,
        ),
        tool(
            "wait",
            "Wait until a session stops working (it's idle or needs the user) or one of its subagents finishes, instead of checking again and again",
            json!({"session": session, "subagent": {"type": "string", "description": "Wait for this subagent instead of the session"}, "seconds": {"type": "integer", "description": "Give up after this long, 300 by default, at most 1800"}}),
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
            "Move a Claude Code or Codex session, with its conversation and uncommitted work, to another machine once its turn ends",
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
    match name {
        "alerts" => {
            let now_secs = log::now_secs();
            let active: Vec<Value> = alerts::conditions(&client.model, Instant::now(), now_secs)
                .into_iter()
                .map(|condition| json!({"machine": condition.machine, "text": condition.text, "severity": condition.severity}))
                .collect();
            Ok(pretty(Value::Array(active)))
        }
        "recent_work" => {
            let items = arguments["items"].as_u64().map_or(RECENT_ITEMS, |items| {
                (items as usize).clamp(1, MAX_RECENT_ITEMS)
            });
            let subagent = arguments["subagent"].as_str();
            watch::recent(client, &text("session")?, subagent, items).map(|text| clip(&text))
        }
        "subagents" => Ok(pretty(Value::Array(watch::subagents(client)))),
        "wait" => {
            let seconds = arguments["seconds"]
                .as_u64()
                .map_or(WAIT_SECS, |seconds| seconds.clamp(1, MAX_WAIT_SECS));
            let subagent = arguments["subagent"].as_str();
            watch::wait(
                client,
                &text("session")?,
                subagent,
                Duration::from_secs(seconds),
            )
        }
        "send_message" => cli::send(client, &text("session")?, text("text")?),
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
            let (endpoint, request) = client.model.move_request(
                &found.session.id,
                &found.session.title,
                found.endpoint,
                &to,
                Instant::now(),
            )?;
            client.ask(endpoint, &request, ANSWER_WITHIN)
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
            cli::start(client, &machine, &request)
        }
        _ => Err(format!("magi has no tool {name}")),
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
            "alerts" => Ok(format!("listed {arguments}")),
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
        assert_eq!(read_only, ["recent_work", "subagents", "wait", "alerts"]);
        let called = answer(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"alerts","arguments":{"x":1}}}"#).unwrap();
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
