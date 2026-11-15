use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};

use super::{Cursor, clip, draw_panel, panel, spans_width, spread, tail};
use crate::viewer::model::Picker;
use crate::viewer::style::{self, theme};

pub fn render(frame: &mut Frame, area: Rect, picker: &Picker, recent: &[String], home: &str) {
    if area.height < 5 || area.width < 16 {
        return;
    }
    let agent = if picker.codex { "Codex" } else { "Claude" };
    draw_panel(
        frame,
        area,
        panel(
            vec![Span::styled(
                format!("New {agent} session on {}", picker.machine),
                style::bold(theme().text),
            )],
            None,
        ),
    );
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let width = inner.width as usize;
    let list = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let mut cursor = Cursor::new(list);
    let note = |text: &str| Line::from(Span::styled(text.to_string(), style::fg(theme().faint)));
    match &picker.folders {
        None => cursor.line(
            frame,
            note(&format!("asking {} for its projects…", picker.machine)),
        ),
        Some(_) => {
            let shown = picker.shown(recent);
            if shown.is_empty() {
                cursor.line(frame, note("No project folder matches."));
            }
            let room = list.height as usize;
            let first = picker.selected.saturating_sub(room.saturating_sub(1));
            for (index, folder) in shown.iter().enumerate().skip(first).take(room) {
                let name = folder.rsplit('/').next().unwrap_or(folder);
                let place = folder
                    .strip_prefix(home)
                    .map(|rest| format!("~{rest}"))
                    .unwrap_or_else(|| folder.to_string());
                let is_selected = index == picker.selected;
                let mut spans = vec![
                    Span::styled(
                        if is_selected { "▎" } else { " " },
                        style::fg(theme().accent),
                    ),
                    Span::styled(
                        name.to_string(),
                        if is_selected {
                            style::bold(theme().text)
                        } else {
                            style::fg(theme().text)
                        },
                    ),
                ];
                if recent.iter().any(|project| project == name) {
                    spans.push(Span::styled("  active", style::fg(theme().accent)));
                }
                let right = vec![Span::styled(place, style::fg(theme().faint))];
                let mut line = spread(clip(spans, width / 2), right, width);
                if is_selected {
                    line = line.style(style::fg(theme().text).bg(theme().selected));
                }
                cursor.line(frame, line);
            }
        }
    }
    let input = Rect::new(
        inner.x,
        inner.y + inner.height.saturating_sub(2),
        inner.width,
        2,
    );
    let mut cursor = Cursor::new(input);
    cursor.line(
        frame,
        Line::from(Span::styled("─".repeat(width), style::fg(theme().rule))),
    );
    let keys = |pairs: &[(&str, &str)]| -> Vec<Span<'static>> {
        pairs
            .iter()
            .flat_map(|(key, label)| {
                [
                    Span::styled(format!("   {key}"), style::bold(theme().text)),
                    Span::styled(format!(" {label}"), style::fg(theme().faint)),
                ]
            })
            .collect()
    };
    let other = if picker.codex { "Claude" } else { "Codex" };
    let right = keys(&[("↵", "open"), ("⇥", other), ("esc", "cancel")]);
    let room = width.saturating_sub(spans_width(&right) + 4);
    let typed = if picker.filter.is_empty() {
        Span::styled("type to filter", style::fg(theme().faint))
    } else {
        Span::styled(tail(&picker.filter, room), style::fg(theme().text))
    };
    cursor.line(
        frame,
        spread(
            vec![
                Span::styled("› ", style::fg(theme().accent)),
                typed,
                Span::styled("▏", style::fg(theme().accent)),
            ],
            right,
            width,
        ),
    );
}
