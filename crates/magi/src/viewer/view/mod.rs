mod agents;
mod bands;
pub(super) mod launcher;
mod machines;
pub(super) mod markdown;

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::graphics::Graphics;
use super::model::{Focus, Model};
use super::style::{self, theme};
use super::units;
use crate::proto::{Agent, FULL, Plan, SessionState, Waiting};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const STALE_PLAN_SECS: u64 = 15 * 60;

pub fn markdown_ansi(text: &str, width: usize) -> String {
    let (lines, links) = markdown::markdown_links(text, width);
    let mut finder = LinkFinder::new(links);
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            let url = markdown::is_link_style(span.style)
                .then(|| finder.url_for(&span.content))
                .flatten();
            if let Some(url) = &url {
                out.push_str(&format!("\x1b]8;;{url}\x1b\\"));
            }
            out.push_str(&sgr(span.style));
            out.push_str(&span.content);
            out.push_str("\x1b[0m");
            if url.is_some() {
                out.push_str("\x1b]8;;\x1b\\");
            }
        }
        out.push('\n');
    }
    out
}

struct LinkFinder {
    links: Vec<(String, String)>,
    at: usize,
}

impl LinkFinder {
    fn new(links: Vec<(String, String)>) -> Self {
        Self { links, at: 0 }
    }

    fn url_for(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let found = self.links[self.at..]
            .iter()
            .position(|(label, _)| label.contains(text))?;
        self.at += found;
        Some(self.links[self.at].1.clone())
    }
}

fn sgr(style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    for (modifier, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
    ] {
        if style.add_modifier.contains(modifier) {
            codes.push(code.into());
        }
    }
    for (color, layer) in [(style.fg, 38), (style.bg, 48)] {
        if let Some(Color::Rgb(r, g, b)) = color {
            codes.push(format!("{layer};2;{r};{g};{b}"));
        }
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
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
    bands::render(frame, model, now, graphics);
}

fn draw_panel(frame: &mut Frame, area: Rect, title: Vec<Span<'static>>, right: Vec<Span<'static>>) {
    let band = Rect::new(area.x, area.y, area.width, area.height.min(1));
    let row = band.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    let line = spread(title, right, row.width as usize);
    frame.render_widget(Paragraph::new(line), row);
}

fn body(area: Rect) -> Rect {
    Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
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

    fn rest(&self) -> Rect {
        Rect::new(self.area.x, self.y, self.area.width, self.left())
    }

    fn line(&mut self, frame: &mut Frame, line: Line<'static>) {
        if self.left() == 0 {
            return;
        }
        frame.render_widget(Paragraph::new(line), self.row());
        self.y += 1;
    }
}

fn plans_line(plans: &[&Plan], meter: usize, now_secs: u64) -> Composed {
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
            if meter > 0 {
                line.bar(
                    meter,
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
        let age = now_secs.saturating_sub(plan.as_of);
        if age >= STALE_PLAN_SECS {
            line.push(Span::styled(
                format!("  {} ago", units::duration(Duration::from_secs(age))),
                style::fg(theme().faint),
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
        let answers = keys(&[("←→", "machine"), ("y", "move"), ("n", "cancel")]);
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
    if let Some(Focus::Machine(_)) = model.selection(now) {
        let page = if model.machine_page.is_some() {
            "back"
        } else {
            "details"
        };
        return vec![("enter", page), ("c", "new session")];
    }
    let Some(agent) = model.selected_agent(now) else {
        return vec![("c", "new session")];
    };
    let mut actions = Vec::new();
    let in_rex = agent.session.pane.is_some();
    match &agent.session.state {
        SessionState::Waiting(Waiting::Permission(_)) if in_rex && agent.live => {
            actions.extend([("y", "allow"), ("n", "deny")]);
        }
        SessionState::Waiting(Waiting::Trust) if in_rex && agent.live => {
            actions.extend([("y", "trust"), ("n", "leave")]);
        }
        SessionState::Waiting(Waiting::Input(_)) if in_rex && agent.live => {
            actions.push(("1–9", "choose"));
        }
        _ => {}
    }
    if in_rex && agent.live && matches!(agent.session.state, SessionState::Working { .. }) {
        actions.push(("x", "stop"));
    }
    if in_rex {
        actions.push(("enter", "open in Rex"));
    }
    if agent.live {
        actions.push(("v", "diff"));
        if !in_rex && agent.session.agent == Agent::ClaudeCode {
            actions.push(("r", "continue in Rex"));
        }
        actions.push(("m", "move"));
    }
    actions.push(("c", "new session"));
    actions
}

pub(in crate::viewer) fn spread(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
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
