use std::path::Path;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{clip, panel};
use crate::log;
use crate::viewer::alerts::{Alerts, Event, Severity};
use crate::viewer::style::{self, CRIT, DIM, FAINT, TEXT, WARN};

pub fn render(frame: &mut Frame, area: Rect, alerts: &Alerts) {
    let active = alerts.active().count();
    let block = panel(
        vec![
            Span::styled("Alerts", style::bold(TEXT)),
            Span::styled(
                format!("  {active} active"),
                if active > 0 {
                    style::bold(WARN)
                } else {
                    style::fg(DIM)
                },
            ),
        ],
        alerts.path().map(|path| {
            vec![Span::styled(
                format!("kept in {}", home_relative(path)),
                style::fg(FAINT),
            )]
        }),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    let today = log::local_time(log::now_secs(), c"%Y-%m-%d");
    let mut history = alerts.history().peekable();
    if history.peek().is_none() {
        lines.push(Line::from(Span::styled("No alerts yet.", style::fg(DIM))));
        lines.push(Line::from(Span::styled(
            "One is raised when an agent needs you, a machine stops answering or stays critical, or a plan passes 90%.",
            style::fg(FAINT),
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
                    Severity::Error => CRIT,
                    Severity::Permission | Severity::Question => WARN,
                },
            ),
            Event::Resolved => ("○", format!("resolved · {}", entry.text), DIM),
        };
        lines.push(Line::from(clip(
            vec![
                Span::styled(format!("{when:>12}  "), style::fg(FAINT)),
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
