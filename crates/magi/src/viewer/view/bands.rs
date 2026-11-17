use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use super::agents::{cost, state_text};
use super::{Composed, machines, plans_line, render_footer, selected_style, truncate};
use crate::proto::{Checks, ForwardState, PrState, PullRequest, Review, SessionState};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Focus, LinkState, Model, Row};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const SIDE: u16 = 2;
const PLAN_METER: usize = 4;
const LOAD_METER: usize = 8;
const NAME_WIDTH: usize = 22;
const MIN_TITLE: usize = 24;
const MAX_TITLE: usize = 48;
const MIN_DOING: usize = 12;
const NARROW: usize = 72;
const TAG_WIDTH: usize = 22;

enum Line<'a> {
    Heading(&'static str, Color),
    Machine(&'a Row<'a>),
    Session(&'a AgentRow<'a>, bool),
    Note(String),
    Blank,
}

pub fn render(frame: &mut Frame, model: &Model, now: Instant, graphics: &mut Graphics) {
    let full = frame.area();
    frame
        .buffer_mut()
        .set_style(full, Style::new().bg(theme().panel));
    let inner = Rect::new(
        full.x + SIDE,
        full.y + 1,
        full.width.saturating_sub(SIDE * 2),
        full.height.saturating_sub(3),
    );
    let footer = Rect::new(
        full.x + 1,
        full.y + full.height - 1,
        full.width.saturating_sub(2),
        1,
    );
    let rows = model.rows(now);
    let agents = model.agents(now);
    top_line(frame, graphics, inner, model, &agents, now);
    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    if let Some(row) = model
        .machine_page
        .as_ref()
        .and_then(|name| rows.iter().find(|row| row.name == *name))
    {
        machines::render_detail(frame, body, row, model, now, graphics);
        return render_footer(frame, footer, model, now);
    }
    let selected = model.selection(now);
    let board = model.board(now);
    let lines = lines(model, &board, &rows, &agents, now);
    let at = lines
        .iter()
        .position(|line| focus(line) == selected)
        .unwrap_or(0);
    let room = usize::from(body.height);
    let first = at
        .saturating_sub(room.saturating_sub(2))
        .min(lines.len().saturating_sub(room));
    let width = usize::from(body.width);
    for (offset, line) in lines.iter().skip(first).take(room).enumerate() {
        let area = Rect::new(body.x, body.y + offset as u16, body.width, 1);
        let chosen = focus(line).is_some() && focus(line) == selected;
        let style = selected_style(chosen);
        let composed = match line {
            Line::Blank => continue,
            Line::Note(text) => Composed::of(vec![
                Span::styled("      ⇢ ", style::bold(theme().warn)),
                Span::styled(text.clone(), style::fg(theme().warn)),
            ]),
            Line::Heading(text, color) => {
                Composed::of(vec![Span::styled(*text, style::bold(*color))])
            }
            Line::Machine(row) => machine_line(row, &row.name, width),
            Line::Session(agent, asking) => {
                let tag = agent.machine.as_str();
                let forwards = model.forwards_to(&agent.session.id, &agent.machine, now);
                let parent = parent_link(agent, &agents);
                session_line(agent, (*asking).then_some(tag), &forwards, parent, width)
            }
        };
        composed.draw(frame, graphics, area, style);
    }
    render_footer(frame, footer, model, now);
}

fn focus(line: &Line<'_>) -> Option<Focus> {
    match line {
        Line::Machine(row) => Some(Focus::Machine(row.name.clone())),
        Line::Session(agent, _) => Some(Focus::Agent(agent.session.id.clone())),
        Line::Heading(..) | Line::Note(_) | Line::Blank => None,
    }
}

fn lines<'a>(
    model: &Model,
    board: &[Focus],
    rows: &'a [Row<'a>],
    agents: &'a [AgentRow<'a>],
    now: Instant,
) -> Vec<Line<'a>> {
    let agent = |id: &str| agents.iter().find(|agent| agent.session.id == id);
    let row = |name: &str| rows.iter().find(|row| row.name == name);
    let mut lines = Vec::new();
    let mut in_machines = false;
    for (index, item) in board.iter().enumerate() {
        match item {
            Focus::Agent(id) if !in_machines => {
                if index == 0 {
                    lines.push(Line::Heading("Needs you", theme().warn));
                }
                lines.extend(agent(id).map(|agent| Line::Session(agent, true)));
            }
            Focus::Agent(id) => {
                let Some(agent) = agent(id) else { continue };
                lines.push(Line::Session(agent, false));
                if let Some(suggestion) = model.move_suggestion(agent, now) {
                    lines.push(Line::Note(format!(
                        "{}, and {} has room. m moves it there.",
                        suggestion.pressure.describe(&agent.machine),
                        suggestion.target.machine
                    )));
                }
            }
            Focus::Machine(name) => {
                if index > 0 {
                    lines.push(Line::Blank);
                }
                in_machines = true;
                lines.extend(row(name).map(Line::Machine));
            }
        }
    }
    lines
}

fn machine_line(row: &Row<'_>, name: &str, width: usize) -> Composed {
    let name_width = NAME_WIDTH.min(width / 3);
    let mut left = Composed::of(vec![Span::styled(
        format!(
            "{:<name_width$}",
            truncate(name, name_width.saturating_sub(1))
        ),
        style::bold(theme().text),
    )]);
    let live = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)));
    let Some((machine, sample)) = live else {
        let why = match row.state() {
            Some(LinkState::Lost { .. }) => "not answering",
            Some(LinkState::Rejected(_)) => "refused",
            Some(LinkState::Connecting) => "connecting…",
            _ => "no magi agent",
        };
        left.push(Span::styled(why, style::fg(theme().crit)));
        return left;
    };
    let cpu = machine.smoothed_cpu();
    let memory = machine.memory_fraction();
    left.push(Span::styled("CPU ", style::fg(theme().dim)));
    left.bar(
        LOAD_METER,
        cpu,
        style::load(cpu),
        format!("band:{}:cpu", row.name),
    );
    left.push(Span::styled(
        format!(" {:>4}   ", units::percent(cpu).trim()),
        style::fg(theme().text),
    ));
    left.push(Span::styled("mem ", style::fg(theme().dim)));
    left.bar(
        LOAD_METER,
        memory,
        style::load(memory),
        format!("band:{}:memory", row.name),
    );
    left.push(Span::styled(
        format!(" {:>4}   ", units::percent(memory).trim()),
        style::fg(theme().text),
    ));
    if width < NARROW {
        return left;
    }
    let mut facts = vec![format!("load {:.1}", sample.load[0])];
    if let Some(degrees) = sample.temperature {
        facts.push(format!("{degrees:.0}°C"));
    }
    facts.push(format!(
        "up {}",
        units::duration(Duration::from_secs(sample.uptime_secs))
    ));
    left.push(Span::styled(facts.join(" · "), style::fg(theme().faint)));
    let (status, color) = match machine.pressure() {
        Some(pressure) => (format!("busy · {}", pressure.short()), theme().warn),
        None if row.here => ("this machine".to_string(), theme().faint),
        None => (String::new(), theme().faint),
    };
    left.spread(
        Composed::of(vec![Span::styled(status, style::fg(color))]),
        width,
    )
}

fn parent_link(agent: &AgentRow<'_>, agents: &[AgentRow<'_>]) -> Option<String> {
    let id = agent.session.parent.as_ref()?;
    let parent = agents.iter().find(|other| other.session.id == *id)?;
    Some(format!("← {}", parent.session.title))
}

fn session_line(
    agent: &AgentRow<'_>,
    machine: Option<&str>,
    forwards: &[(u16, ForwardState)],
    parent: Option<String>,
    width: usize,
) -> Composed {
    let title_width = (width / 3).clamp(MIN_TITLE.min(width / 2), MAX_TITLE);
    let session = agent.session;
    let (dot, dot_color) = dot(agent);
    let (mark, mark_color) = style::agent(session.agent);
    let mut left = vec![
        Span::raw("  "),
        Span::styled(format!("{dot} "), style::fg(dot_color)),
        Span::styled(format!("{mark} "), style::fg(mark_color)),
        Span::styled(
            format!(
                "{:<title_width$}",
                truncate(&session.title, title_width - 1)
            ),
            style::fg(theme().text),
        ),
    ];
    if let Some(machine) = machine {
        left.push(Span::styled(
            format!("{:<TAG_WIDTH$}", truncate(machine, TAG_WIDTH - 1)),
            style::fg(theme().dim),
        ));
    }
    let (doing, doing_color) = match &session.state {
        _ if !agent.live => ("its machine isn't answering".to_string(), theme().faint),
        SessionState::Waiting(_) => (state_text(session, true).0, theme().warn),
        SessionState::Working { .. } => (state_text(session, true).0, theme().dim),
        SessionState::Idle => (
            format!(
                "idle {}",
                units::duration(Duration::from_secs(u64::from(session.state_secs)))
            ),
            theme().faint,
        ),
    };
    left.push(Span::styled(doing, style::fg(doing_color)));
    let mut details: Vec<(u8, String)> = Vec::new();
    if session.pane.is_none() {
        details.push((5, "not in Rex".to_string()));
    }
    let model = units::model(&session.model);
    if !model.is_empty() {
        details.push((7, model));
    }
    match session.context_used {
        Some(used) => details.push((
            6,
            format!(
                "{} context",
                units::percent(f32::from(used) / f32::from(crate::proto::FULL)).trim()
            ),
        )),
        None if session.context_tokens > 0 => {
            details.push((
                6,
                format!("{} context", units::count(session.context_tokens)),
            ));
        }
        None => {}
    }
    for (port, state) in forwards {
        details.push((
            3,
            match state {
                ForwardState::Connected => format!("localhost:{port}"),
                ForwardState::Waiting => format!("localhost:{port} waiting"),
                ForwardState::Stopped => format!("localhost:{port} stopped"),
            },
        ));
    }
    if session.queued.is_some() {
        details.push((0, "queued".into()));
    }
    details.extend(parent.map(|link| (0, link)));
    if let Some(pr) = &session.pr {
        details.push((1, pull_request(pr)));
    }
    details.push((4, cost(session)));
    let room = width.saturating_sub(title_width + MIN_DOING + 6);
    let shown = |details: &[(u8, String)]| {
        details
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    };
    while shown(&details).width() > room && !details.is_empty() {
        let least = details
            .iter()
            .enumerate()
            .max_by_key(|(_, (rank, _))| *rank)
            .map(|(index, _)| index)
            .expect("details isn't empty");
        details.remove(least);
    }
    Composed::of(left).spread(
        Composed::of(vec![Span::styled(
            shown(&details),
            style::fg(theme().faint),
        )]),
        width,
    )
}

fn pull_request(pr: &PullRequest) -> String {
    let number = pr.number;
    let mut said = match pr.state {
        PrState::Merged => return format!("PR #{number} merged"),
        PrState::Closed => return format!("PR #{number} closed"),
        PrState::Draft => format!("draft PR #{number}"),
        PrState::Open => format!("PR #{number}"),
    };
    said.push_str(match pr.checks {
        Checks::Passing => " ✓",
        Checks::Failing => " checks failing",
        Checks::Running => " checks running",
        Checks::None => "",
    });
    said.push_str(match pr.review {
        Some(Review::Approved) => " · approved",
        Some(Review::ChangesRequested) => " · changes requested",
        Some(Review::Required) | None => "",
    });
    said
}

fn dot(agent: &AgentRow<'_>) -> (&'static str, Color) {
    match agent.session.state {
        _ if !agent.live => ("○", theme().faint),
        SessionState::Waiting(_) => ("●", theme().warn),
        SessionState::Working { .. } => ("●", theme().accent),
        SessionState::Idle => ("○", theme().faint),
    }
}

fn top_line(
    frame: &mut Frame,
    graphics: &mut Graphics,
    area: Rect,
    model: &Model,
    agents: &[AgentRow<'_>],
    now: Instant,
) {
    let count =
        |wanted: fn(&AgentRow<'_>) -> bool| agents.iter().filter(|agent| wanted(agent)).count();
    let needs = count(|agent| agent.live && agent.session.state.is_waiting());
    let working =
        count(|agent| agent.live && matches!(agent.session.state, SessionState::Working { .. }));
    let mut left = Composed::of(vec![Span::styled("magi", style::bold(theme().text))]);
    let summary = match (needs, working) {
        (0, 0) => ("all quiet".to_string(), theme().faint),
        (0, working) => (format!("{working} working"), theme().dim),
        (needs, 0) => (format!("{needs} need you"), theme().warn),
        (needs, working) => (
            format!("{needs} need you · {working} working"),
            theme().warn,
        ),
    };
    left.push(Span::styled(
        format!("   {}", summary.0),
        style::fg(summary.1),
    ));
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let plans = model.plans(now);
    let right = plans_line(&plans, PLAN_METER, now_secs);
    left.spread(right, usize::from(area.width)).draw(
        frame,
        graphics,
        Rect::new(area.x, area.y, area.width, 1),
        Style::new(),
    );
}
