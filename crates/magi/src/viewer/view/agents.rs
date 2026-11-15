use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Composed, Cursor, clip, markdown, panel, selected_style, spans_width, spread};
use crate::proto::{
    AgentSession, BOLD, Child, Content, DIM, FULL, INVERSE, ITALIC, Run, SessionState, Shade,
    UNDERLINE, Waiting,
};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Entry, Group, Listed, Model, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;

const BUSY_MILLICORES: u32 = 500;
const IDLE_MILLICORES: u32 = 50;
const METER: usize = 12;

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
    frame.render_widget(block, area);
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
    let mut right = vec![state_span(session, agent.live)];
    if let Some((millicores, share)) = busy_cpu(agent) {
        right.push(Span::styled(
            format!("  {}", units::cores(millicores)),
            style::fg(style::load(share)),
        ));
    }
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

fn busy_cpu(agent: &AgentRow<'_>) -> Option<(u32, f32)> {
    let millicores = agent
        .session
        .usage?
        .cpu
        .filter(|cpu| *cpu >= BUSY_MILLICORES)?;
    Some((millicores, cpu_share(millicores, agent.capacity.cores)))
}

fn cpu_share(millicores: u32, cores: usize) -> f32 {
    millicores as f32 / (cores.max(1) * 1000) as f32
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
    graphics: &mut Graphics,
) {
    let session = agent.session;
    let (mark, color) = style::agent(session.agent);
    if area.height < 3 || area.width < 16 {
        return;
    }
    let state = state_spans(agent);
    let room = (area.width as usize).saturating_sub(spans_width(&state) + 10);
    let title = clip(
        vec![
            Span::styled(format!("{mark} "), style::fg(color)),
            Span::styled(session.title.clone(), style::bold(theme().text)),
        ],
        room,
    );
    frame.render_widget(panel(title, Some(state)), area);
    let place: Vec<String> = [
        session.project.clone(),
        units::model(&session.model),
        format!(
            "running {}",
            units::duration(Duration::from_secs(u64::from(session.age_secs)))
        ),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect();
    let footing = Rect::new(area.x + 2, area.y + area.height - 1, area.width - 4, 1);
    let mut facts = measures(agent);
    facts.spans.insert(0, Span::raw(" "));
    facts.bars.iter_mut().for_each(|bar| bar.offset += 1);
    facts.push(Span::raw(" "));
    Composed::of(vec![Span::styled(
        format!(" {} ", place.join(" · ")),
        style::fg(theme().dim),
    )])
    .spread_by(
        facts,
        footing.width as usize,
        "─",
        style::fg(theme().border),
    )
    .draw(frame, graphics, footing, Style::new());
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let width = inner.width as usize;
    let mut cursor = Cursor::new(inner);
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

fn measures(agent: &AgentRow<'_>) -> Composed {
    let session = agent.session;
    let gap = || Span::styled(" · ", style::fg(theme().faint));
    let mut line = Composed::of(Vec::new());
    match session.context_used {
        Some(used) => {
            let fraction = f32::from(used) / f32::from(FULL);
            line.push(Span::styled("context ", style::fg(theme().dim)));
            line.bar(METER, fraction, style::load(fraction), "context".into());
            line.push(Span::styled(
                format!(" {}", units::percent(fraction).trim()),
                style::fg(theme().text),
            ));
            line.push(gap());
        }
        None if session.context_tokens > 0 => {
            line.push(Span::styled(
                format!("context {} tokens", units::count(session.context_tokens)),
                style::fg(theme().dim),
            ));
            line.push(gap());
        }
        None => {}
    }
    line.push(Span::styled(cost(session), style::bold(theme().text)));
    if let Some(usage) = session.usage {
        line.push(gap());
        let cpu = match usage.cpu {
            Some(millicores) if millicores < IDLE_MILLICORES => "cpu idle".to_string(),
            Some(millicores) => units::cores(millicores),
            None => "measuring".to_string(),
        };
        let share = usage.cpu.map_or(0.0, |millicores| {
            cpu_share(millicores, agent.capacity.cores)
        });
        line.push(Span::styled(cpu, style::fg(style::load(share))));
        line.push(Span::styled(
            format!(" · mem {}", units::bytes(usage.memory)),
            style::fg(theme().dim),
        ));
    }
    line
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
    frame.render_widget(
        panel(
            vec![Span::styled(machine.to_string(), style::bold(theme().text))],
            None,
        ),
        area,
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
