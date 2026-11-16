use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::agents::{child_line, cost, state_text};
use super::{Cursor, body, clip, draw_panel, panel, spread};
use crate::proto::{ForwardState, SessionState, Waiting};
use crate::viewer::model::{AgentRow, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;

pub struct Monitor<'a> {
    pub agents: &'a [AgentRow<'a>],
    pub agent: &'a AgentRow<'a>,
    pub suggestion: Option<&'a MoveTarget>,
    pub forwards: &'a [(u16, ForwardState)],
}

pub fn render(frame: &mut Frame, area: Rect, monitor: Monitor<'_>) {
    if area.height < 5 || area.width < 16 {
        return;
    }
    draw_panel(
        frame,
        area,
        panel(
            vec![Span::styled("Sessions", style::bold(theme().text))],
            Some(vec![Span::styled(
                summary(monitor.agents),
                style::fg(theme().dim),
            )]),
        ),
    );
    let inner = body(area);
    let width = usize::from(inner.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (title, group) in [
        ("Needs you", Group::NeedsYou),
        ("Working", Group::Working),
        ("Quiet", Group::Quiet),
    ] {
        let members: Vec<&AgentRow<'_>> = monitor
            .agents
            .iter()
            .filter(|agent| group_of(agent) == group)
            .collect();
        if members.is_empty() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(Span::styled(
            title.to_string(),
            style::bold(theme().dim),
        )));
        for agent in members {
            let chosen = agent.session.id == monitor.agent.session.id;
            lines.extend(board_rows(agent, chosen, width));
        }
    }
    let detail = detail_lines(&monitor, width);
    let room = usize::from(inner.height);
    let board_room = room.saturating_sub(detail.len() + 1).max(room / 2);
    let mut cursor = Cursor::new(inner);
    for line in lines.into_iter().take(board_room) {
        cursor.line(frame, line);
    }
    if cursor.left() > 1 {
        cursor.skip();
        for line in detail {
            cursor.line(frame, line);
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Group {
    NeedsYou,
    Working,
    Quiet,
}

fn group_of(agent: &AgentRow<'_>) -> Group {
    match agent.session.state {
        SessionState::Waiting(_) if agent.live => Group::NeedsYou,
        SessionState::Working { .. } if agent.live => Group::Working,
        _ => Group::Quiet,
    }
}

fn summary(agents: &[AgentRow<'_>]) -> String {
    let count = |group| {
        agents
            .iter()
            .filter(|agent| group_of(agent) == group)
            .count()
    };
    let (needs, working) = (count(Group::NeedsYou), count(Group::Working));
    match (needs, working) {
        (0, 0) => "all quiet".into(),
        (0, working) => format!("{working} working"),
        (needs, 0) => format!("{needs} need you"),
        (needs, working) => format!("{needs} need you · {working} working"),
    }
}

fn board_rows(agent: &AgentRow<'_>, chosen: bool, width: usize) -> Vec<Line<'static>> {
    let session = agent.session;
    let (mark, color) = style::agent(session.agent);
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    let title_style = if chosen {
        style::bold(theme().text)
    } else {
        style::fg(theme().text)
    };
    let mut first = spread(
        vec![
            Span::styled(if chosen { "▎" } else { " " }, style::fg(theme().accent)),
            Span::styled(format!("{mark} "), style::fg(color)),
            Span::styled(session.title.clone(), title_style),
            Span::styled(format!("  {}", session.project), style::fg(theme().faint)),
        ],
        vec![Span::styled(
            format!("{} · {since}", agent.machine),
            style::fg(theme().dim),
        )],
        width,
    );
    if chosen {
        first = first.style(ratatui::style::Style::new().bg(theme().selected));
    }
    let second = match (&session.state, &session.activity) {
        (SessionState::Waiting(_), _) if agent.live => {
            Some((state_text(session, true).0, theme().warn))
        }
        (SessionState::Working { .. }, Some(activity)) => Some((activity.clone(), theme().dim)),
        _ => None,
    };
    let mut rows = vec![first];
    if let Some((text, color)) = second {
        rows.push(Line::from(clip(
            vec![Span::raw("    "), Span::styled(text, style::fg(color))],
            width,
        )));
    }
    rows
}

fn detail_lines(monitor: &Monitor<'_>, width: usize) -> Vec<Line<'static>> {
    let agent = monitor.agent;
    let session = agent.session;
    let mut lines = vec![Line::from(clip(
        vec![Span::styled(
            session.title.clone(),
            style::bold(theme().text),
        )],
        width,
    ))];
    lines.extend(now_lines(agent, width));
    let model = units::model(&session.model);
    let context = session.context_used.map(|used| {
        format!(
            "{} of its context",
            units::percent(f32::from(used) / f32::from(crate::proto::FULL)).trim()
        )
    });
    let using = session.usage.map(|usage| {
        let cpu = usage.cpu.map_or_else(
            || "–".to_string(),
            |millicores| format!("{}% CPU", millicores / 10),
        );
        format!("{cpu} · {}", units::bytes(usage.memory))
    });
    let ports = (!monitor.forwards.is_empty()).then(|| {
        monitor
            .forwards
            .iter()
            .map(|(port, state)| {
                format!(
                    "localhost:{port} {}",
                    match state {
                        ForwardState::Connected => "connected",
                        _ => "waiting",
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let facts: Vec<String> = [
        (!model.is_empty()).then_some(model),
        context,
        Some(cost(session)),
        using,
        ports,
    ]
    .into_iter()
    .flatten()
    .collect();
    lines.push(Line::from(clip(
        vec![Span::styled(facts.join(" · "), style::fg(theme().faint))],
        width,
    )));
    if let Some(target) = monitor.suggestion {
        lines.push(Line::from(clip(
            vec![
                Span::styled("⇢ ", style::bold(theme().warn)),
                Span::styled(
                    format!(
                        "{} is busy and {} is at {}. m moves this session there.",
                        agent.machine,
                        target.machine,
                        units::percent(target.cpu).trim()
                    ),
                    style::fg(theme().warn),
                ),
            ],
            width,
        )));
    }
    for child in &session.children {
        lines.push(child_line(child, 0, false, width));
    }
    lines
}

fn now_lines(agent: &AgentRow<'_>, width: usize) -> Vec<Line<'static>> {
    let session = agent.session;
    let (text, color) = state_text(session, agent.live);
    let mut lines = vec![Line::from(clip(
        vec![Span::styled(text, style::bold(color))],
        width,
    ))];
    let detail = match (&session.state, &session.activity) {
        (SessionState::Waiting(_), _) if agent.live => Some((
            match &session.state {
                SessionState::Waiting(Waiting::Input(_)) => "↵ answers it in its Rex tab",
                _ => "↵ approves or denies it in its Rex tab",
            }
            .to_string(),
            theme().dim,
        )),
        (SessionState::Working { .. }, Some(activity)) => Some((activity.clone(), theme().dim)),
        (SessionState::Idle, _) => Some((
            format!(
                "quiet for {}",
                units::duration(Duration::from_secs(u64::from(session.state_secs)))
            ),
            theme().faint,
        )),
        _ => None,
    };
    if let Some((text, color)) = detail {
        lines.push(Line::from(clip(
            vec![Span::styled(text, style::fg(color))],
            width,
        )));
    }
    lines
}
