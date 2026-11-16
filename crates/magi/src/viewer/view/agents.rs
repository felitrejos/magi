use std::collections::HashMap;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use super::{
    Cursor, Mark, body, clip, draw_panel, markdown, panel, selected_style, spans_width, spread,
    tail, truncate,
};
use crate::proto::{
    Agent, AgentSession, BOLD, Child, ChildKind, ChildState, Content, DIM, FULL, ForwardState,
    INVERSE, ITALIC, Run, SessionState, Shade, UNDERLINE, Waiting,
};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Draft, Endpoint, Entry, Group, Listed, Model, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const HIGH_CPU_SHARE: f32 = 0.4;
const MIN_TITLE: usize = 28;
const MAX_BADGE: usize = 10;
const ICON_WIDTH: u16 = 2;
const DRAFT_ROWS: usize = 6;
const TAB: &str = "    ";

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
    let mut placed: Vec<(usize, usize, &Mark)> = Vec::new();
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
                    placed.push((lines.len(), column, mark));
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
    for (line, column, mark) in placed {
        let Some(row) = line.checked_sub(first).filter(|row| *row < visible) else {
            continue;
        };
        graphics.icon(
            frame.buffer_mut(),
            Rect::new(area.x + column as u16, area.y + row as u16, ICON_WIDTH, 1),
            format!("session:{line}"),
            &mark.symbols,
            theme().dim,
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
    let mut right = Vec::new();
    match mark {
        Some(_) if icons => right.push(Span::raw(" ".repeat(ICON_WIDTH as usize + 1))),
        Some(mark) if remote => right.push(Span::styled(
            format!("{} ", truncate(&mark.tag, MAX_BADGE)),
            style::fg(theme().faint),
        )),
        _ => {}
    }
    right.push(row_state(session, agent.live));
    let icon_at = (mark.is_some() && icons).then(|| width.saturating_sub(spans_width(&right)));
    let line = spread(
        vec![
            Span::raw(" ".repeat(1 + depth * 2)),
            Span::styled(format!("{agent_mark} "), style::fg(color)),
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
    (line, icon_at)
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
        SessionState::Idle => Span::styled(short(&age), style::fg(theme().faint)),
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

fn child_line(child: &Child, depth: usize, is_selected: bool, width: usize) -> Line<'static> {
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

pub fn render_child(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentRow<'_>,
    child: &Child,
    preview: Preview<'_>,
) {
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
    let context = Rect::new(inner.x, inner.y - 1, inner.width, 1);
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
    frame.render_widget(Paragraph::new(Line::from(clip(from, width))), context);
    let mut cursor = Cursor::new(inner);
    live_view(&mut cursor, frame, preview, width);
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

fn state_span(session: &AgentSession, live: bool) -> Span<'static> {
    let (text, color) = state_text(session, live);
    match session.state {
        SessionState::Waiting(_) if live => Span::styled(
            format!(" {text} "),
            style::pill(theme().warn, theme().warn_tint),
        ),
        _ => Span::styled(text, style::fg(color)),
    }
}

fn state_text(session: &AgentSession, live: bool) -> (String, Color) {
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

fn cost(session: &AgentSession) -> String {
    let dollars = units::dollars(session.cost_micros);
    if session.cost_complete {
        dollars
    } else {
        format!("≥{dollars}")
    }
}

pub fn render_detail(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentRow<'_>,
    suggestion: Option<&MoveTarget>,
    preview: Preview<'_>,
    draft: Option<&Draft>,
    forwards: &[(u16, ForwardState)],
) {
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
    let context = Rect::new(inner.x, inner.y - 1, inner.width, 1);
    frame.render_widget(Paragraph::new(context_line(agent, width)), context);
    let draft = draft.filter(|draft| draft.session == session.id);
    let shown = draft.map(|draft| {
        let lines: Vec<String> = draft
            .text
            .split('\n')
            .map(|line| line.replace('\t', TAB))
            .collect();
        let hidden = lines.len().saturating_sub(DRAFT_ROWS);
        (lines, hidden)
    });
    let rows = shown
        .as_ref()
        .map_or(1, |(lines, hidden)| lines.len() - hidden);
    let [view, input] = Layout::vertical([Constraint::Min(0), Constraint::Length(2 + rows as u16)])
        .spacing(1)
        .areas(inner);
    let mut cursor = Cursor::new(view);
    if let Some(target) = suggestion {
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    Span::styled("⇢ ", style::bold(theme().warn)),
                    Span::styled(
                        format!(
                            "{} is busy and {} is at {}. Press m to move this session there.",
                            agent.machine,
                            target.machine,
                            units::percent(target.cpu).trim()
                        ),
                        style::fg(theme().warn),
                    ),
                ],
                width,
            )),
        );
        cursor.skip();
    }
    live_view(&mut cursor, frame, preview, width);
    let mut frame_box = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::fg(if shown.is_some() {
            theme().accent
        } else {
            theme().border
        }));
    if let Some((_, hidden)) = shown.as_ref().filter(|(_, hidden)| *hidden > 0) {
        frame_box = frame_box.title(
            Line::from(Span::styled(
                format!(" {hidden} more above "),
                style::fg(theme().faint),
            ))
            .alignment(Alignment::Right),
        );
    }
    let field = frame_box.inner(input).inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    frame.render_widget(frame_box, input);
    let width = field.width as usize;
    let mut cursor = Cursor::new(field);
    let Some((lines, hidden)) = shown else {
        return cursor.line(frame, input_line(agent, width));
    };
    let multi_line = lines.len() > 1;
    let last = lines.len() - 1;
    for (index, line) in lines.into_iter().enumerate().skip(hidden) {
        cursor.line(
            frame,
            draft_line(index, line, index == last, multi_line, width),
        );
    }
}

fn context_line(agent: &AgentRow<'_>, width: usize) -> Line<'static> {
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

fn draft_line(
    index: usize,
    text: String,
    last: bool,
    multi_line: bool,
    width: usize,
) -> Line<'static> {
    let prompt = if index == 0 { "› " } else { "  " };
    let prompt = Span::styled(prompt, style::fg(theme().accent));
    let text_style = style::fg(theme().text);
    if !last {
        return Line::from(clip(vec![prompt, Span::styled(text, text_style)], width));
    }
    let mut pairs = vec![("↵", "send")];
    if !multi_line {
        pairs.push(("⇧↵", "new line"));
    }
    pairs.push(("esc", "cancel"));
    let right = keys(&pairs);
    let room = width.saturating_sub(spans_width(&right) + 4);
    spread(
        vec![
            prompt,
            Span::styled(tail(&text, room), text_style),
            Span::styled("▏", style::fg(theme().accent)),
        ],
        right,
        width,
    )
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
enum Facts {
    All,
    NoModel,
    Essential,
    StateOnly,
}

fn facts(
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
    if high_cpu(agent) {
        facts.extend([Span::styled("high CPU", style::bold(theme().warn)), dot()]);
    }
    facts.extend(state_spans(agent));
    facts
}

fn input_line(agent: &AgentRow<'_>, width: usize) -> Line<'static> {
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
                    hint("continue it in Rex to message it here; the original stays"),
                ],
                keys(&[("r", "continue in Rex")]),
                width,
            );
        }
        return Line::from(clip(
            vec![
                prompt(theme().faint),
                hint("messages and approvals work for sessions in Rex"),
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
        SessionState::Working { .. } => Line::from(vec![
            prompt(theme().faint),
            hint("working: you can message it once it's idle"),
        ]),
        SessionState::Idle => spread(
            vec![prompt(theme().faint), hint("Message this session")],
            keys(&[("↵", "write")]),
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

#[derive(Clone, Copy)]
pub enum Source {
    Pane,
    Conversation,
}

pub enum Preview<'a> {
    Waiting(Source),
    Ready(&'a Content),
    Failed(&'a str),
    Gone,
}

fn live_view(cursor: &mut Cursor, frame: &mut Frame, preview: Preview<'_>, width: usize) {
    let note = |cursor: &mut Cursor, frame: &mut Frame, text: &str, color| {
        cursor.line(
            frame,
            Line::from(clip(
                vec![Span::styled(text.to_string(), style::fg(color))],
                width,
            )),
        );
    };
    let lines: Vec<Line<'static>> = match preview {
        Preview::Ready(Content::Screen(rows)) => rows
            .iter()
            .map(|row| Line::from(clip(row.iter().map(run_span).collect(), width)))
            .collect(),
        Preview::Ready(Content::Conversation(said)) => {
            markdown::conversation(said, width, cursor.left() as usize)
        }
        Preview::Waiting(Source::Pane) => {
            return note(cursor, frame, "connecting to the pane…", theme().faint);
        }
        Preview::Waiting(Source::Conversation) => {
            return note(cursor, frame, "reading the conversation…", theme().faint);
        }
        Preview::Failed(reason) => return note(cursor, frame, reason, theme().warn),
        Preview::Gone => {
            return note(cursor, frame, "its machine isn't answering", theme().faint);
        }
    };
    let room = cursor.left() as usize;
    if matches!(preview, Preview::Ready(Content::Conversation(_))) {
        for _ in lines.len()..room {
            cursor.skip();
        }
    }
    for line in lines
        .into_iter()
        .rev()
        .take(room)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        cursor.line(frame, line);
    }
}

fn run_span(run: &Run) -> Span<'static> {
    let color = |shade: Shade| match shade {
        Shade::Default => None,
        Shade::Indexed(index) => Some(Color::Indexed(index)),
        Shade::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    };
    let modifiers = [
        (BOLD, Modifier::BOLD),
        (DIM, Modifier::DIM),
        (ITALIC, Modifier::ITALIC),
        (UNDERLINE, Modifier::UNDERLINED),
        (INVERSE, Modifier::REVERSED),
    ]
    .into_iter()
    .filter(|(flag, _)| run.flags & flag != 0)
    .fold(Modifier::empty(), |all, (_, modifier)| all | modifier);
    Span::styled(
        run.text.clone(),
        Style {
            fg: color(run.fg),
            bg: color(run.bg),
            ..Style::new()
        }
        .add_modifier(modifiers),
    )
}
