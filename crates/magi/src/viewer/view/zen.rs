use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::agents::state_text;
use super::{Composed, clip, marks, plans_line, render_footer, spread, truncate};
use crate::proto::Sample;
use crate::proto::{SessionState, Waiting};
use crate::viewer::graphics::{Chart, Graphics, Kind, Series};
use crate::viewer::model::{
    self, AgentRow, HISTORY, Level, LinkState, Machine, Model, Row, Subsystem,
};
use crate::viewer::style::{self, theme};
use crate::viewer::units;

const COLUMN_GAP: u16 = 4;
const MIN_COLUMN: u16 = 34;
const SIDE: u16 = 2;
const PLAN_METER: usize = 4;
const CHART_ROWS: u16 = 3;
const TALL_COLUMN: u16 = 18;
const CHART_GAP: u16 = 2;

pub fn render(frame: &mut Frame, model: &Model, now: Instant, graphics: &mut Graphics) {
    let full = frame.area();
    frame
        .buffer_mut()
        .set_style(full, Style::new().bg(theme().panel));
    let rows = model.rows(now);
    let agents = model.agents(now);
    let board = model.zen_board(now);
    let selected = model
        .selected_agent(now)
        .map(|agent| agent.session.id.clone());
    let inner = Rect::new(
        full.x + SIDE,
        full.y + 1,
        full.width.saturating_sub(SIDE * 2),
        full.height.saturating_sub(3),
    );
    top_line(frame, graphics, inner, model, &agents, now);
    let marks = marks(model, &rows);
    let columns = board.len().max(1) as u16;
    let per_row = (inner.width / MIN_COLUMN).clamp(1, columns);
    let width = (inner.width + COLUMN_GAP) / per_row - COLUMN_GAP;
    let grid_rows = columns.div_ceil(per_row);
    let height = inner.height.saturating_sub(2) / grid_rows.max(1);
    for (index, (name, ids)) in board.iter().enumerate() {
        let index = index as u16;
        let area = Rect::new(
            inner.x + (index % per_row) * (width + COLUMN_GAP),
            inner.y + 2 + (index / per_row) * height,
            width,
            height.saturating_sub(1),
        );
        let Some(row) = rows.iter().find(|row| row.name == *name) else {
            continue;
        };
        let tag = marks
            .get(name)
            .map_or(name.as_str(), |mark| mark.tag.as_str());
        let sessions: Vec<&AgentRow<'_>> = ids
            .iter()
            .filter_map(|id| agents.iter().find(|agent| agent.session.id == *id))
            .collect();
        column(
            frame,
            graphics,
            area,
            Column {
                row,
                index: index as usize,
                tag,
                sessions: &sessions,
                selected: selected.as_deref(),
                machine_chosen: rows
                    .get(model.selected)
                    .is_some_and(|chosen| chosen.name == *name),
            },
        );
    }
    render_footer(
        frame,
        Rect::new(
            full.x + 1,
            full.y + full.height - 1,
            full.width.saturating_sub(2),
            1,
        ),
        model,
        now,
    );
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

fn machine_block(
    frame: &mut Frame,
    graphics: &mut Graphics,
    area: Rect,
    machine: &Machine,
    sample: &Sample,
    index: usize,
) {
    let half = (area.width.saturating_sub(CHART_GAP)) / 2;
    let window = (usize::from(half) * graphics.samples_per_cell()).min(HISTORY);
    let cpu = machine.smoothed_cpu();
    let memory = machine.memory_fraction();
    let label = |title: &str, value: f32, subsystem: Subsystem, detail: String| {
        Line::from(vec![
            Span::styled(format!("{title} "), style::fg(theme().dim)),
            Span::styled(
                units::percent(value).trim().to_string(),
                style::bold(match model::grade(subsystem, value) {
                    Level::Normal => theme().text,
                    level => style::level(level),
                }),
            ),
            Span::styled(format!("  {detail}"), style::fg(theme().faint)),
        ])
    };
    frame.render_widget(
        Paragraph::new(label(
            "CPU",
            cpu,
            Subsystem::Cpu,
            format!("load {:.1}", sample.load[0]),
        )),
        Rect::new(area.x, area.y, half, 1),
    );
    frame.render_widget(
        Paragraph::new(label(
            "Memory",
            memory,
            Subsystem::Memory,
            format!(
                "{} of {}",
                units::bytes(sample.memory.used),
                units::bytes(sample.memory.total)
            ),
        )),
        Rect::new(area.x + half + CHART_GAP, area.y, half, 1),
    );
    let rows = area.height.saturating_sub(2).max(1);
    for (offset, title, values, color) in [
        (0, "cpu", machine.cpu.tail(window), theme().accent),
        (
            half + CHART_GAP,
            "memory",
            machine.memory.tail(window),
            theme().second,
        ),
    ] {
        graphics.chart(
            frame.buffer_mut(),
            Rect::new(area.x + offset, area.y + 1, half, rows),
            format!("zen:{index}:{title}"),
            &Chart {
                kind: Kind::Area,
                max: 1.0,
                guide: false,
                series: vec![Series {
                    values: &values,
                    color,
                }],
            },
        );
    }
    let mut facts = Vec::new();
    if let Some(fullest) = sample.filesystems.iter().max_by(|a, b| {
        model::fraction(a.used, a.total).total_cmp(&model::fraction(b.used, b.total))
    }) {
        facts.push(format!(
            "disk {}",
            units::percent(model::fraction(fullest.used, fullest.total)).trim()
        ));
    }
    facts.push(format!(
        "↓ {} ↑ {}",
        units::rate(sample.network.rx_per_sec),
        units::rate(sample.network.tx_per_sec)
    ));
    if let Some(degrees) = sample.temperature {
        facts.push(format!("{degrees:.0}°C"));
    }
    facts.push(format!("{} cores", sample.cores.len()));
    frame.render_widget(
        Paragraph::new(Line::from(clip(
            vec![Span::styled(facts.join(" · "), style::fg(theme().faint))],
            usize::from(area.width),
        ))),
        Rect::new(area.x, area.y + 1 + rows, area.width, 1),
    );
}

struct Column<'a> {
    row: &'a Row<'a>,
    index: usize,
    tag: &'a str,
    sessions: &'a [&'a AgentRow<'a>],
    selected: Option<&'a str>,
    machine_chosen: bool,
}

fn column(frame: &mut Frame, graphics: &mut Graphics, area: Rect, column: Column<'_>) {
    if area.height < 3 || area.width < 12 {
        return;
    }
    let width = usize::from(area.width);
    let Column {
        row,
        index,
        tag,
        sessions,
        selected,
        machine_chosen,
    } = column;
    let live = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)));
    let mut header = Composed::of(vec![Span::styled(
        tag.to_string(),
        style::bold(theme().text),
    )]);
    let right = match live {
        Some((_, sample)) => Composed::of(vec![Span::styled(
            format!(
                "up {}",
                units::duration(Duration::from_secs(sample.uptime_secs))
            ),
            style::fg(theme().faint),
        )]),
        None => Composed::of(vec![Span::styled("not answering", style::fg(theme().crit))]),
    };
    header = header.spread(right, width);
    header.draw(
        frame,
        graphics,
        Rect::new(area.x, area.y, area.width, 1),
        if machine_chosen {
            Style::new().bg(theme().selected)
        } else {
            Style::new()
        },
    );
    let mut y = area.y + 2;
    let chart_rows = if area.height >= TALL_COLUMN {
        CHART_ROWS
    } else {
        1
    };
    let block = Rect::new(area.x, y, area.width, chart_rows + 2);
    if let Some((machine, sample)) = live {
        machine_block(frame, graphics, block, machine, sample, index);
    }
    y += chart_rows + 3;
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(width), style::fg(theme().rule))),
        Rect::new(area.x, y, area.width, 1),
    );
    y += 2;
    let bottom = area.y + area.height;
    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("no sessions", style::fg(theme().faint))),
            Rect::new(area.x, y, area.width, 1),
        );
        return;
    }
    for agent in sessions {
        if y + 1 >= bottom {
            let more = Span::styled("…", style::fg(theme().faint));
            frame.render_widget(Paragraph::new(more), Rect::new(area.x, y, area.width, 1));
            return;
        }
        let chosen = selected == Some(agent.session.id.as_str());
        for (offset, line) in card(agent, chosen, width).into_iter().enumerate() {
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(area.x, y + offset as u16, area.width, 1),
            );
        }
        y += 3;
    }
}

fn card(agent: &AgentRow<'_>, chosen: bool, width: usize) -> [Line<'static>; 2] {
    let session = agent.session;
    let (dot, color) = dot(agent);
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    let (mark, agent_color) = style::agent(session.agent);
    let title_style = if chosen {
        style::bold(theme().text)
    } else {
        style::fg(theme().text)
    };
    let mut first = spread(
        vec![
            Span::styled(format!("{dot} "), style::fg(color)),
            Span::styled(format!("{mark} "), style::fg(agent_color)),
            Span::styled(session.title.clone(), title_style),
        ],
        vec![Span::styled(since, style::fg(theme().faint))],
        width,
    );
    let detail = match (&session.state, &session.activity) {
        (SessionState::Waiting(Waiting::Input(_) | Waiting::Permission(_)), _) if agent.live => {
            (state_text(session, true).0, theme().warn)
        }
        (SessionState::Working { .. }, Some(activity)) if agent.live => {
            (activity.clone(), theme().dim)
        }
        _ => {
            let mut place = session.project.clone();
            if let Some(branch) = &session.branch {
                place.push_str(&format!(" · ⎇ {branch}"));
            }
            (place, theme().faint)
        }
    };
    let mut second = Line::from(clip(
        vec![
            Span::raw("    "),
            Span::styled(
                truncate(&detail.0, width.saturating_sub(4)),
                style::fg(detail.1),
            ),
        ],
        width,
    ));
    if chosen {
        let fill = Style::new().bg(theme().selected);
        first = first.style(fill);
        second = second.style(fill);
    }
    [first, second]
}

fn dot(agent: &AgentRow<'_>) -> (&'static str, Color) {
    match agent.session.state {
        _ if !agent.live => ("○", theme().faint),
        SessionState::Waiting(_) => ("●", theme().warn),
        SessionState::Working { .. } => ("●", theme().accent),
        SessionState::Idle => ("○", theme().faint),
    }
}
