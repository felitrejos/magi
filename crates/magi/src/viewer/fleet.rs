use std::io::{self, Write};
use std::time::Instant;

use serde_json::{Value, json};

use super::Config;
use super::client::Client;
use super::model::{LinkState, Row};
use crate::proto::{
    Agent, AgentSession, ChildState, FULL, PermissionMode, Plan, SessionState, Waiting,
};

pub fn print(config: Config, as_json: bool) -> io::Result<()> {
    let client = Client::connect(config.port)?;
    let now = Instant::now();
    let machines: Vec<Value> = client.model.rows(now).iter().map(describe).collect();
    let plans: Vec<Value> = client.model.plans(now).into_iter().map(plan).collect();
    let mut out = io::stdout().lock();
    if as_json {
        serde_json::to_writer(&mut out, &json!({ "machines": machines, "plans": plans }))?;
        writeln!(out)
    } else {
        table(&mut out, &machines)
    }
}

pub(super) fn describe(row: &Row<'_>) -> Value {
    let Some((machine, sample)) = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| Some((machine, machine.latest.as_ref()?)))
    else {
        let error = match row.state() {
            Some(LinkState::Lost { reason, .. } | LinkState::Rejected(reason)) => reason.clone(),
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
            "arch": machine.identity.arch,
            "tools": machine.identity.tools,
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

pub(super) fn session(session: &AgentSession) -> Value {
    let usage = session.usage;
    let asking = match &session.state {
        SessionState::Waiting(Waiting::Permission(tool)) => json!({"permission": tool}),
        SessionState::Waiting(Waiting::Input(question)) => json!({"question": question}),
        SessionState::Waiting(Waiting::Trust) => json!({"trust": true}),
        SessionState::Working { .. } | SessionState::Idle => Value::Null,
    };
    json!({
        "id": session.id,
        "agent": agent_key(session.agent),
        "title": session.title,
        "project": session.project,
        "branch": session.branch,
        "state": match session.state {
            SessionState::Working { .. } => "working",
            SessionState::Waiting(_) => "waiting",
            SessionState::Idle => "idle",
        },
        "asking": asking,
        "activity": session.activity,
        "mode": session.mode.map(PermissionMode::name),
        "queued": session.queued,
        "in_rex": session.pane.is_some(),
        "remote_url": session.remote_url(),
        "pr": session.pr.as_ref().map(|pr| json!({
            "number": pr.number,
            "url": pr.url,
            "state": format!("{:?}", pr.state).to_lowercase(),
            "checks": format!("{:?}", pr.checks).to_lowercase(),
            "review": pr.review.map(|review| format!("{review:?}").to_lowercase()),
        })),
        "subagents": session.children.iter().map(|child| json!({
            "id": child.id,
            "label": child.label,
            "running": child.state == ChildState::Running,
            "doing": child.doing,
        })).collect::<Vec<_>>(),
        "cpu": usage.and_then(|usage| usage.cpu).map(|millicores| f64::from(millicores) / 1000.0),
        "memory": usage.map(|usage| usage.memory),
    })
}

pub(super) fn plan(plan: &Plan) -> Value {
    json!({
        "agent": agent_key(plan.agent),
        "windows": plan.windows.iter().map(|window| json!({
            "label": window.label(),
            "used": round(f32::from(window.used) / f32::from(FULL)),
            "resets_at": window.resets_at,
        })).collect::<Vec<_>>(),
    })
}

pub(super) fn agent_key(agent: Agent) -> &'static str {
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
    use crate::viewer::alerts;
    use crate::viewer::client::settled;
    use crate::viewer::model::{Endpoint, Link, LinkUpdate, Model};

    #[test]
    fn a_live_machine_reports_its_load_and_agents_and_a_lost_one_its_reason() {
        let now = Instant::now();
        let mut model = Model::new(2015, now, alerts::Alerts::in_memory(now));
        model.links.insert(Endpoint::Local, Link::connecting());
        let identity = Identity {
            hostname: "mac".into(),
            tailscale_node: Some("mac".into()),
            os: "macOS 27.2".into(),
            arch: String::new(),
            tools: Vec::new(),
            cpu_model: String::new(),
            core_kinds: Vec::new(),
            reaches: Vec::new(),
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
        use crate::proto::{Usage, Window};
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
            cost_micros: 0,
            cost_complete: true,
            pane: None,
            usage: Some(Usage {
                cpu: Some(2500),
                memory: 1 << 30,
            }),
            children: Vec::new(),
            branch: None,
            parent: None,
            activity: None,
            mode: None,
            queued: None,
            remote: None,
            pr: None,
        };
        assert_eq!(
            session(&busy),
            json!({"id": "c1", "agent": "codex", "title": "run the tests", "project": "api",
                   "branch": null, "state": "working", "asking": null, "activity": null,
                   "mode": null, "queued": null, "in_rex": false, "remote_url": null, "pr": null,
                   "subagents": [],
                   "cpu": 2.5, "memory": 1u64 << 30})
        );
        let asking = AgentSession {
            state: SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
            mode: Some(PermissionMode::Plan),
            ..busy.clone()
        };
        assert_eq!(session(&asking)["asking"], json!({"permission": "Bash"}));
        let trusting = AgentSession {
            state: SessionState::Waiting(Waiting::Trust),
            ..busy.clone()
        };
        assert_eq!(session(&trusting)["asking"], json!({"trust": true}));
        assert_eq!(session(&asking)["mode"], json!("plan"));
        let remote = AgentSession {
            remote: Some("session_01abc".into()),
            ..busy.clone()
        };
        assert_eq!(
            session(&remote)["remote_url"],
            json!("https://claude.ai/code/session_01abc")
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
