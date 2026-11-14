use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{
    Composed, Cursor, clip, icon, panel, rule, selected_style, short_name, spread, truncate,
};
use crate::proto::{AgentSession, FULL, Plan, SessionState, Waiting};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Model};
use crate::viewer::style::{self, ACCENT, DIM, FAINT, TEXT, WARN, WARN_TINT};
use crate::viewer::units;

const LABEL: usize = 14;

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    agents: &[AgentRow<'_>],
    offset: usize,
) {
    let total: u64 = agents.iter().map(|agent| agent.session.cost_micros).sum();
    let complete = agents.iter().all(|agent| agent.session.cost_complete);
    let block = panel(
        vec![Span::styled("Agents", style::bold(TEXT))],
        (!agents.is_empty()).then(|| {
            vec![Span::styled(
                format!(
                    "{}{} in total",
                    if complete { "" } else { "≥" },
                    units::dollars(total)
                ),
                style::fg(DIM),
            )]
        }),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    if agents.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("No coding agents running.", style::fg(DIM))),
                Line::from(Span::styled(
                    "Claude Code and Codex sessions show up here.",
                    style::fg(FAINT),
                )),
            ]),
            inner,
        );
        return;
    }
    let visible = (inner.height as usize / 2).max(1);
    let selected = model.selected.checked_sub(offset);
    let first = selected
        .unwrap_or(0)
        .min(agents.len())
        .saturating_sub(visible - 1);
    let mut lines = Vec::new();
    for (index, agent) in agents.iter().enumerate().skip(first).take(visible) {
        let is_selected = selected == Some(index);
        let session = agent.session;
        let (mark, color) = style::agent(session.agent);
        let title = spread(
            vec![
                Span::styled(if is_selected { "▎" } else { " " }, style::fg(ACCENT)),
                Span::styled(format!("{mark} "), style::fg(color)),
                Span::styled(
                    session.title.clone(),
                    if is_selected {
                        style::bold(TEXT)
                    } else {
                        style::fg(TEXT)
                    },
                ),
            ],
            vec![Span::styled(cost(session), style::fg(TEXT))],
            width,
        );
        let mut status = vec![Span::raw("   "), state_span(session, agent.live)];
        let place: Vec<&str> = [session.project.as_str(), agent.machine.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        status.push(Span::styled(
            format!("  {}", place.join(" · ")),
            style::fg(FAINT),
        ));
        let details = if width >= 56 {
            vec![Span::styled(
                format!(
                    "{}  {}",
                    units::model(&session.model),
                    context_text(session)
                ),
                style::fg(DIM),
            )]
        } else {
            Vec::new()
        };
        lines.push(title.style(selected_style(is_selected)));
        lines.push(spread(status, details, width).style(selected_style(is_selected)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn state_span(session: &AgentSession, live: bool) -> Span<'static> {
    let (text, color) = state_text(session, live);
    match session.state {
        SessionState::Waiting(_) if live => {
            Span::styled(format!(" {text} "), style::pill(WARN, WARN_TINT))
        }
        _ => Span::styled(text, style::fg(color)),
    }
}

fn state_text(session: &AgentSession, live: bool) -> (String, Color) {
    if !live {
        return ("machine lost".into(), FAINT);
    }
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    match &session.state {
        SessionState::Waiting(Waiting::Permission(Some(tool))) => {
            (format!("needs you · {tool}"), WARN)
        }
        SessionState::Waiting(Waiting::Permission(None)) => ("needs you · approve".into(), WARN),
        SessionState::Waiting(Waiting::Input(reason)) => (format!("needs you · {reason}"), WARN),
        SessionState::Working { tool: Some(tool) } => (format!("working · {tool}"), ACCENT),
        SessionState::Working { tool: None } => ("working".into(), ACCENT),
        SessionState::Idle => (format!("idle {since}"), DIM),
    }
}

fn cost(session: &AgentSession) -> String {
    let dollars = units::dollars(session.cost_micros);
    if session.cost_complete {
        dollars
    } else {
        format!("≥{dollars}")
    }
}

fn context_text(session: &AgentSession) -> String {
    match session.context_used {
        Some(used) => format!(
            "ctx {}",
            units::percent(f32::from(used) / f32::from(FULL)).trim()
        ),
        None if session.context_tokens > 0 => {
            format!("ctx {}", units::count(session.context_tokens))
        }
        None => String::new(),
    }
}

pub fn render_detail(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentRow<'_>,
    plans: &[&Plan],
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
    cursor.line(
        frame,
        rule(
            Span::styled(mark.to_string(), style::fg(color)),
            &truncate(&session.title, width / 2),
            vec![Span::styled(session.agent.name(), style::fg(color))],
            width,
        ),
    );
    let facts: Vec<String> = [
        session.project.clone(),
        format!("on {}", agent.machine),
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
            vec![Span::styled(facts.join(" · "), style::fg(DIM))],
            width,
        )),
    );
    let mut status = vec![state_span(session, agent.live)];
    if agent.live && !matches!(session.state, SessionState::Idle) {
        let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
        status.push(Span::styled(format!("  for {since}"), style::fg(DIM)));
    }
    cursor.line(frame, Line::from(clip(status, width)));
    cursor.skip();

    let bar = width.saturating_sub(LABEL + 30).clamp(6, 40);
    let label = |text: &str| Span::styled(format!("{text:<LABEL$}"), style::bold(TEXT));
    if let Some(used) = session.context_used {
        let fraction = f32::from(used) / f32::from(FULL);
        let mut line = Composed::of(vec![label("Context")]);
        line.bar(bar, fraction, style::load(fraction), "context".into());
        line.push(Span::styled(
            format!(
                "  {} · {} tokens",
                units::percent(fraction).trim(),
                units::count(session.context_tokens)
            ),
            style::fg(TEXT),
        ));
        cursor.composed(frame, graphics, line);
    } else if session.context_tokens > 0 {
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    label("Context"),
                    Span::styled(
                        format!("{} tokens", units::count(session.context_tokens)),
                        style::fg(TEXT),
                    ),
                ],
                width,
            )),
        );
    }
    cursor.line(
        frame,
        Line::from(clip(
            vec![
                label("Cost"),
                Span::styled(cost(session), style::bold(TEXT)),
                Span::styled(
                    if session.cost_complete {
                        "  at API list prices, subagents included"
                    } else {
                        "  so far; a model or tier without a known price is left out"
                    },
                    style::fg(FAINT),
                ),
            ],
            width,
        )),
    );
    cursor.skip();
    let tokens = session.tokens;
    cursor.line(frame, rule(icon("◇"), "Tokens", Vec::new(), width));
    for (name, value) in [
        ("output", tokens.output),
        ("input", tokens.input),
        ("cache read", tokens.cache_read),
        ("cache write", tokens.cache_write),
    ] {
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    Span::styled(
                        format!("  {name:<width$}", width = LABEL - 2),
                        style::fg(DIM),
                    ),
                    Span::styled(units::count(value), style::fg(TEXT)),
                ],
                width,
            )),
        );
    }

    if let Some(plan) = plans.iter().find(|plan| plan.agent == session.agent) {
        cursor.skip();
        cursor.line(
            frame,
            rule(
                icon("◔"),
                &format!("{} plan", short_name(plan.agent)),
                Vec::new(),
                width,
            ),
        );
        for window in &plan.windows {
            let fraction = f32::from(window.used) / f32::from(FULL);
            let mut line = Composed::of(vec![Span::styled(
                format!("  {:<width$}", window.long_label(), width = LABEL - 2),
                style::fg(DIM),
            )]);
            line.bar(
                bar,
                fraction,
                style::load(fraction),
                format!("detail-plan:{}", window.minutes),
            );
            line.push(Span::styled(
                format!(
                    "  {} · resets in {}",
                    units::percent(fraction).trim(),
                    until(window.resets_at)
                ),
                style::fg(TEXT),
            ));
            cursor.composed(frame, graphics, line);
        }
    }
    cursor.skip();
    cursor.line(
        frame,
        Line::from(Span::styled(
            truncate(&format!("session {}", session.id), width),
            style::fg(FAINT),
        )),
    );
}

fn until(epoch_secs: u64) -> String {
    units::duration(Duration::from_secs(
        epoch_secs.saturating_sub(crate::log::now_secs()),
    ))
}
