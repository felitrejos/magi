mod agents;
mod history;
mod machines;

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::graphics::{Chart, Graphics};
use super::model::{AgentRow, Level, LinkState, Model, Row, Strain};
use super::style::{self, ACCENT, BORDER, CRIT, DIM, FAINT, GOOD, TEXT, WARN, WARN_TINT};
use super::units;
use crate::log;
use crate::proto::{Agent, FULL, Plan, SessionState};

const WIDE: u16 = 110;

pub fn render(frame: &mut Frame, model: &Model, now: Instant, graphics: &mut Graphics) {
    let full = frame.area();
    if full.width < 28 || full.height < 8 {
        frame.render_widget(
            Paragraph::new("magi · enlarge the pane").style(style::fg(DIM)),
            full,
        );
        return;
    }
    let area = full.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let rows = model.rows(now);
    let agents = model.agents(now);
    let plans = model.plans(now);
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(frame, header, &rows, &agents, &plans, graphics);
    render_footer(frame, footer, model, now);

    let (machine_area, agent_area, detail_area, wide) = if body.width >= WIDE && body.height >= 14 {
        let left_width = (u32::from(body.width) * 42 / 100).clamp(46, 70) as u16;
        let [left, right] =
            Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);
        let machines_height = (rows.len() as u16 * 2 + 2).min(left.height / 2);
        let [machines, agents_area] =
            Layout::vertical([Constraint::Length(machines_height), Constraint::Min(0)]).areas(left);
        (machines, agents_area, right, true)
    } else {
        let machines_height = (rows.len() as u16 + 2).min(body.height / 4).max(3);
        let agents_height = (agents.len().max(1) as u16 * 2 + 2)
            .min(body.height * 2 / 5)
            .max(3);
        let [machines, agents_area, detail] = Layout::vertical([
            Constraint::Length(machines_height),
            Constraint::Length(agents_height),
            Constraint::Min(0),
        ])
        .areas(body);
        (machines, agents_area, detail, false)
    };
    machines::render_list(frame, machine_area, model, &rows, wide, now, graphics);
    agents::render_list(frame, agent_area, model, &agents, rows.len());

    if model.showing_alerts {
        history::render(frame, detail_area, &model.alerts);
    } else if let Some(row) = rows.get(model.selected) {
        machines::render_detail(frame, detail_area, row, model.port, now, graphics);
    } else if let Some(agent) = agents.get(model.selected - rows.len()) {
        agents::render_detail(frame, detail_area, agent, &plans, graphics);
    }
}

fn panel<'a>(title: Vec<Span<'a>>, right: Option<Vec<Span<'a>>>) -> Block<'a> {
    let mut title = title;
    title.insert(0, Span::raw(" "));
    title.push(Span::raw(" "));
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::fg(BORDER))
        .title(Line::from(title));
    if let Some(mut right) = right {
        right.insert(0, Span::raw(" "));
        right.push(Span::raw(" "));
        block = block.title(Line::from(right).alignment(Alignment::Right));
    }
    block
}

struct Bar {
    offset: usize,
    width: usize,
    fraction: f32,
    color: Color,
    slot: String,
}

#[derive(Default)]
struct Composed {
    spans: Vec<Span<'static>>,
    bars: Vec<Bar>,
}

impl Composed {
    fn of(spans: Vec<Span<'static>>) -> Self {
        Self {
            spans,
            bars: Vec::new(),
        }
    }

    fn push(&mut self, span: Span<'static>) {
        self.spans.push(span);
    }

    fn bar(&mut self, width: usize, fraction: f32, color: Color, slot: String) {
        self.bars.push(Bar {
            offset: self.width(),
            width,
            fraction,
            color,
            slot,
        });
        self.spans.push(Span::raw(" ".repeat(width)));
    }

    fn width(&self) -> usize {
        spans_width(&self.spans)
    }

    fn spread(mut self, right: Composed, width: usize) -> Composed {
        let room = width.saturating_sub(right.width() + 1);
        self.spans = clip(self.spans, room);
        self.bars.retain(|bar| bar.offset + bar.width <= room);
        let gap = width.saturating_sub(self.width() + right.width());
        self.spans.push(Span::raw(" ".repeat(gap)));
        let shift = self.width();
        self.spans.extend(right.spans);
        self.bars.extend(right.bars.into_iter().map(|bar| Bar {
            offset: bar.offset + shift,
            ..bar
        }));
        self
    }

    fn draw(self, frame: &mut Frame, graphics: &mut Graphics, area: Rect, style: Style) {
        let width = area.width as usize;
        let line = Line::from(clip(self.spans, width)).style(style);
        frame.render_widget(Paragraph::new(line), area);
        for bar in self.bars {
            if bar.width == 0 || bar.offset + bar.width > width {
                continue;
            }
            graphics.meter(
                frame.buffer_mut(),
                Rect::new(area.x + bar.offset as u16, area.y, bar.width as u16, 1),
                bar.slot,
                bar.fraction,
                bar.color,
            );
        }
    }
}

struct Cursor {
    area: Rect,
    y: u16,
}

impl Cursor {
    fn new(area: Rect) -> Self {
        Self { area, y: area.y }
    }

    fn left(&self) -> u16 {
        (self.area.y + self.area.height).saturating_sub(self.y)
    }

    fn skip(&mut self) {
        self.y += 1;
    }

    fn row(&self) -> Rect {
        Rect::new(self.area.x, self.y, self.area.width, 1)
    }

    fn line(&mut self, frame: &mut Frame, line: Line<'static>) {
        if self.left() == 0 {
            return;
        }
        frame.render_widget(Paragraph::new(line), self.row());
        self.y += 1;
    }

    fn composed(&mut self, frame: &mut Frame, graphics: &mut Graphics, composed: Composed) {
        if self.left() == 0 {
            return;
        }
        composed.draw(frame, graphics, self.row(), Style::new());
        self.y += 1;
    }

    fn chart(
        &mut self,
        frame: &mut Frame,
        graphics: &mut Graphics,
        rows: u16,
        slot: &str,
        chart: &Chart,
    ) {
        let rows = rows.min(self.left());
        if rows == 0 {
            return;
        }
        graphics.chart(
            frame.buffer_mut(),
            Rect::new(self.area.x, self.y, self.area.width, rows),
            slot.to_string(),
            chart,
        );
        self.y += rows;
    }
}

fn render_header(
    frame: &mut Frame,
    area: Rect,
    rows: &[Row<'_>],
    agents: &[AgentRow<'_>],
    plans: &[&Plan],
    graphics: &mut Graphics,
) {
    let width = area.width as usize;
    let mut left = vec![
        Span::styled("magi", style::bold(ACCENT)),
        Span::styled(
            format!(
                "  {} {} · {} {}",
                rows.len(),
                plural(rows.len(), "machine", "machines"),
                agents.len(),
                plural(agents.len(), "agent", "agents")
            ),
            style::fg(DIM),
        ),
    ];
    let waiting = agents
        .iter()
        .filter(|agent| agent.live && matches!(agent.session.state, SessionState::Waiting(_)))
        .count();
    if waiting > 0 {
        left.push(Span::raw("  "));
        left.push(Span::styled(
            format!(" {waiting} {} you ", plural(waiting, "needs", "need")),
            style::pill(WARN, WARN_TINT),
        ));
    }
    let lost = rows
        .iter()
        .filter(|row| {
            matches!(
                row.state(),
                Some(LinkState::Lost { .. } | LinkState::Rejected(_))
            )
        })
        .count();
    if lost > 0 {
        left.push(Span::styled(format!("  {lost} lost"), style::bold(CRIT)));
    }
    left.extend(health(&Model::strains(rows), rows));

    let clock = log::clock();
    let clock = Span::styled(format!("  {}", &clock[..5]), style::fg(DIM));
    let room = width.saturating_sub(spans_width(&left) + 6 + 2);
    let mut right = plans_line(plans, true);
    if right.width() > room {
        right = plans_line(plans, false);
    }
    if right.width() > room {
        right = Composed::default();
    }
    right.push(clock);
    Composed::of(left)
        .spread(right, width)
        .draw(frame, graphics, area, Style::new());
}

fn health(strains: &[Strain; 3], rows: &[Row<'_>]) -> Vec<Span<'static>> {
    if !rows.iter().any(|row| row.state() == Some(&LinkState::Live)) {
        return Vec::new();
    }
    let worst = strains
        .iter()
        .filter(|strain| strain.level != Level::Normal)
        .max_by(|a, b| {
            a.level
                .partial_cmp(&b.level)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.worst.total_cmp(&b.worst))
        });
    match worst.and_then(|strain| strain.culprit.as_deref().map(|culprit| (strain, culprit))) {
        Some((strain, culprit)) => vec![Span::styled(
            format!(
                "  {culprit} {} {}",
                strain.subsystem.name(),
                if strain.level == Level::Critical {
                    "critical"
                } else {
                    "high"
                }
            ),
            style::fg(style::level(strain.level)),
        )],
        None => vec![Span::styled("  all normal", style::fg(GOOD))],
    }
}

fn plans_line(plans: &[&Plan], meters: bool) -> Composed {
    let mut line = Composed::default();
    for plan in plans {
        if line.width() > 0 {
            line.push(Span::raw("   "));
        }
        let (_, color) = style::agent(plan.agent);
        line.push(Span::styled(
            format!("{} ", short_name(plan.agent)),
            style::fg(color),
        ));
        for (index, window) in plan.windows.iter().enumerate() {
            let fraction = f32::from(window.used) / f32::from(FULL);
            if index > 0 {
                line.push(Span::raw("  "));
            }
            line.push(Span::styled(format!("{} ", window.label()), style::fg(DIM)));
            if meters {
                line.bar(
                    6,
                    fraction,
                    style::load(fraction),
                    format!("plan:{:?}:{}", plan.agent, window.minutes),
                );
                line.push(Span::raw(" "));
            }
            line.push(Span::styled(
                units::percent(fraction).trim().to_string(),
                style::fg(if fraction >= 0.85 { CRIT } else { TEXT }),
            ));
        }
    }
    line
}

fn short_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude",
        Agent::Codex => "Codex",
    }
}

fn render_footer(frame: &mut Frame, area: Rect, model: &Model, now: Instant) {
    let active = model.alerts.active().count();
    let key = |text: &str| Span::styled(text.to_string(), style::fg(TEXT));
    let label = |text: &str| Span::styled(text.to_string(), style::fg(FAINT));
    let mut keys = vec![key("↑↓"), label(" select   "), key("a"), label(" alerts")];
    if active > 0 {
        keys.push(Span::styled(format!(" {active}"), style::bold(WARN)));
    }
    keys.push(label("   "));
    keys.push(key("q"));
    keys.push(label(" quit"));
    let problem = model
        .topology
        .problem
        .clone()
        .or_else(|| model.alerts.problem().map(str::to_string))
        .or_else(|| model.notices(now).into_iter().next());
    let width = area.width as usize;
    let line = match problem {
        Some(problem) => {
            let room = width.saturating_sub(spans_width(&keys) + 2);
            spread(
                vec![Span::styled(truncate(&problem, room), style::fg(WARN))],
                keys,
                width,
            )
        }
        None => Line::from(keys).alignment(Alignment::Center),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    Line::from(Composed::of(left).spread(Composed::of(right), width).spans)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.chars().count()).sum()
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut room = width;
    let mut clipped = Vec::new();
    for span in spans {
        let len = span.content.chars().count();
        if len <= room {
            room -= len;
            clipped.push(span);
            continue;
        }
        if room > 0 {
            clipped.push(Span::styled(truncate(&span.content, room), span.style));
        }
        break;
    }
    clipped
}

fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        truncate(text, width)
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    if width > 0 {
        out.push('…');
    }
    out
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::new().bg(style::SELECTED)
    } else {
        Style::new()
    }
}

#[cfg(test)]
mod tests;
