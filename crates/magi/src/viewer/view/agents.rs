use std::collections::HashMap;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{
    Cursor, Mark, body, clip, draw_panel, panel, selected_style, spans_width, spread, truncate,
};
use crate::proto::{
    Agent, AgentSession, Child, ChildKind, ChildState, FULL, ForwardState, SessionState, Waiting,
};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Endpoint, Entry, Group, Listed, Model, MoveSuggestion};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const HIGH_CPU_SHARE: f32 = 0.4;
const MIN_TITLE: usize = 28;
const MAX_BADGE: usize = 7;
const ICON_WIDTH: u16 = 2;

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    listed: &[Listed<'_>],
    offset: usize,
    marks: &HashMap<String, Mark>,
    graphics: &mut Graphics,
) {
    let icons = graphics.has_pixels();
    let width = area.width as usize;
    if listed.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(" No sessions yet", style::fg(theme().dim))),
                Line::from(Span::styled(
                    " Press c to start one",
                    style::fg(theme().faint),
                )),
            ]),
            area,
        );
        return;
    }
    let selected = model.selected.checked_sub(offset);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut placed: Vec<(usize, usize, &Mark, ratatui::style::Color)> = Vec::new();
    let mut selected_end = 0;
    let mut group = None;
    for (index, item) in listed.iter().enumerate() {
        if group != Some(&item.group) {
            if group.is_some() {
                lines.push(Line::default());
            }
            if let Some(heading) = heading(&item.group) {
                lines.push(heading);
            }
            group = Some(&item.group);
        }
        let is_selected = selected == Some(index);
        match &item.entry {
            Entry::Session(agent) => {
                let mark = marks.get(&agent.machine);
                let remote = agent.endpoint != Endpoint::Local;
                let depth = match (&item.group, item.nested) {
                    (Group::Project(_), true) => 2,
                    (Group::Project(_), false) => 1,
                    _ => 0,
                };
                let (line, icon_at) =
                    session_line(agent, mark, icons, remote, depth, is_selected, width);
                if let (Some(column), Some(mark)) = (icon_at, mark) {
                    placed.push((
                        lines.len(),
                        column,
                        mark,
                        style::agent(agent.session.agent).1,
                    ));
                }
                lines.push(line);
                if let Some(worktree) = &agent.session.worktree {
                    lines.push(worktree_line(
                        worktree,
                        agent.session.branch.as_deref(),
                        depth,
                        width,
                    ));
                }
            }
            Entry::Child { child, .. } => {
                let depth = match (&item.group, item.nested) {
                    (Group::Project(_), true) => 2,
                    (Group::Project(_), false) => 1,
                    _ => 0,
                };
                lines.push(child_line(child, depth, is_selected, width));
            }
            Entry::Project {
                name, count, open, ..
            } => {
                lines.push(folder_line(name, *count, *open, is_selected, width));
            }
            Entry::Fold { count, open } => {
                lines.push(folder_line("Earlier", *count, *open, is_selected, width));
            }
            Entry::NewSession => {
                let color = if is_selected {
                    theme().text
                } else {
                    theme().faint
                };
                lines.push(
                    Line::from(Span::styled(" + New session", style::fg(color)))
                        .style(selected_style(is_selected)),
                );
            }
        }
        if is_selected {
            selected_end = lines.len();
        }
    }

    let visible = area.height as usize;
    let first = selected_end.saturating_sub(visible);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(first).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), area);
    for (line, column, mark, color) in placed {
        let Some(row) = line.checked_sub(first).filter(|row| *row < visible) else {
            continue;
        };
        graphics.icon(
            frame.buffer_mut(),
            Rect::new(area.x + column as u16, area.y + row as u16, ICON_WIDTH, 1),
            format!("session:{line}"),
            &mark.symbols,
            color,
        );
    }
}

fn heading(group: &Group) -> Option<Line<'static>> {
    let (text, color) = match group {
        Group::NeedsYou => ("Needs you".to_string(), theme().warn),
        Group::Project(_) | Group::Earlier | Group::Actions => return None,
    };
    Some(Line::from(Span::styled(
        format!(" {text}"),
        style::bold(color),
    )))
}

fn session_line(
    agent: &AgentRow<'_>,
    mark: Option<&Mark>,
    icons: bool,
    remote: bool,
    depth: usize,
    is_selected: bool,
    width: usize,
) -> (Line<'static>, Option<usize>) {
    let session = agent.session;
    let (agent_mark, color) = style::agent(session.agent);
    let indent = 1 + depth * 2;
    let with_icon = icons && mark.is_some();
    let lead = if with_icon {
        Span::raw(" ".repeat(ICON_WIDTH as usize + 1))
    } else {
        Span::styled(format!("{agent_mark} "), style::fg(color))
    };
    let mut right = Vec::new();
    if let Some(mark) = mark.filter(|_| remote && !with_icon) {
        right.push(Span::styled(
            format!("{} ", truncate(&mark.tag, MAX_BADGE)),
            style::fg(theme().faint),
        ));
    }
    right.push(row_state(session, agent.live));
    let line = spread(
        vec![
            Span::raw(" ".repeat(indent)),
            lead,
            Span::styled(
                session.title.clone(),
                if is_selected {
                    style::bold(theme().text)
                } else {
                    style::fg(theme().text)
                },
            ),
        ],
        right,
        width,
    )
    .style(selected_style(is_selected));
    (line, with_icon.then_some(indent))
}

fn short(age: &str) -> String {
    age.split(' ').next().unwrap_or(age).to_string()
}

fn row_state(session: &AgentSession, live: bool) -> Span<'static> {
    let age = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    match session.state {
        _ if !live => Span::styled("lost", style::fg(theme().faint)),
        SessionState::Working { .. } => Span::styled("●", style::fg(theme().accent)),
        SessionState::Waiting(_) => Span::styled(short(&age), style::bold(theme().warn)),
        SessionState::Idle => Span::raw(""),
    }
}

fn child_glyph(kind: ChildKind) -> &'static str {
    match kind {
        ChildKind::Subagent => "◇",
        ChildKind::Shell => "$",
        ChildKind::Task => "⇢",
    }
}

fn child_state(child: &Child) -> Span<'static> {
    let age = short(&units::duration(Duration::from_secs(u64::from(child.secs))));
    match child.state {
        ChildState::Running => Span::styled("●", style::fg(theme().accent)),
        ChildState::Done => Span::styled(format!("✓ {age}"), style::fg(theme().faint)),
        ChildState::Failed => Span::styled("failed", style::fg(theme().crit)),
        ChildState::Stopped => Span::styled("stopped", style::fg(theme().faint)),
    }
}

pub(super) fn child_line(
    child: &Child,
    depth: usize,
    is_selected: bool,
    width: usize,
) -> Line<'static> {
    let glyph = child_glyph(child.kind);
    let mut left = vec![
        Span::raw(" ".repeat(3 + depth * 2)),
        Span::styled(format!("{glyph} "), style::fg(theme().faint)),
        Span::styled(
            child.label.clone(),
            style::fg(if is_selected {
                theme().text
            } else {
                theme().dim
            }),
        ),
    ];
    if !child.detail.is_empty() {
        left.push(Span::styled(
            format!(" · {}", child.detail),
            style::fg(theme().faint),
        ));
    }
    spread(left, vec![child_state(child)], width).style(selected_style(is_selected))
}

pub fn render_child(frame: &mut Frame, area: Rect, agent: &AgentRow<'_>, child: &Child) {
    if area.height < 5 || area.width < 16 {
        return;
    }
    let kind = match child.kind {
        ChildKind::Subagent => "subagent",
        ChildKind::Shell => "background command",
        ChildKind::Task => "task on another machine",
    };
    draw_panel(
        frame,
        area,
        panel(
            vec![
                Span::styled(
                    format!("{} ", child_glyph(child.kind)),
                    style::fg(theme().dim),
                ),
                Span::styled(child.label.clone(), style::bold(theme().text)),
            ],
            Some(vec![
                Span::styled(format!("{kind} · "), style::fg(theme().dim)),
                child_state(child),
            ]),
        ),
    );
    let inner = body(area);
    let width = inner.width as usize;
    let mut from = vec![
        Span::styled("from ", style::fg(theme().faint)),
        Span::styled(agent.session.title.clone(), style::fg(theme().dim)),
    ];
    if !child.detail.is_empty() {
        from.push(Span::styled(
            format!(" · {}", child.detail),
            style::fg(theme().faint),
        ));
    }
    let mut cursor = Cursor::new(Rect::new(
        inner.x,
        inner.y - 1,
        inner.width,
        inner.height + 1,
    ));
    cursor.line(frame, Line::from(clip(from, width)));
    cursor.skip();
    let where_ = match (child.kind, child.state) {
        (ChildKind::Subagent, ChildState::Running) => {
            format!("Its work shows live in its own tab, ↳ {}", child.label)
        }
        (ChildKind::Subagent, _) => "It has finished and handed its result to the session".into(),
        (ChildKind::Shell, _) => "Its output is in the session's own pane".into(),
        (ChildKind::Task, _) => "cielago keeps its result: run cielago tasks".into(),
    };
    cursor.line(
        frame,
        Line::from(clip(
            vec![Span::styled(where_, style::fg(theme().dim))],
            width,
        )),
    );
}

fn worktree_line(
    worktree: &str,
    branch: Option<&str>,
    depth: usize,
    width: usize,
) -> Line<'static> {
    let mut left = vec![
        Span::raw(" ".repeat(3 + depth * 2)),
        Span::styled("⎇ ", style::fg(theme().faint)),
        Span::styled(worktree.to_string(), style::fg(theme().dim)),
    ];
    if let Some(branch) = branch.filter(|branch| !branch.ends_with(worktree)) {
        left.push(Span::styled(
            format!(" · {branch}"),
            style::fg(theme().faint),
        ));
    }
    Line::from(clip(left, width))
}

fn folder_line(
    name: &str,
    count: usize,
    open: bool,
    is_selected: bool,
    width: usize,
) -> Line<'static> {
    let chevron = if open { "▾" } else { "▸" };
    let right = if open {
        Vec::new()
    } else {
        vec![Span::styled(count.to_string(), style::fg(theme().faint))]
    };
    spread(
        vec![
            Span::styled(format!(" {chevron} "), style::fg(theme().faint)),
            Span::styled(name.to_string(), style::bold(theme().text)),
        ],
        right,
        width,
    )
    .style(selected_style(is_selected))
}

pub(super) fn state_span(session: &AgentSession, live: bool) -> Span<'static> {
    let (text, color) = state_text(session, live);
    match session.state {
        SessionState::Waiting(_) if live => Span::styled(
            format!(" {text} "),
            style::pill(theme().warn, theme().warn_tint),
        ),
        _ => Span::styled(text, style::fg(color)),
    }
}

pub(super) fn state_text(session: &AgentSession, live: bool) -> (String, Color) {
    if !live {
        return ("machine lost".into(), theme().faint);
    }
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    match &session.state {
        SessionState::Waiting(Waiting::Permission(Some(tool))) => {
            (format!("needs you · {tool}"), theme().warn)
        }
        SessionState::Waiting(Waiting::Permission(None)) => {
            ("needs you · approve".into(), theme().warn)
        }
        SessionState::Waiting(Waiting::Input(reason)) => {
            (format!("needs you · {reason}"), theme().warn)
        }
        SessionState::Working { tool: Some(tool) } => (format!("working · {tool}"), theme().accent),
        SessionState::Working { tool: None } => ("working".into(), theme().accent),
        SessionState::Idle => (format!("idle {since}"), theme().dim),
    }
}

fn high_cpu(agent: &AgentRow<'_>) -> bool {
    agent
        .session
        .usage
        .and_then(|usage| usage.cpu)
        .is_some_and(|millicores| {
            millicores as f32 / (agent.capacity.cores.max(1) * 1000) as f32 >= HIGH_CPU_SHARE
        })
}

pub(super) fn cost(session: &AgentSession) -> String {
    let dollars = units::dollars(session.cost_micros);
    if session.cost_complete {
        dollars
    } else {
        format!("≥{dollars}")
    }
}

pub struct Detail<'a> {
    pub agent: &'a AgentRow<'a>,
    pub suggestion: Option<&'a MoveSuggestion>,
    pub forwards: &'a [(u16, ForwardState)],
}

pub fn render_detail(frame: &mut Frame, area: Rect, detail: Detail<'_>) {
    let Detail {
        agent,
        suggestion,
        forwards,
    } = detail;
    let session = agent.session;
    let (mark, color) = style::agent(session.agent);
    if area.height < 5 || area.width < 16 {
        return;
    }
    let title_width = session.title.width().min(MIN_TITLE) + 10;
    let facts = [
        Facts::All,
        Facts::NoModel,
        Facts::Essential,
        Facts::StateOnly,
    ]
    .into_iter()
    .map(|shown| facts(agent, forwards, shown))
    .find(|facts| spans_width(facts) + title_width <= area.width as usize)
    .unwrap_or_else(|| facts(agent, forwards, Facts::StateOnly));
    let room = (area.width as usize).saturating_sub(spans_width(&facts) + 8);
    let title = clip(
        vec![
            Span::styled(format!("{mark} "), style::fg(color)),
            Span::styled(session.title.clone(), style::bold(theme().text)),
        ],
        room,
    );
    draw_panel(frame, area, panel(title, Some(facts)));
    let inner = body(area);
    let width = inner.width as usize;
    let mut cursor = Cursor::new(Rect::new(
        inner.x,
        inner.y - 1,
        inner.width,
        inner.height + 1,
    ));
    cursor.line(frame, context_line(agent, width));
    cursor.skip();
    if let Some(MoveSuggestion { pressure, target }) = suggestion {
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    Span::styled("⇢ ", style::bold(theme().warn)),
                    Span::styled(
                        format!(
                            "{}, and {} has room (CPU {}, memory {}). m moves this session there.",
                            pressure.describe(&agent.machine),
                            target.machine,
                            units::percent(target.cpu).trim(),
                            units::percent(target.memory).trim()
                        ),
                        style::fg(theme().warn),
                    ),
                ],
                width,
            )),
        );
        cursor.skip();
    }
    cursor.line(frame, status_line(agent, width));
    if let Some(queued) = &session.queued {
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    Span::styled("  queued: ", style::fg(theme().accent)),
                    Span::styled(queued.replace('\n', " "), style::fg(theme().dim)),
                ],
                width,
            )),
        );
    }
    if session.children.is_empty() {
        return;
    }
    cursor.skip();
    cursor.line(
        frame,
        Line::from(Span::styled("Subagents and tasks", style::fg(theme().dim))),
    );
    for child in &session.children {
        cursor.line(frame, child_line(child, 0, false, width));
    }
}

pub(super) fn context_line(agent: &AgentRow<'_>, width: usize) -> Line<'static> {
    let session = agent.session;
    let dot = || Span::styled(" · ", style::fg(theme().faint));
    let mut spans = vec![Span::styled(
        session.project.clone(),
        style::fg(theme().dim),
    )];
    if let Some(branch) = &session.branch {
        spans.extend([
            dot(),
            Span::styled(format!("⎇ {branch}"), style::fg(theme().dim)),
        ]);
    }
    if let Some(worktree) = &session.worktree {
        spans.extend([
            dot(),
            Span::styled(format!("worktree {worktree}"), style::fg(theme().dim)),
        ]);
    }
    spans.extend([
        dot(),
        Span::styled(agent.machine.clone(), style::fg(theme().dim)),
    ]);
    Line::from(clip(spans, width))
}

fn keys(pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (key, label) in pairs {
        spans.push(Span::styled(format!("   {key}"), style::bold(theme().text)));
        spans.push(Span::styled(format!(" {label}"), style::fg(theme().faint)));
    }
    spans
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Facts {
    All,
    NoModel,
    Essential,
    StateOnly,
}

pub(super) fn facts(
    agent: &AgentRow<'_>,
    forwards: &[(u16, ForwardState)],
    shown: Facts,
) -> Vec<Span<'static>> {
    let session = agent.session;
    if shown == Facts::StateOnly {
        return state_spans(agent);
    }
    let dot = || Span::styled(" · ", style::fg(theme().faint));
    let mut facts = Vec::new();
    let model = units::model(&session.model);
    if shown == Facts::All && !model.is_empty() {
        facts.extend([Span::styled(model, style::fg(theme().dim)), dot()]);
    }
    match session.context_used {
        _ if shown == Facts::Essential => {}
        Some(used) => {
            let fraction = f32::from(used) / f32::from(FULL);
            facts.push(Span::styled(
                format!("{} context", units::percent(fraction).trim()),
                style::fg(style::load(fraction)),
            ));
            facts.push(dot());
        }
        None if session.context_tokens > 0 => {
            facts.push(Span::styled(
                format!("{} context", units::count(session.context_tokens)),
                style::fg(theme().dim),
            ));
            facts.push(dot());
        }
        None => {}
    }
    facts.extend([Span::styled(cost(session), style::fg(theme().text)), dot()]);
    for (port, state) in forwards {
        let (note, color) = match state {
            ForwardState::Connected => ("", theme().accent),
            ForwardState::Waiting => (" waiting", theme().warn),
            ForwardState::Stopped => (" stopped", theme().faint),
        };
        facts.extend([
            Span::styled(format!("localhost:{port}{note}"), style::fg(color)),
            dot(),
        ]);
    }
    if let Some(mode) = session.mode.filter(|_| shown == Facts::All) {
        facts.extend([
            Span::styled(format!("{} mode", mode.name()), style::fg(theme().dim)),
            dot(),
        ]);
    }
    if high_cpu(agent) {
        facts.extend([Span::styled("high CPU", style::bold(theme().warn)), dot()]);
    }
    facts.extend(state_spans(agent));
    facts
}

fn status_line(agent: &AgentRow<'_>, width: usize) -> Line<'static> {
    let session = agent.session;
    let prompt = |color| Span::styled("› ", style::fg(color));
    let hint = |text: &str| Span::styled(text.to_string(), style::fg(theme().faint));
    if !agent.live {
        return Line::from(vec![
            prompt(theme().faint),
            hint("its machine isn't answering"),
        ]);
    }
    if session.pane.is_none() {
        if session.agent == Agent::ClaudeCode {
            return spread(
                vec![
                    prompt(theme().faint),
                    hint(
                        "not in Rex: continue it there to answer it from here; the original stays",
                    ),
                ],
                keys(&[("r", "continue in Rex")]),
                width,
            );
        }
        return Line::from(clip(
            vec![
                prompt(theme().faint),
                hint("approvals from magi work for sessions in Rex"),
            ],
            width,
        ));
    }
    match &session.state {
        SessionState::Waiting(Waiting::Permission(tool)) => spread(
            vec![
                prompt(theme().warn),
                Span::styled(
                    format!("Allow {}?", tool.as_deref().unwrap_or("this")),
                    style::bold(theme().warn),
                ),
            ],
            keys(&[("y", "allow"), ("n", "deny")]),
            width,
        ),
        SessionState::Waiting(Waiting::Input(_)) => spread(
            vec![
                prompt(theme().warn),
                Span::styled(
                    "Pick an option from the screen above",
                    style::bold(theme().warn),
                ),
            ],
            keys(&[("1–9", "choose"), ("o", "open")]),
            width,
        ),
        SessionState::Working { .. } => spread(
            vec![
                prompt(theme().accent),
                hint(session.activity.as_deref().unwrap_or("Working")),
            ],
            keys(&[("↵", "open")]),
            width,
        ),
        SessionState::Idle => spread(
            vec![prompt(theme().faint), hint("Idle, waiting for you")],
            keys(&[("↵", "open")]),
            width,
        ),
    }
}

fn state_spans(agent: &AgentRow<'_>) -> Vec<Span<'static>> {
    let session = agent.session;
    let mut state = vec![state_span(session, agent.live)];
    if agent.live && !matches!(session.state, SessionState::Idle) {
        let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
        let separator = if matches!(session.state, SessionState::Waiting(_)) {
            " "
        } else {
            " · "
        };
        state.push(Span::styled(
            format!("{separator}{since}"),
            style::fg(theme().dim),
        ));
    }
    state
}
