use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{
    Composed, Cursor, clip, icon, panel, rule, selected_style, short_name, spread, truncate,
};
use crate::proto::{AgentSession, FULL, Plan, SessionState, Usage, Waiting};
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{AgentRow, Capacity, Model, MoveTarget};
use crate::viewer::style::{self, theme};
use crate::viewer::units;

const LABEL: usize = 14;
const BUSY_MILLICORES: u32 = 500;

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
        vec![Span::styled("Agents", style::bold(theme().text))],
        (!agents.is_empty()).then(|| {
            vec![Span::styled(
                format!(
                    "{}{} in total",
                    if complete { "" } else { "≥" },
                    units::dollars(total)
                ),
                style::fg(theme().dim),
            )]
        }),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    if agents.is_empty() {
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
            ],
            vec![Span::styled(cost(session), style::fg(theme().text))],
            width,
        );
        let mut status = vec![Span::raw("   "), state_span(session, agent.live)];
        let place: Vec<&str> = [session.project.as_str(), agent.machine.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        status.push(Span::styled(
            format!("  {}", place.join(" · ")),
            style::fg(theme().faint),
        ));
        let mut details = Vec::new();
        if width >= 56 {
            details.push(Span::styled(
                format!(
                    "{}  {}",
                    units::model(&session.model),
                    context_text(session)
                ),
                style::fg(theme().dim),
            ));
        }
        if let Some((millicores, share)) = busy_cpu(agent) {
            details.push(Span::styled(
                format!("  {}", units::cores(millicores)),
                style::fg(style::load(share)),
            ));
        }
        lines.push(title.style(selected_style(is_selected)));
        lines.push(spread(status, details, width).style(selected_style(is_selected)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
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
    suggestion: Option<&MoveTarget>,
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
            vec![Span::styled(facts.join(" · "), style::fg(theme().dim))],
            width,
        )),
    );
    let mut status = vec![state_span(session, agent.live)];
    if agent.live && !matches!(session.state, SessionState::Idle) {
        let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
        status.push(Span::styled(
            format!("  for {since}"),
            style::fg(theme().dim),
        ));
    }
    cursor.line(frame, Line::from(clip(status, width)));
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

    let bar = width.saturating_sub(LABEL + 30).clamp(6, 40);
    let label = |text: &str| Span::styled(format!("{text:<LABEL$}"), style::bold(theme().text));
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
            style::fg(theme().text),
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
                        style::fg(theme().text),
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
                Span::styled(cost(session), style::bold(theme().text)),
                Span::styled(
                    if session.cost_complete {
                        "  at API list prices, subagents included"
                    } else {
                        "  so far; a model or tier without a known price is left out"
                    },
                    style::fg(theme().faint),
                ),
            ],
            width,
        )),
    );
    if let Some(usage) = session.usage {
        resources(&mut cursor, frame, graphics, usage, agent.capacity, bar);
    }
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
                        style::fg(theme().dim),
                    ),
                    Span::styled(units::count(value), style::fg(theme().text)),
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
                style::fg(theme().dim),
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
                style::fg(theme().text),
            ));
            cursor.composed(frame, graphics, line);
        }
    }
    cursor.skip();
    cursor.line(
        frame,
        Line::from(Span::styled(
            truncate(&format!("session {}", session.id), width),
            style::fg(theme().faint),
        )),
    );
}

fn resources(
    cursor: &mut Cursor,
    frame: &mut Frame,
    graphics: &mut Graphics,
    usage: Usage,
    capacity: Capacity,
    bar: usize,
) {
    let width = cursor.area.width as usize;
    cursor.skip();
    cursor.line(
        frame,
        rule(
            icon("◍"),
            "Resources",
            vec![Span::styled(
                "with everything it started",
                style::fg(theme().faint),
            )],
            width,
        ),
    );
    let label = |text: &str| {
        Span::styled(
            format!("  {text:<width$}", width = LABEL - 2),
            style::fg(theme().dim),
        )
    };
    let mut cpu = Composed::of(vec![label("cpu")]);
    match usage.cpu {
        Some(millicores) => {
            let share = cpu_share(millicores, capacity.cores);
            cpu.bar(bar, share, style::load(share), "detail-cpu".into());
            cpu.push(Span::styled(
                format!(
                    "  {:.1} of {} cores",
                    f64::from(millicores) / 1000.0,
                    capacity.cores
                ),
                style::fg(theme().text),
            ));
        }
        None => cpu.push(Span::styled("measuring…", style::fg(theme().dim))),
    }
    cursor.composed(frame, graphics, cpu);
    let share = usage.memory as f32 / capacity.memory.max(1) as f32;
    let mut memory = Composed::of(vec![label("memory")]);
    memory.bar(bar, share, style::load(share), "detail-memory".into());
    memory.push(Span::styled(
        format!(
            "  {} of {}",
            units::bytes(usage.memory),
            units::bytes(capacity.memory)
        ),
        style::fg(theme().text),
    ));
    cursor.composed(frame, graphics, memory);
}

fn until(epoch_secs: u64) -> String {
    units::duration(Duration::from_secs(
        epoch_secs.saturating_sub(crate::log::now_secs()),
    ))
}
