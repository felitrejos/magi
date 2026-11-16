use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use crate::bounded;
use crate::proto::RexPane;

const REX_LIMIT: Duration = Duration::from_secs(5);

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
}

pub fn open(pane: &RexPane) -> Result<(), String> {
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
    rex(
        &here,
        &[
            "-C",
            &owner,
            "do",
            "session.select",
            &format!("session_id={}", pane.session),
            &format!("window_id={}", pane.window),
        ],
    )?;
    Ok(())
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
