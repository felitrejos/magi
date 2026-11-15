mod agents;
mod history;
mod machines;
mod markdown;
mod picker;

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::graphics::Graphics;
use super::model::{Entry, Model};
use super::style::{self, theme};
use super::units;
use crate::proto::{Agent, FULL, Plan, SessionState};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const WIDE: u16 = 110;

pub struct Areas {
    pub machines: Rect,
    pub agents: Rect,
    pub detail: Rect,
    pub footer: Rect,
    pub wide: bool,
}

pub fn areas(full: Rect, machines: usize, agents: usize) -> Areas {
    let area = full.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    if body.width >= WIDE && body.height >= 14 {
        let left_width = (u32::from(body.width) * 42 / 100).clamp(46, 70) as u16;
        let [left, right] =
            Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)])
                .spacing(1)
                .areas(body);
        let machines_height = (machines as u16 + 2).min(left.height / 2);
        let [machines, agents] =
            Layout::vertical([Constraint::Length(machines_height), Constraint::Min(0)]).areas(left);
        return Areas {
            machines,
            agents,
            detail: right,
            footer,
            wide: true,
        };
    }
    let machines_height = (machines as u16 + 2).min(body.height / 4).max(3);
    let agents_height = (agents.max(1) as u16 * 2 + 2)
        .min(body.height * 2 / 5)
        .max(3);
    let [machines, agents, detail] = Layout::vertical([
        Constraint::Length(machines_height),
        Constraint::Length(agents_height),
        Constraint::Min(0),
    ])
    .areas(body);
    Areas {
        machines,
        agents,
        detail,
        footer,
        wide: false,
    }
}

pub fn render(frame: &mut Frame, model: &Model, now: Instant, graphics: &mut Graphics) {
    let full = frame.area();
    if full.width < 28 || full.height < 8 {
        frame.render_widget(
            Paragraph::new("magi · enlarge the pane").style(style::fg(theme().dim)),
            full,
        );
        return;
    }
    let rows = model.rows(now);
    let agents = model.agents(now);
    let plans = model.plans(now);
    let Areas {
        machines: machine_area,
        agents: agent_area,
        detail: detail_area,
        footer,
        wide,
    } = areas(full, rows.len(), agents.len());
    render_footer(frame, footer, model, now);
    machines::render_list(frame, machine_area, model, &rows, wide, now, graphics);
    let listed = model.listed(now);
    let total = (
        agents.iter().map(|agent| agent.session.cost_micros).sum(),
        agents.iter().all(|agent| agent.session.cost_complete),
    );
    let limits = |room: usize| {
        [true, false]
            .into_iter()
            .map(|meters| plans_line(&plans, meters))
            .find(|line| line.width() <= room)
    };
    let limits = limits(agent_area.width.saturating_sub(4) as usize);
    let top = agents::Top {
        cost: total,
        limits,
    };
    agents::render_list(frame, agent_area, model, &listed, top, rows.len(), graphics);

    if let Some(picker) = &model.picker {
        let recent = model.recent_projects(picker.endpoint, now);
        let home = std::env::var("HOME").unwrap_or_default();
        picker::render(frame, detail_area, picker, &recent, &home);
    } else if model.showing_alerts {
        history::render(frame, detail_area, &model.alerts);
    } else if let Some(row) = rows.get(model.selected) {
        machines::render_detail(frame, detail_area, row, model.port, now, graphics);
    } else if let Some(item) = listed.get(model.selected - rows.len()) {
        match &item.entry {
            Entry::Session(agent) => {
                let suggestion = model.move_suggestion(agent, now);
                let source = if agent.session.pane.is_some() && agent.session.state.is_waiting() {
                    agents::Source::Pane
                } else {
                    agents::Source::Conversation
                };
                let preview = match model.screen(&agent.session.id) {
                    None if agent.live => agents::Preview::Waiting(source),
                    None => agents::Preview::Gone,
                    Some(screen) => match &screen.view {
                        Ok(content) => agents::Preview::Ready(content),
                        Err(reason) => agents::Preview::Failed(reason),
                    },
                };
                agents::render_detail(
                    frame,
                    detail_area,
                    agent,
                    suggestion.as_ref(),
                    preview,
                    model.draft.as_ref(),
                    &model.forwards_to(&agent.session.id, &agent.machine, now),
                );
            }
            Entry::Fold { machine, count, .. } => {
                agents::render_fold(frame, detail_area, machine, *count);
            }
        }
    }
}

fn panel<'a>(title: Vec<Span<'a>>, right: Option<Vec<Span<'a>>>) -> Block<'a> {
    let mut title = title;
    title.insert(0, Span::raw(" "));
    title.push(Span::raw(" "));
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::fg(theme().border))
        .title(Line::from(title));
    if let Some(mut right) = right {
        right.insert(0, Span::raw(" "));
        right.push(Span::raw(" "));
        block = block.title(Line::from(right).alignment(Alignment::Right));
    }
    block
}

fn draw_panel(frame: &mut Frame, area: Rect, block: Block<'_>) {
    let inside = block.inner(area);
    frame.render_widget(block, area);
    frame
        .buffer_mut()
        .set_style(inside, Style::new().bg(theme().panel));
}

fn rule(
    icon: Span<'static>,
    title: &str,
    right: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let head = vec![
        icon,
        Span::raw(" "),
        Span::styled(title.to_string(), style::bold(theme().second)),
        Span::raw("  "),
    ];
    let right_width = spans_width(&right);
    let gap = if right_width > 0 { 2 } else { 0 };
    let fill = width.saturating_sub(spans_width(&head) + right_width + gap);
    let mut spans = head;
    spans.push(Span::styled("╌".repeat(fill), style::fg(theme().rule)));
    if right_width > 0 {
        spans.push(Span::raw(" ".repeat(gap)));
        spans.extend(right);
    }
    Line::from(clip(spans, width))
}

fn icon(symbol: &str) -> Span<'static> {
    Span::styled(symbol.to_string(), style::fg(theme().second))
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

    fn spread(self, right: Composed, width: usize) -> Composed {
        self.spread_by(right, width, " ", Style::new())
    }

    fn spread_by(mut self, right: Composed, width: usize, fill: &str, style: Style) -> Composed {
        let room = width.saturating_sub(right.width() + 1);
        self.spans = clip(self.spans, room);
        self.bars.retain(|bar| bar.offset + bar.width <= room);
        let gap = width.saturating_sub(self.width() + right.width());
        self.spans.push(Span::styled(fill.repeat(gap), style));
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
            line.push(Span::styled(
                format!("{} ", window.label()),
                style::fg(theme().dim),
            ));
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
                style::fg(if fraction >= 0.85 {
                    theme().crit
                } else {
                    theme().text
                }),
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
    let area = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    let width = area.width as usize;
    let keys = |pairs: &[(&str, &str)]| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (index, (key, label)) in pairs.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(key.to_string(), style::bold(theme().text)));
            spans.push(Span::styled(format!(" {label}"), style::fg(theme().faint)));
        }
        spans
    };
    let line = if let Some(pending) = &model.pending_move {
        let question = format!(
            "Move {} from {} to {}?",
            pending.title, pending.from, pending.to
        );
        let answers = keys(&[("y", "move"), ("n", "cancel")]);
        let room = width.saturating_sub(spans_width(&answers) + 3);
        spread(
            vec![Span::styled(
                truncate(&question, room),
                style::bold(theme().text),
            )],
            answers,
            width,
        )
    } else {
        let mut global = keys(&[("a", "alerts")]);
        let active = model.alerts.active().count();
        if active > 0 {
            global.push(Span::styled(
                format!(" {active}"),
                style::bold(theme().warn),
            ));
        }
        global.push(Span::raw("   "));
        global.extend(keys(&[("q", "quit")]));
        let room = width.saturating_sub(spans_width(&global) + 3);
        let message = model
            .flashed(now)
            .map(|(text, good)| (text.to_string(), good))
            .or_else(|| {
                model
                    .topology
                    .problem
                    .clone()
                    .or_else(|| model.alerts.problem().map(str::to_string))
                    .or_else(|| model.notices(now).into_iter().next())
                    .map(|problem| (problem, false))
            });
        let left = match message {
            Some((text, good)) => {
                let color = if good { theme().accent } else { theme().warn };
                vec![Span::styled(truncate(&text, room), style::fg(color))]
            }
            None => clip(keys(&actions(model, now)), room),
        };
        spread(left, global, width)
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn actions(model: &Model, now: Instant) -> Vec<(&'static str, &'static str)> {
    let Some(agent) = model.selected_agent(now) else {
        return vec![("c", "new session")];
    };
    let mut actions = Vec::new();
    let in_rex = agent.session.pane.is_some();
    if in_rex && agent.live && matches!(agent.session.state, SessionState::Working { .. }) {
        actions.push(("x", "stop"));
    }
    if in_rex {
        actions.push(("o", "open in Rex"));
    }
    if agent.live && agent.session.agent == Agent::ClaudeCode {
        if !in_rex {
            actions.push(("r", "continue in Rex"));
        }
        actions.push(("m", "move"));
    }
    actions.push(("c", "new session"));
    actions
}

fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    Line::from(Composed::of(left).spread(Composed::of(right), width).spans)
}

fn tail(text: &str, width: usize) -> String {
    let mut used = 0;
    let start = text
        .char_indices()
        .rev()
        .find(|(_, c)| {
            used += c.width().unwrap_or(0);
            used > width
        })
        .map_or(0, |(index, c)| index + c.len_utf8());
    text[start..].to_string()
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut room = width;
    let mut clipped = Vec::new();
    for span in spans {
        let len = span.width();
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
    let len = text.width();
    if len >= width {
        truncate(text, width)
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = fit(text.graphemes(true), width.saturating_sub(1));
    if width > 0 {
        out.push('…');
    }
    out
}

fn fit<'a>(clusters: impl Iterator<Item = &'a str>, room: usize) -> String {
    let mut used = 0;
    clusters
        .take_while(|cluster| {
            used += cluster.width();
            used <= room
        })
        .collect()
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::new().bg(theme().selected)
    } else {
        Style::new()
    }
}

#[cfg(test)]
mod tests;
