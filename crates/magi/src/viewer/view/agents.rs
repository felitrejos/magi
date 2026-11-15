use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{
    Composed, Cursor, clip, draw_panel, markdown, panel, selected_style, spans_width, spread, tail,
};
use crate::proto::{
    AgentSession, BOLD, Child, Content, DIM, FULL, ForwardState, INVERSE, ITALIC, Run,
    SessionState, Shade, UNDERLINE, Waiting,
};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Draft, Entry, Group, Listed, Model, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const HIGH_CPU_SHARE: f32 = 0.4;
const MIN_TITLE: usize = 28;

pub struct Top {
    pub cost: (u64, bool),
    pub limits: Option<Composed>,
}

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    listed: &[Listed<'_>],
    top: Top,
    offset: usize,
    graphics: &mut Graphics,
) {
    let Top {
        cost: (cost_total, complete),
        limits,
    } = top;
    let block = panel(
        vec![Span::styled("Agents", style::bold(theme().text))],
        (!listed.is_empty()).then(|| {
            vec![Span::styled(
                format!(
                    "{}{} in total",
                    if complete { "" } else { "≥" },
                    units::dollars(cost_total)
                ),
                style::fg(theme().dim),
            )]
        }),
    );
    let mut inner = block.inner(area);
    draw_panel(frame, area, block);
    if let Some(limits) = limits.filter(|line| line.width() > 0 && inner.height > 2) {
        let row = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(1), 1);
        limits.draw(frame, graphics, row, Style::new());
        inner = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
    }
    let width = inner.width as usize;
    if listed.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No coding agents running.",
                    style::fg(theme().dim),
                )),
                Line::from(Span::styled(
                    "Claude Code and Codex sessions show up here.",
                    style::fg(theme().faint),
                )),
            ]),
            inner,
        );
        return;
    }
    let selected = model.selected.checked_sub(offset);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut selected_end = 0;
    let mut group = None;
    for (index, item) in listed.iter().enumerate() {
        if group != Some(&item.group) {
            if group.is_some() {
                lines.push(Line::default());
            }
            lines.push(heading(&item.group));
            group = Some(&item.group);
        }
        let is_selected = selected == Some(index);
        match &item.entry {
            Entry::Session(agent) => {
                let away = item.group == Group::NeedsYou;
                lines.push(session_line(agent, is_selected, away, width));
                lines.extend(
                    agent
                        .session
                        .children
                        .iter()
                        .map(|child| child_line(child, width)),
                );
            }
            Entry::Fold { count, open, .. } => {
                lines.push(fold_line(*count, *open, is_selected, width));
            }
        }
        if is_selected {
            selected_end = lines.len();
        }
    }
    let visible = inner.height as usize;
    let first = selected_end.saturating_sub(visible);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(first).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), inner);
}

fn heading(group: &Group) -> Line<'static> {
    match group {
        Group::NeedsYou => Line::from(Span::styled(" Needs you", style::bold(theme().warn))),
        Group::Machine(name) => {
            Line::from(Span::styled(format!(" {name}"), style::fg(theme().dim)))
        }
    }
}

fn session_line(
    agent: &AgentRow<'_>,
    is_selected: bool,
    away: bool,
    width: usize,
) -> Line<'static> {
    let session = agent.session;
    let (mark, color) = style::agent(session.agent);
    let right = vec![state_span(session, agent.live)];
    spread(
        vec![
            Span::styled(
                if is_selected { "▎" } else { " " },
                style::fg(theme().accent),
            ),
            Span::styled(format!("{mark} "), style::fg(color)),
            Span::styled(
                session.title.clone(),
                if is_selected {
                    style::bold(theme().text)
                } else {
                    style::fg(theme().text)
                },
            ),
            Span::styled(
                if away {
                    format!(" · {}", agent.machine)
                } else {
                    String::new()
                },
                style::fg(theme().faint),
            ),
        ],
        right,
        width,
    )
    .style(selected_style(is_selected))
}

fn child_line(child: &Child, width: usize) -> Line<'static> {
    let mut spans = vec![
        Span::styled("     ↳ ", style::fg(theme().faint)),
        Span::styled(child.label.clone(), style::fg(theme().dim)),
    ];
    if !child.detail.is_empty() {
        spans.push(Span::styled(
            format!(" · {}", child.detail),
            style::fg(theme().faint),
        ));
    }
    Line::from(clip(spans, width))
}

fn fold_line(count: usize, open: bool, is_selected: bool, width: usize) -> Line<'static> {
    let text = if open {
        "▾ idle for a day or more".to_string()
    } else {
        format!("▸ {count} idle for a day or more")
    };
    Line::from(clip(
        vec![
            Span::styled(
                if is_selected { "▎" } else { " " },
                style::fg(theme().accent),
            ),
            Span::styled(format!(" {text}"), style::fg(theme().dim)),
        ],
        width,
    ))
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
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let width = inner.width as usize;
    let [view, input] = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);
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
    let mut cursor = Cursor::new(input);
    cursor.line(
        frame,
        Line::from(Span::styled("─".repeat(width), style::fg(theme().rule))),
    );
    cursor.line(frame, input_line(agent, draft, width));
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

fn input_line(agent: &AgentRow<'_>, draft: Option<&Draft>, width: usize) -> Line<'static> {
    let session = agent.session;
    let prompt = |color| Span::styled("› ", style::fg(color));
    let hint = |text: &str| Span::styled(text.to_string(), style::fg(theme().faint));
    let keys = |pairs: &[(&str, &str)]| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (key, label) in pairs {
            spans.push(Span::styled(format!("   {key}"), style::bold(theme().text)));
            spans.push(Span::styled(format!(" {label}"), style::fg(theme().faint)));
        }
        spans
    };
    if let Some(draft) = draft.filter(|draft| draft.session == session.id) {
        let right = keys(&[("↵", "send"), ("esc", "cancel")]);
        let room = width.saturating_sub(spans_width(&right) + 4);
        return spread(
            vec![
                prompt(theme().accent),
                Span::styled(tail(&draft.text, room), style::fg(theme().text)),
                Span::styled("▏", style::fg(theme().accent)),
            ],
            right,
            width,
        );
    }
    if !agent.live {
        return Line::from(vec![
            prompt(theme().faint),
            hint("its machine isn't answering"),
        ]);
    }
    if session.pane.is_none() {
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

pub fn render_fold(frame: &mut Frame, area: Rect, machine: &str, count: usize) {
    draw_panel(
        frame,
        area,
        panel(
            vec![Span::styled(machine.to_string(), style::bold(theme().text))],
            None,
        ),
    );
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("{count} sessions on {machine} have been idle for a day or more."),
                style::fg(theme().text),
            )),
            Line::from(Span::styled(
                "They stay folded so the busy ones stand out. Move down to look at one.",
                style::fg(theme().faint),
            )),
        ]),
        inner,
    );
}
