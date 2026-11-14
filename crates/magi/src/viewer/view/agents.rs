use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Composed, Cursor, clip, icon, panel, rule, selected_style, spread};
use crate::proto::{
    AgentSession, BOLD, Child, DIM, FULL, INVERSE, ITALIC, Run, SessionState, Shade, Source,
    UNDERLINE, Waiting,
};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Entry, Group, Listed, Model, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BUSY_MILLICORES: u32 = 500;
const IDLE_MILLICORES: u32 = 50;
const METER: usize = 12;

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    listed: &[Listed<'_>],
    total: (u64, bool),
    offset: usize,
) {
    let (cost_total, complete) = total;
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
    let inner = block.inner(area);
    frame.render_widget(block, area);
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
    right.push(Span::styled(
        format!("  {:>7}", cost(session)),
        style::fg(theme().text),
    ));
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
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    let width = inner.width as usize;
    let mut cursor = Cursor::new(inner);
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
    cursor.line(
        frame,
        spread(
            vec![
                Span::styled(format!("{mark} "), style::fg(color)),
                Span::styled(session.title.clone(), style::bold(theme().text)),
            ],
            state,
            width,
        ),
    );
    let place: Vec<String> = [
        session.project.clone(),
        agent.machine.clone(),
        units::model(&session.model),
        format!(
            "running {}",
            units::duration(Duration::from_secs(u64::from(session.age_secs)))
        ),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect();
    cursor.line(
        frame,
        Line::from(clip(
            vec![Span::styled(place.join(" · "), style::fg(theme().dim))],
            width,
        )),
    );
    cursor.composed(frame, graphics, measures(agent));
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
    }
    cursor.skip();
    live_view(&mut cursor, frame, preview, width);
}

fn measures(agent: &AgentRow<'_>) -> Composed {
    let session = agent.session;
    let gap = || Span::raw("    ");
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
            format!(" · {}", units::bytes(usage.memory)),
            style::fg(theme().dim),
        ));
    }
    line
}

pub enum Preview<'a> {
    Waiting(Source),
    Ready(Source, &'a [Vec<Run>]),
    Failed(Source, &'a str),
    Gone,
}

fn live_view(cursor: &mut Cursor, frame: &mut Frame, preview: Preview<'_>, width: usize) {
    let source = match preview {
        Preview::Waiting(source) | Preview::Ready(source, _) | Preview::Failed(source, _) => source,
        Preview::Gone => Source::Conversation,
    };
    let (title, symbol, hint) = match source {
        Source::Pane => ("Screen", "▣", "live from Rex · ↵ to open"),
        Source::Conversation => ("Conversation", "▤", "latest messages"),
    };
    cursor.line(
        frame,
        rule(
            icon(symbol),
            title,
            vec![Span::styled(hint, style::fg(theme().faint))],
            width,
        ),
    );
    let note = |cursor: &mut Cursor, frame: &mut Frame, text: &str, color| {
        cursor.line(
            frame,
            Line::from(clip(
                vec![Span::styled(text.to_string(), style::fg(color))],
                width,
            )),
        );
    };
    let rows = match preview {
        Preview::Ready(_, rows) => rows,
        Preview::Waiting(Source::Pane) => {
            return note(cursor, frame, "connecting to the pane…", theme().faint);
        }
        Preview::Waiting(Source::Conversation) => {
            return note(cursor, frame, "reading the conversation…", theme().faint);
        }
        Preview::Failed(_, reason) => return note(cursor, frame, reason, theme().warn),
        Preview::Gone => {
            return note(cursor, frame, "its machine isn't answering", theme().faint);
        }
    };
    let lines: Vec<Line<'static>> = match source {
        Source::Pane => rows
            .iter()
            .map(|row| Line::from(clip(row.iter().map(run_span).collect(), width)))
            .collect(),
        Source::Conversation => rows.iter().flat_map(|row| wrap(row, width)).collect(),
    };
    let room = cursor.left() as usize;
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

fn wrap(row: &[Run], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for run in row {
        let style = run_span(run).style;
        for word in run.text.split_inclusive(' ') {
            let word_width = word.trim_end().width();
            if used > 0 && used + word_width > width {
                lines.push(Line::from(std::mem::take(&mut line)));
                used = 0;
            }
            let mut rest = word;
            while used == 0 && rest.trim_end().width() > width {
                let cut = split_at_width(rest, width);
                line.push(Span::styled(rest[..cut].to_string(), style));
                lines.push(Line::from(std::mem::take(&mut line)));
                rest = &rest[cut..];
            }
            if !(used == 0 && rest.trim().is_empty() && !lines.is_empty()) {
                line.push(Span::styled(rest.to_string(), style));
                used += rest.width();
            }
        }
    }
    lines.push(Line::from(line));
    lines
}

fn split_at_width(text: &str, width: usize) -> usize {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += c.width().unwrap_or(0);
        if used > width {
            return index.max(c.len_utf8());
        }
    }
    text.len()
}

fn run_span(run: &Run) -> Span<'static> {
    let color = |shade: Shade| match shade {
        Shade::Default => Color::Reset,
        Shade::Indexed(index) => Color::Indexed(index),
        Shade::Rgb(r, g, b) => Color::Rgb(r, g, b),
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
        Style::new()
            .fg(color(run.fg))
            .bg(color(run.bg))
            .add_modifier(modifiers),
    )
}

pub fn render_fold(frame: &mut Frame, area: Rect, machine: &str, count: usize) {
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    fn plain(text: &str) -> Run {
        Run {
            text: text.into(),
            fg: Shade::Default,
            bg: Shade::Default,
            flags: 0,
        }
    }

    #[test]
    fn conversation_text_wraps_between_words_and_only_splits_words_wider_than_the_panel() {
        assert_eq!(
            texts(&wrap(&[plain("the agent side builds now")], 12)),
            ["the agent ", "side builds ", "now"]
        );
        assert_eq!(
            texts(&wrap(&[plain("ab abcdefghijkl")], 5)),
            ["ab ", "abcde", "fghij", "kl"]
        );
        assert_eq!(texts(&wrap(&[plain("short")], 40)), ["short"]);
        let styled = wrap(
            &[
                plain("› "),
                Run {
                    flags: BOLD,
                    ..plain("run it")
                },
            ],
            40,
        );
        assert_eq!(texts(&styled), ["› run it"]);
    }
}
