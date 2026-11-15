mod agents;
mod history;
mod machines;
mod markdown;

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
use crate::proto::{Agent, FULL, Plan, SessionState, Waiting};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const WIDE: u16 = 110;

pub fn render(frame: &mut Frame, model: &Model, now: Instant, graphics: &mut Graphics) {
    let full = frame.area();
    if full.width < 28 || full.height < 8 {
        frame.render_widget(
            Paragraph::new("magi · enlarge the pane").style(style::fg(theme().dim)),
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
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    render_footer(frame, footer, model, now);

    let (machine_area, agent_area, detail_area, wide) = if body.width >= WIDE && body.height >= 14 {
        let left_width = (u32::from(body.width) * 42 / 100).clamp(46, 70) as u16;
        let [left, right] =
            Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);
        let machines_height = (rows.len() as u16 + 2).min(left.height / 2);
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

    if model.showing_alerts {
        history::render(frame, detail_area, &model.alerts);
    } else if let Some(row) = rows.get(model.selected) {
        machines::render_detail(frame, detail_area, row, model.port, now, graphics);
    } else if let Some(item) = listed.get(model.selected - rows.len()) {
        match &item.entry {
            Entry::Session(agent) => {
                let suggestion = model.move_suggestion(agent, now);
                let source = if agent.session.pane.is_some() {
                    agents::Source::Pane
                } else {
                    agents::Source::Conversation
                };
                let preview = match model.screen(&agent.session.id) {
                    None if agent.live => agents::Preview::Waiting(source),
                    None => agents::Preview::Gone,
                    Some(screen) => match &screen.view {
                        Ok(content) => agents::Preview::Ready(content),
                        Err(reason) => agents::Preview::Failed(source, reason),
                    },
                };
                agents::render_detail(
                    frame,
                    detail_area,
                    agent,
                    suggestion.as_ref(),
                    preview,
                    graphics,
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
    let key = |text: &str| Span::styled(text.to_string(), style::fg(theme().text));
    let label = |text: &str| Span::styled(text.to_string(), style::fg(theme().faint));
    let width = area.width as usize;
    if let Some(pending) = &model.pending_move {
        let question = vec![
            Span::styled(
                format!(
                    "Move {} from {} to {}?",
                    pending.title, pending.from, pending.to
                ),
                style::bold(theme().text),
            ),
            label("   "),
            key("y"),
            label(" move   "),
            key("n"),
            label(" cancel"),
        ];
        let line = Line::from(clip(question, width)).alignment(Alignment::Center);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if let Some(draft) = &model.draft {
        let hints = vec![key("↵"), label(" send   "), key("esc"), label(" cancel")];
        let lead = format!("Message {} › ", draft.title);
        let room = width.saturating_sub(spans_width(&hints) + 3);
        let lead = truncate(&lead, room / 2);
        let typed = tail(&draft.text, room.saturating_sub(lead.width() + 1));
        let line = spread(
            vec![
                Span::styled(lead, style::fg(theme().dim)),
                Span::styled(typed, style::fg(theme().text)),
                Span::styled("▏", style::fg(theme().accent)),
            ],
            hints,
            width,
        );
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let selected = model.selected_agent(now);
    let mut keys = vec![key("↑↓"), label(" select   ")];
    if let Some(agent) = &selected
        && agent.session.pane.is_some()
        && matches!(
            agent.session.state,
            SessionState::Waiting(Waiting::Permission(_))
        )
    {
        keys.extend([key("y"), label(" allow   "), key("n"), label(" deny   ")]);
    }
    if selected
        .as_ref()
        .is_some_and(|agent| agent.live && agent.session.pane.is_some())
    {
        keys.extend([key("i"), label(" message   ")]);
    }
    if selected
        .as_ref()
        .is_some_and(|agent| agent.session.pane.is_some())
    {
        keys.extend([key("↵"), label(" open   ")]);
    }
    if selected
        .as_ref()
        .is_some_and(|agent| agent.live && agent.session.agent == Agent::ClaudeCode)
    {
        keys.extend([key("m"), label(" move   ")]);
    }
    keys.extend([key("a"), label(" alerts")]);
    let active = model.alerts.active().count();
    if active > 0 {
        keys.push(Span::styled(
            format!(" {active}"),
            style::bold(theme().warn),
        ));
    }
    keys.push(label("   "));
    keys.push(key("q"));
    keys.push(label(" quit"));
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
    let line = match message {
        Some((text, good)) => {
            let room = width.saturating_sub(spans_width(&keys) + 2);
            let color = if good { theme().accent } else { theme().warn };
            spread(
                vec![Span::styled(truncate(&text, room), style::fg(color))],
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
