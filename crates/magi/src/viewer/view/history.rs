use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{clip, icon, rule};
use crate::log;
use crate::viewer::alerts::{Alerts, Event, Severity};
use crate::viewer::style::{self, theme};

pub fn render(frame: &mut Frame, area: Rect, alerts: &Alerts) {
    let active = alerts.active().count();
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    let mut right = vec![Span::styled(
        format!("{active} active"),
        if active > 0 {
            style::bold(theme().warn)
        } else {
            style::fg(theme().dim)
        },
    )];
    if let Some(path) = alerts.path() {
        right.push(Span::styled(
            format!(" · kept in {}", home_relative(path)),
            style::fg(theme().faint),
        ));
    }
    frame.render_widget(
        Paragraph::new(rule(icon("●"), "Alerts", right, inner.width as usize)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let inner = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let width = inner.width as usize;
    let mut lines = Vec::new();
    let today = log::local_time(log::now_secs(), c"%Y-%m-%d");
    let mut history = alerts.history().peekable();
    if history.peek().is_none() {
        lines.push(Line::from(Span::styled(
            "No alerts yet.",
            style::fg(theme().dim),
        )));
        lines.push(Line::from(Span::styled(
            "One is raised when an agent needs you, a machine stops answering or stays critical, or a plan passes 90%.",
            style::fg(theme().faint),
        )));
    }
    for entry in history.take(inner.height as usize) {
        let when = if log::local_time(entry.at, c"%Y-%m-%d") == today {
            log::local_time(entry.at, c"%H:%M")
        } else {
            log::local_time(entry.at, c"%b %d %H:%M")
        };
        let (mark, text, color) = match entry.event {
            Event::Raised => (
                "●",
                entry.text.clone(),
                match entry.severity {
                    Severity::Error => theme().crit,
                    Severity::Permission | Severity::Question => theme().warn,
                },
            ),
            Event::Resolved => ("○", format!("resolved · {}", entry.text), theme().dim),
        };
        lines.push(Line::from(clip(
            vec![
                Span::styled(format!("{when:>12}  "), style::fg(theme().faint)),
                Span::styled(format!("{mark} "), style::fg(color)),
                Span::styled(text, style::fg(color)),
            ],
            width,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn home_relative(path: &Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if shown.starts_with(&home) => format!("~{}", &shown[home.len()..]),
        _ => shown,
    }
}
