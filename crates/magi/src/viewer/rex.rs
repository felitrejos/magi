use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::bounded;
use crate::proto::RexPane;

const REX_LIMIT: Duration = Duration::from_secs(5);
pub const POPUP: [f32; 4] = [0.06, 0.05, 0.88, 0.9];
pub const MODAL: [f32; 4] = [0.2, 0.15, 0.6, 0.6];

struct Here {
    session: String,
    block: String,
    server: Option<OsString>,
    bin_dir: PathBuf,
}

#[derive(Deserialize)]
struct Listing {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    block_id: String,
    owner: Option<String>,
    window_id: Option<String>,
}

pub fn available() -> bool {
    here().is_some()
}

pub fn float(label: &str, command: &[String], bounds: [f32; 4]) -> Result<(), String> {
    let here = here().ok_or("open magi in a Rex pane to see this")?;
    let listing = rex(&here, &["block", "ls", "--json", "-s", &here.session])?;
    let listing: Listing = serde_json::from_slice(&listing)
        .map_err(|error| format!("Rex listed its panes in a form magi can't read: {error}"))?;
    let window = listing
        .blocks
        .into_iter()
        .find(|block| block.block_id == here.block)
        .and_then(|block| block.window_id)
        .ok_or("Rex didn't say which window magi is in")?;
    let request = serde_json::json!({
        "session_id": here.session,
        "window_id": window,
        "bounds": bounds,
        "focus": true,
        "layout": {"block": {
            "flavor": "com.superlogical.terminal.shell",
            "label": label,
            "options": {"command": command},
        }},
    });
    rex(
        &here,
        &[
            "api",
            "call",
            "-s",
            &here.session,
            "session.new_layer",
            &request.to_string(),
        ],
    )
    .map(|_| ())
}

pub fn open(pane: &RexPane, machine: Option<&str>) -> Result<(), String> {
    let here = here().ok_or("open magi in a Rex pane to jump to agents")?;
    let listing = rex(&here, &["block", "ls", "--json", "-s", &here.session])?;
    let listing: Listing = serde_json::from_slice(&listing)
        .map_err(|error| format!("Rex listed its panes in a form magi can't read: {error}"))?;
    let owner = listing
        .blocks
        .into_iter()
        .find(|block| block.block_id == here.block)
        .and_then(|block| block.owner)
        .ok_or("no Rex app is showing this pane right now")?;
    let (server, client) = match machine {
        None => (Vec::new(), owner),
        Some(machine) => {
            let host = host_label(&here, machine)?;
            let server = vec!["-S".to_string(), host];
            let app = clients(&here, &[])?
                .into_iter()
                .find(|client| client.client_id == owner)
                .and_then(|client| client.info.label)
                .ok_or("the Rex app showing magi has no label to find it by")?;
            let there = clients(&here, &server)?
                .into_iter()
                .find(|client| {
                    client.info.kind == "app" && client.info.label.as_ref() == Some(&app)
                })
                .ok_or_else(|| format!("the Rex app isn't connected to {machine}"))?;
            (server, there.client_id)
        }
    };
    let on = |args: &[&str]| -> Result<Vec<u8>, String> {
        let mut all: Vec<&str> = server.iter().map(String::as_str).collect();
        all.extend_from_slice(args);
        rex(&here, &all)
    };
    on(&[
        "-C",
        &client,
        "do",
        "session.select",
        &format!("session_id={}", pane.session),
        &format!("window_id={}", pane.window),
    ])?;
    let focus = serde_json::json!({"session_id": pane.session, "block_id": pane.block});
    on(&["api", "call", "session.focus_block", &focus.to_string()])?;
    Ok(())
}

#[derive(Deserialize)]
struct Clients {
    clients: Vec<Client>,
}

#[derive(Deserialize)]
struct Client {
    client_id: String,
    info: ClientInfo,
}

#[derive(Deserialize)]
struct ClientInfo {
    kind: String,
    label: Option<String>,
}

fn clients(here: &Here, server: &[String]) -> Result<Vec<Client>, String> {
    let mut args: Vec<&str> = server.iter().map(String::as_str).collect();
    args.extend(["client", "ls", "--json"]);
    let listing = rex(here, &args)?;
    serde_json::from_slice::<Clients>(&listing)
        .map(|clients| clients.clients)
        .map_err(|error| format!("Rex listed its clients in a form magi can't read: {error}"))
}

fn host_label(here: &Here, machine: &str) -> Result<String, String> {
    let listing = rex(here, &["hosts", "list", "--json"])?;
    let hosts: Hosts = serde_json::from_slice(&listing)
        .map_err(|error| format!("Rex listed its hosts in a form magi can't read: {error}"))?;
    hosts
        .hosts
        .into_iter()
        .find_map(|listed| {
            let label = listed.host.label?;
            names_machine(&label, &listed.host.endpoints, machine).then_some(label)
        })
        .ok_or_else(|| {
            format!("{machine} isn't one of Rex's hosts. Add it in Rex to open its sessions")
        })
}

fn names_machine(label: &str, endpoints: &[String], machine: &str) -> bool {
    label.eq_ignore_ascii_case(machine)
        || endpoints.iter().any(|endpoint| {
            let host = endpoint
                .split_once("://")
                .map_or(endpoint.as_str(), |(_, rest)| rest)
                .split(['.', ':', '/'])
                .next()
                .unwrap_or_default();
            host.eq_ignore_ascii_case(machine)
                || host
                    .to_ascii_lowercase()
                    .strip_prefix(&machine.to_ascii_lowercase())
                    .is_some_and(|rest| rest.starts_with('-'))
        })
}

#[derive(Deserialize)]
struct Hosts {
    hosts: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    host: Host,
}

#[derive(Deserialize)]
struct Host {
    label: Option<String>,
    icon: Option<String>,
    #[serde(default)]
    endpoints: Vec<String>,
}

pub fn host_icons() -> HashMap<String, String> {
    let Some(here) = here() else {
        return HashMap::new();
    };
    let Ok(listing) = rex(&here, &["hosts", "list", "--json"]) else {
        return HashMap::new();
    };
    serde_json::from_slice::<Hosts>(&listing)
        .map(|hosts| host_icon_pairs(hosts.hosts))
        .unwrap_or_default()
}

fn host_icon_pairs(hosts: Vec<Listed>) -> HashMap<String, String> {
    hosts
        .into_iter()
        .filter_map(|listed| {
            let label = listed.host.label?.to_lowercase();
            let icon = listed.host.icon.filter(|icon| !icon.is_empty())?;
            Some((label, icon))
        })
        .collect()
}

fn here() -> Option<Here> {
    Some(Here {
        session: std::env::var("REX_SESSION")
            .ok()
            .filter(|id| !id.is_empty())?,
        block: std::env::var("REX_BLOCK")
            .ok()
            .filter(|id| !id.is_empty())?,
        server: std::env::var_os("REX_SERVER"),
        bin_dir: std::env::var_os("REX_BIN_DIR").map(PathBuf::from)?,
    })
}

fn rex(here: &Here, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(here.bin_dir.join("rex"));
    command
        .args(["--autostart=false", "--timeout", "3s"])
        .args(args);
    match &here.server {
        Some(server) => command.env("REX_SERVER", server),
        None => command.env_remove("REX_SERVER"),
    };
    let output = bounded::output(&mut command, REX_LIMIT)
        .map_err(|error| format!("can't run rex: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let reason = String::from_utf8_lossy(&output.stderr);
    let reason = reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("rex failed")
        .trim_start_matches("Error: ");
    Err(format!("Rex couldn't open it: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_is_found_among_rex_hosts_by_label_or_by_its_endpoints() {
        let endpoints = [
            "https://homelab-rex.taile4be02.ts.net".to_string(),
            "ssh://homelab".to_string(),
        ];
        assert!(names_machine("homelab", &[], "Homelab"));
        assert!(names_machine("server", &endpoints, "homelab"));
        assert!(names_machine("server", &endpoints[..1], "homelab"));
        assert!(!names_machine("server", &endpoints, "home"));
        assert!(!names_machine("nas", &[], "homelab"));
    }
}
