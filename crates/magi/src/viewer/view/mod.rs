mod agents;
mod history;
mod machines;
pub(super) mod markdown;
mod picker;

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::graphics::Graphics;
use super::model::{AgentRow, Focus, Model, Naming, Renaming, Row};
use super::style::{self, theme};
use super::typeset::Typesetter;
use super::units;
use crate::proto::{Agent, FULL, Plan, SessionState};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const WIDE: u16 = 90;
const STALE_PLAN_SECS: u64 = 15 * 60;
const SIDEBAR_SHARE: u32 = 22;
const SIDEBAR_MIN: u16 = 24;
const SIDEBAR_MAX: u16 = 30;

pub struct Areas {
    pub machines: Rect,
    pub plans: Rect,
    pub agents: Rect,
    pub rules: [Rect; 2],
    pub detail: Rect,
    pub footer: Rect,
}

pub fn areas(full: Rect, machines: usize, plans: usize, agents: usize) -> Areas {
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(full);
    let (sidebar, detail) = if body.width >= WIDE && body.height >= 12 {
        let width = (u32::from(body.width) * SIDEBAR_SHARE / 100) as u16;
        let [sidebar, detail] = Layout::horizontal([
            Constraint::Length(width.clamp(SIDEBAR_MIN, SIDEBAR_MAX)),
            Constraint::Min(0),
        ])
        .areas(body);
        (sidebar, detail)
    } else {
        let wanted = (machines + plans + agents * 2 + 6) as u16;
        let height = wanted.min(body.height * 2 / 5).max(6);
        let [sidebar, detail] =
            Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(body);
        (sidebar, detail)
    };
    let inner = sidebar.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let usage = if plans > 0 { 1 } else { 0 };
    let [machines, _, top_rule, _, agents, bottom_rule, _, plans] = Layout::vertical([
        Constraint::Length((machines as u16).min(inner.height / 3)),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(usage),
        Constraint::Length(usage),
        Constraint::Length(plans as u16),
    ])
    .areas(inner);
    Areas {
        machines,
        plans: plans.inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
        agents,
        rules: [top_rule, bottom_rule],
        detail,
        footer: footer.inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
    }
}

pub fn render(
    frame: &mut Frame,
    model: &Model,
    now: Instant,
    graphics: &mut Graphics,
    paper: &mut Option<Typesetter>,
) {
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
        plans: plan_area,
        agents: agent_area,
        rules,
        detail: detail_area,
        footer,
    } = areas(full, rows.len(), plans.len(), agents.len());
    let detail_area = detail_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    frame
        .buffer_mut()
        .set_style(detail_area, Style::new().bg(theme().panel));
    render_footer(frame, footer, model, now);
    for rule in rules.into_iter().filter(|rule| !rule.is_empty()) {
        let line = "─".repeat(rule.width as usize);
        frame.render_widget(
            Paragraph::new(Span::styled(line, style::fg(theme().rule))),
            rule,
        );
    }
    let marks = marks(model, &rows);
    machines::render_list(frame, machine_area, model, &rows, &marks, now, graphics);
    let mut cursor = Cursor::new(plan_area);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    for plan in &plans {
        let room = plan_area.width as usize;
        let line = [6, 3, 0]
            .into_iter()
            .map(|meter| plans_line(&[*plan], meter, now_secs))
            .find(|line| line.width() <= room)
            .unwrap_or_else(|| plans_line(&[*plan], 0, now_secs));
        cursor.composed(frame, graphics, line);
    }
    let listed = model.listed(now);
    agents::render_list(
        frame,
        agent_area,
        model,
        &listed,
        rows.len(),
        &marks,
        graphics,
    );

    if let Some(picker) = &model.picker {
        let recent = model.recent_projects(picker.endpoint, now);
        let home = std::env::var("HOME").unwrap_or_default();
        picker::render(frame, detail_area, picker, &recent, &home);
    } else if model.showing_alerts {
        history::render(frame, detail_area, &model.alerts);
    } else {
        match model.detail_focus(now) {
            Some(Focus::Machine(name)) => {
                if let Some(row) = rows.iter().find(|row| row.name == name) {
                    machines::render_detail(frame, detail_area, row, model.port, now, graphics);
                }
            }
            Some(Focus::Agent(id)) => {
                if let Some(agent) = agents.iter().find(|agent| agent.session.id == id) {
                    render_session(frame, detail_area, model, agent, now, graphics, paper);
                }
            }
            Some(Focus::Child(id, child_id)) => {
                let found = agents
                    .iter()
                    .find(|agent| agent.session.id == id)
                    .and_then(|agent| {
                        let child = agent
                            .session
                            .children
                            .iter()
                            .find(|child| child.id == child_id)?;
                        Some((agent, child))
                    });
                if let Some((agent, child)) = found {
                    let preview = match model.screen(&id, Some(&child_id)) {
                        None if agent.live => {
                            agents::Preview::Waiting(agents::Source::Conversation)
                        }
                        None => agents::Preview::Gone,
                        Some(screen) => match &screen.view {
                            Ok(content) => agents::Preview::Ready(content),
                            Err(reason) => agents::Preview::Failed(reason),
                        },
                    };
                    agents::render_child(
                        frame,
                        detail_area,
                        agent,
                        child,
                        preview,
                        &mut Pixels { graphics, paper },
                    );
                }
            }
            _ => {}
        }
    }
}

fn render_session(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    agent: &AgentRow<'_>,
    now: Instant,
    graphics: &mut Graphics,
    paper: &mut Option<Typesetter>,
) {
    let suggestion = model.move_suggestion(agent, now);
    let source = if agent.session.pane.is_some() && agent.session.state.is_waiting() {
        agents::Source::Pane
    } else {
        agents::Source::Conversation
    };
    let preview = match model.screen(&agent.session.id, None) {
        None if agent.live => agents::Preview::Waiting(source),
        None => agents::Preview::Gone,
        Some(screen) => match &screen.view {
            Ok(content) => agents::Preview::Ready(content),
            Err(reason) => agents::Preview::Failed(reason),
        },
    };
    let forwards = model.forwards_to(&agent.session.id, &agent.machine, now);
    agents::render_detail(
        frame,
        area,
        agents::Detail {
            agent,
            suggestion: suggestion.as_ref(),
            preview,
            draft: model.draft.as_ref(),
            forwards: &forwards,
        },
        &mut Pixels { graphics, paper },
    );
}

pub struct Pixels<'a> {
    pub graphics: &'a mut Graphics,
    pub paper: &'a mut Option<Typesetter>,
}

struct Header<'a> {
    title: Vec<Span<'a>>,
    right: Option<Vec<Span<'a>>>,
}

fn panel<'a>(title: Vec<Span<'a>>, right: Option<Vec<Span<'a>>>) -> Header<'a> {
    Header { title, right }
}

fn draw_panel(frame: &mut Frame, area: Rect, header: Header<'static>) {
    let band = Rect::new(area.x, area.y, area.width, area.height.min(1));
    let row = band.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    let line = spread(
        header.title,
        header.right.unwrap_or_default(),
        row.width as usize,
    );
    frame.render_widget(Paragraph::new(line), row);
}

pub struct Mark {
    tag: String,
    symbols: Vec<String>,
}

fn marks(model: &Model, rows: &[Row<'_>]) -> HashMap<String, Mark> {
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    rows.iter()
        .map(|row| {
            let symbols = model
                .host_icons
                .get(&row.name.to_lowercase())
                .into_iter()
                .cloned()
                .chain(default_symbols(row).iter().map(|symbol| symbol.to_string()))
                .collect();
            let tag = machine_tag(&row.name, &names).to_string();
            (row.name.clone(), Mark { tag, symbols })
        })
        .collect()
}

fn default_symbols(row: &Row<'_>) -> &'static [&'static str] {
    let mac = row
        .machine()
        .is_some_and(|machine| machine.identity.os.starts_with("macOS"));
    if mac {
        &["macbook", "laptopcomputer"]
    } else {
        &["server.rack"]
    }
}

fn machine_tag<'a>(name: &'a str, names: &[&str]) -> &'a str {
    let tag = first_word(name);
    let shared = names
        .iter()
        .filter(|other| first_word(other) == tag)
        .count()
        > 1;
    if shared { name } else { tag }
}

fn first_word(name: &str) -> &str {
    name.split(['-', '.', '_', ' ']).next().unwrap_or(name)
}

fn body(area: Rect) -> Rect {
    Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
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

    fn composed(&mut self, frame: &mut Frame, graphics: &mut Graphics, composed: Composed) {
        if self.left() == 0 {
            return;
        }
        composed.draw(frame, graphics, self.row(), Style::new());
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
    let line = if let Some(naming) = &model.naming {
        naming_line(naming, &model.project_names(now), width)
    } else if let Some(pending) = &model.pending_move {
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

fn naming_line(naming: &Naming, names: &[String], width: usize) -> Line<'static> {
    let question = match &naming.what {
        Renaming::Repo(repo) => format!("Move {repo} to project "),
        Renaming::Project { name, .. } => format!("Rename {name} to "),
    };
    let completion = naming.completion(names);
    let ghost: String = completion
        .map(|name| name.chars().skip(naming.text.chars().count()).collect())
        .unwrap_or_default();
    let mut pairs = Vec::new();
    if completion.is_some() {
        pairs.push(("⇥", "complete"));
    }
    pairs.push(("↵", "save"));
    pairs.push(("esc", "cancel"));
    let mut right = Vec::new();
    for (index, (key, label)) in pairs.into_iter().enumerate() {
        if index > 0 {
            right.push(Span::raw("   "));
        }
        right.push(Span::styled(key, style::bold(theme().text)));
        right.push(Span::styled(format!(" {label}"), style::fg(theme().faint)));
    }
    let hint = if naming.text.is_empty() && ghost.is_empty() {
        "empty puts it back where magi would"
    } else {
        ""
    };
    spread(
        vec![
            Span::styled(question, style::fg(theme().text)),
            Span::styled("› ", style::fg(theme().accent)),
            Span::styled(naming.text.clone(), style::bold(theme().text)),
            Span::styled(ghost, style::fg(theme().faint)),
            Span::styled("▏", style::fg(theme().accent)),
            Span::styled(hint, style::fg(theme().faint)),
        ],
        right,
        width,
    )
}

fn actions(model: &Model, now: Instant) -> Vec<(&'static str, &'static str)> {
    if let Some((_, _, open)) = model.selected_folder(now) {
        return vec![("↵", if open { "fold" } else { "open" }), ("P", "rename")];
    }
    if model.selection(now) == Some(Focus::NewSession) {
        return vec![("↵", "new session")];
    }
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
    if agent.live {
        actions.extend([("d", "diff"), ("v", "plan")]);
    }
    if agent.live && agent.session.agent == Agent::ClaudeCode {
        if !in_rex {
            actions.push(("r", "continue in Rex"));
        }
        actions.push(("m", "move"));
    }
    actions.push(("p", "project"));
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
