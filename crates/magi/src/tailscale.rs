use std::collections::HashMap;
use std::env;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use crate::cmd;

const TIMEOUT: Duration = Duration::from_secs(5);
const MACOS_APP_CLI: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

pub struct Status {
    pub node: String,
    pub login: Option<String>,
    pub tagged: bool,
    pub ipv4: Option<Ipv4Addr>,
    pub peers: Vec<Peer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub node: String,
    pub ipv4: Ipv4Addr,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(rename = "Self")]
    me: NodeJson,
    #[serde(rename = "Peer", default)]
    peers: HashMap<String, NodeJson>,
    #[serde(rename = "User", default)]
    users: HashMap<String, UserJson>,
}

#[derive(Deserialize)]
struct NodeJson {
    #[serde(rename = "DNSName")]
    dns_name: String,
    #[serde(rename = "TailscaleIPs", default)]
    ips: Vec<IpAddr>,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "UserID")]
    user_id: u64,
    #[serde(rename = "Tags", default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct UserJson {
    #[serde(rename = "LoginName")]
    login_name: String,
}

#[derive(Deserialize)]
struct WhoisJson {
    #[serde(rename = "UserProfile")]
    profile: UserJson,
}

pub fn status() -> io::Result<Status> {
    let status: StatusJson = cmd::json(program(), &["status", "--json"], TIMEOUT)?;
    Ok(parse_status(status))
}

pub fn whois_login(peer: SocketAddr) -> io::Result<String> {
    let whois: WhoisJson = cmd::json(program(), &["whois", "--json", &peer.to_string()], TIMEOUT)?;
    Ok(whois.profile.login_name)
}

fn program() -> &'static str {
    static PROGRAM: OnceLock<&'static str> = OnceLock::new();
    PROGRAM.get_or_init(|| {
        let on_path = env::var_os("PATH")
            .is_some_and(|path| env::split_paths(&path).any(|dir| dir.join("tailscale").is_file()));
        if !on_path && cfg!(target_os = "macos") && Path::new(MACOS_APP_CLI).is_file() {
            MACOS_APP_CLI
        } else {
            "tailscale"
        }
    })
}

fn parse_status(status: StatusJson) -> Status {
    let owner = status.me.user_id;
    let mut peers: Vec<Peer> = status
        .peers
        .values()
        .filter(|peer| peer.online && peer.user_id == owner && peer.tags.is_empty())
        .filter_map(|peer| {
            Some(Peer {
                node: node_name(&peer.dns_name),
                ipv4: ipv4(&peer.ips)?,
            })
        })
        .collect();
    peers.sort_by(|a, b| a.node.cmp(&b.node));
    Status {
        node: node_name(&status.me.dns_name),
        login: status
            .users
            .get(&owner.to_string())
            .map(|user| user.login_name.clone()),
        tagged: !status.me.tags.is_empty(),
        ipv4: ipv4(&status.me.ips),
        peers,
    }
}

fn node_name(dns_name: &str) -> String {
    dns_name
        .split_once('.')
        .map_or(dns_name, |(node, _)| node)
        .to_ascii_lowercase()
}

fn ipv4(ips: &[IpAddr]) -> Option<Ipv4Addr> {
    ips.iter().find_map(|ip| match ip {
        IpAddr::V4(v4) => Some(*v4),
        IpAddr::V6(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_keeps_online_untagged_peers_of_the_same_owner() {
        let json = r#"{
            "Self": {"DNSName": "laptop.tail.ts.net.", "TailscaleIPs": ["100.1.1.1", "fd7a::1"], "Online": true, "UserID": 7},
            "Peer": {
                "a": {"DNSName": "homelab.tail.ts.net.", "TailscaleIPs": ["100.2.2.2"], "Online": true, "UserID": 7},
                "b": {"DNSName": "sleeping.tail.ts.net.", "TailscaleIPs": ["100.3.3.3"], "Online": false, "UserID": 7},
                "c": {"DNSName": "friend.tail.ts.net.", "TailscaleIPs": ["100.4.4.4"], "Online": true, "UserID": 9},
                "d": {"DNSName": "ci.tail.ts.net.", "TailscaleIPs": ["100.5.5.5"], "Online": true, "UserID": 7, "Tags": ["tag:ci"]}
            },
            "User": {"7": {"LoginName": "me@github"}, "9": {"LoginName": "friend@github"}}
        }"#;
        let status = parse_status(serde_json::from_str(json).unwrap());
        assert_eq!(status.node, "laptop");
        assert_eq!(status.login.as_deref(), Some("me@github"));
        assert!(!status.tagged);
        assert_eq!(status.ipv4, Some(Ipv4Addr::new(100, 1, 1, 1)));
        assert_eq!(
            status.peers,
            vec![Peer {
                node: "homelab".into(),
                ipv4: Ipv4Addr::new(100, 2, 2, 2)
            }]
        );
    }

    #[test]
    fn a_tagged_machine_is_reported_as_tagged() {
        let json = r#"{"Self": {"DNSName": "server.tail.ts.net.", "TailscaleIPs": ["100.9.9.9"], "UserID": 1, "Tags": ["tag:server"]},
                       "User": {"1": {"LoginName": "tagged-devices"}}}"#;
        assert!(parse_status(serde_json::from_str(json).unwrap()).tagged);
    }

    #[test]
    fn node_names_are_the_first_dns_label() {
        assert_eq!(node_name("Homelab.tail.ts.net."), "homelab");
        assert_eq!(node_name("bare"), "bare");
    }
}
