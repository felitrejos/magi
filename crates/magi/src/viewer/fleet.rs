use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::model::{Endpoint, LinkState, Model, Presence, Row};
use super::{Config, EVENT_BACKLOG, Event, Links, alerts, handle, net, topology};
use crate::proto::{Agent, AgentSession, FULL, Plan, SessionState};

const WAIT: Duration = Duration::from_secs(3);

pub fn print(config: Config, as_json: bool) -> io::Result<()> {
    let (sender, events) = mpsc::sync_channel(EVENT_BACKLOG);
    let mut links = Links {
        handles: HashMap::new(),
        next_generation: 0,
        port: config.port,
        socket: net::local_socket()?,
        sender: sender.clone(),
    };
    let started = Instant::now();
    let mut model = Model::new(config.port, started, alerts::Alerts::in_memory(started));
    links.reconcile(&mut model, &[Endpoint::Local])?;
    topology::spawn(sender)?;
    let deadline = started + WAIT;
    let mut found_peers = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        found_peers |= matches!(event, Event::Topology(_));
        handle(event, &mut model, &mut links)?;
        if found_peers && settled(&model, Instant::now()) {
            break;
        }
    }
    let now = Instant::now();
    let machines: Vec<Value> = model.rows(now).iter().map(describe).collect();
    let plans: Vec<Value> = model.plans(now).into_iter().map(plan).collect();
    let mut out = io::stdout().lock();
    if as_json {
        serde_json::to_writer(&mut out, &json!({ "machines": machines, "plans": plans }))?;
        writeln!(out)
    } else {
        table(&mut out, &machines)
    }
}

fn settled(model: &Model, now: Instant) -> bool {
    model.rows(now).iter().all(|row| match row.presence {
        Presence::Agent(link) => match link.state {
            LinkState::Live => row
                .machine()
                .is_some_and(|machine| machine.latest.is_some()),
            LinkState::Connecting => false,
            LinkState::Lost { .. } | LinkState::Rejected(_) => true,
        },
        Presence::Searching => false,
        Presence::NoAgent => true,
    })
}

fn describe(row: &Row<'_>) -> Value {
    let Some((machine, sample)) = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| Some((machine, machine.latest.as_ref()?)))
    else {
        let error = match row.state() {
            Some(LinkState::Lost { reason, .. }) => reason.clone(),
            Some(LinkState::Rejected(reason)) => reason.clone(),
            Some(LinkState::Connecting | LinkState::Live) => "no answer yet".into(),
            None => "no magi agent".into(),
        };
        return json!({ "name": row.name, "here": row.here, "live": false, "error": error });
    };
    let count = |wanted: fn(&SessionState) -> bool| {
        sample
            .sessions
            .iter()
            .filter(|session| wanted(&session.state))
            .count()
    };
    json!({
            "name": row.name,
            "here": row.here,
            "live": true,
            "os": machine.identity.os,
            "cores": sample.cores.len(),
            "cpu": round(f32::from(sample.cpu) / f32::from(FULL)),
            "load": sample.load.map(round),
            "memory_used": sample.memory.used,
            "memory_total": sample.memory.total,
            "agents": {
                "working": count(|state| matches!(state, SessionState::Working { .. })),
                "waiting": count(|state| matches!(state, SessionState::Waiting(_))),
                "idle": count(|state| matches!(state, SessionState::Idle)),
            },
            "sessions": sample.sessions.iter().map(session).collect::<Vec<_>>(),
            "error": null,
    })
}

fn session(session: &AgentSession) -> Value {
    let usage = session.usage;
    json!({
        "id": session.id,
        "agent": agent_key(session.agent),
        "title": session.title,
        "project": session.project,
        "state": match session.state {
            SessionState::Working { .. } => "working",
            SessionState::Waiting(_) => "waiting",
            SessionState::Idle => "idle",
        },
        "cpu": usage.and_then(|usage| usage.cpu).map(|millicores| f64::from(millicores) / 1000.0),
        "memory": usage.map(|usage| usage.memory),
    })
}

fn plan(plan: &Plan) -> Value {
    json!({
        "agent": agent_key(plan.agent),
        "windows": plan.windows.iter().map(|window| json!({
            "label": window.label(),
            "used": round(f32::from(window.used) / f32::from(FULL)),
            "resets_at": window.resets_at,
        })).collect::<Vec<_>>(),
    })
}

fn agent_key(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude",
        Agent::Codex => "codex",
    }
}

fn round(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

fn table(out: &mut impl Write, machines: &[Value]) -> io::Result<()> {
    for machine in machines {
        let name = machine["name"].as_str().unwrap_or_default();
        if machine["live"] != true {
            writeln!(
                out,
                "{name:<24} {}",
                machine["error"].as_str().unwrap_or_default()
            )?;
            continue;
        }
        let memory = machine["memory_used"].as_f64().unwrap_or(0.0)
            / machine["memory_total"].as_f64().unwrap_or(1.0).max(1.0);
        let agents = &machine["agents"];
        writeln!(
            out,
            "{name:<24} cpu {:>3.0}%  mem {:>3.0}%  {} cores  agents {} working, {} waiting, {} idle",
            machine["cpu"].as_f64().unwrap_or(0.0) * 100.0,
            memory * 100.0,
            machine["cores"],
            agents["working"],
            agents["waiting"],
            agents["idle"],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{Identity, Memory, Sample, Welcome};
    use crate::viewer::model::{Link, LinkUpdate};

    #[test]
    fn a_live_machine_reports_its_load_and_agents_and_a_lost_one_its_reason() {
        let now = Instant::now();
        let mut model = Model::new(2015, now, alerts::Alerts::in_memory(now));
        model.links.insert(Endpoint::Local, Link::connecting());
        let identity = Identity {
            hostname: "mac".into(),
            tailscale_node: Some("mac".into()),
            os: "macOS 27.2".into(),
            kernel: String::new(),
            arch: String::new(),
            cpu_model: String::new(),
            agent_version: String::new(),
        };
        model.apply(
            Endpoint::Local,
            LinkUpdate::Welcome(Box::new(Welcome {
                interval_ms: 1000,
                identity,
            })),
            now,
        );
        let sample = Sample {
            cpu: FULL / 4,
            cores: vec![0; 8],
            load: [1.234, 1.0, 0.5],
            memory: Memory {
                used: 4,
                total: 16,
                ..Memory::default()
            },
            ..Sample::default()
        };
        assert!(!settled(&model, now));
        model.apply(Endpoint::Local, LinkUpdate::Sample(Box::new(sample)), now);
        assert!(settled(&model, now));
        let machine = describe(&model.rows(now)[0]);
        assert_eq!(machine["name"], "mac");
        assert_eq!(machine["here"], true);
        assert_eq!(machine["live"], true);
        assert_eq!(machine["cpu"], 0.25);
        assert_eq!(machine["cores"], 8);
        assert_eq!(machine["load"][0], 1.23);
        assert_eq!(machine["memory_total"], 16);
        assert_eq!(machine["agents"]["idle"], 0);
        assert_eq!(machine["sessions"], json!([]));
        model.apply(
            Endpoint::Local,
            LinkUpdate::Lost("agent not running".into()),
            now,
        );
        let machine = describe(&model.rows(now)[0]);
        assert_eq!(machine["live"], false);
        assert_eq!(machine["error"], "agent not running");
    }

    #[test]
    fn sessions_carry_their_own_cpu_and_memory_and_plans_their_windows() {
        use crate::proto::{Tokens, Usage, Window};
        let busy = AgentSession {
            agent: Agent::Codex,
            id: "c1".into(),
            title: "run the tests".into(),
            project: "api".into(),
            state: SessionState::Working { tool: None },
            state_secs: 0,
            model: String::new(),
            context_tokens: 0,
            context_used: None,
            tokens: Tokens::default(),
            cost_micros: 0,
            cost_complete: true,
            age_secs: 0,
            pane: None,
            usage: Some(Usage {
                cpu: Some(2500),
                memory: 1 << 30,
            }),
            children: Vec::new(),
            branch: None,
            worktree: None,
        };
        assert_eq!(
            session(&busy),
            json!({"id": "c1", "agent": "codex", "title": "run the tests", "project": "api",
                   "state": "working", "cpu": 2.5, "memory": 1u64 << 30})
        );
        let unmeasured = AgentSession {
            usage: None,
            ..busy
        };
        assert_eq!(session(&unmeasured)["cpu"], Value::Null);
        let claude = Plan {
            agent: Agent::ClaudeCode,
            as_of: 0,
            windows: vec![Window {
                minutes: crate::proto::FIVE_HOURS,
                used: 9_000,
                resets_at: 42,
            }],
        };
        assert_eq!(
            plan(&claude),
            json!({"agent": "claude", "windows": [{"label": "5h", "used": 0.9, "resets_at": 42}]})
        );
    }
}
