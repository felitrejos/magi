use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::alerts::{Entry, Event, Severity, dismiss_open, read_history};
use super::input::{self, Input, Key};
use super::style::{self, theme};
use super::terminal;
use super::view::spread as spread_line;
use crate::{log, net};

const REFRESH: Duration = Duration::from_secs(1);
const DAY: u64 = 86_400;

pub fn run() -> io::Result<()> {
    let path = net::state_dir()?.join("alerts.jsonl");
    let mut terminal = ratatui::try_init()?;
    let reply = terminal::probe(&mut io::stdout())?;
    style::update(&reply);
    let (sender, inputs) = mpsc::channel();
    let early = terminal::unanswered(&reply).to_vec();
    thread::Builder::new().name("input".into()).spawn(move || {
        let _ = input::read(&early, |input| sender.send(input).is_ok());
    })?;
    let mut top = 0usize;
    let result = (|| -> io::Result<()> {
        loop {
            let entries: Vec<Entry> = read_history(&path).unwrap_or_default().into();
            let mut room = 0;
            terminal.draw(|frame| room = draw(frame, &entries, &mut top))?;
            let page = room.max(1);
            match inputs.recv_timeout(REFRESH) {
                Ok(Input::Key(Key::Char('q') | Key::Escape | Key::Interrupt))
                | Ok(Input::Closed)
                | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(Input::Key(Key::Down | Key::Char('d'))) => top += 1,
                Ok(Input::Key(Key::Up | Key::Char('u'))) => top = top.saturating_sub(1),
                Ok(Input::Key(Key::PageDown | Key::Char(' '))) => top += page,
                Ok(Input::Key(Key::PageUp | Key::Char('b'))) => top = top.saturating_sub(page),
                Ok(Input::Wheel { up, .. }) => {
                    top = if up { top.saturating_sub(3) } else { top + 3 }
                }
                Ok(Input::Key(Key::First | Key::Char('g'))) => top = 0,
                Ok(Input::Key(Key::Char('c'))) => dismiss_open(&path, log::now_secs())?,
                _ => {}
            }
        }
    })();
    ratatui::try_restore()?;
    io::stdout().flush()?;
    result
}

fn draw(frame: &mut Frame, entries: &[Entry], top: &mut usize) -> usize {
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme().panel));
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    if inner.height < 4 || inner.width < 20 {
        return 0;
    }
    let width = usize::from(inner.width);
    let open = open_count(entries);
    let header = spread_line(
        vec![Span::styled("Alerts", style::bold(theme().text))],
        vec![Span::styled(
            if open == 0 {
                "nothing needs you".to_string()
            } else {
                format!("{open} still open")
            },
            if open == 0 {
                style::fg(theme().dim)
            } else {
                style::bold(theme().warn)
            },
        )],
        width,
    );
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    let lines = feed(entries, log::now_secs(), width);
    let room = usize::from(body.height);
    let overflows = lines.len() > room;
    *top = (*top).min(lines.len().saturating_sub(room));
    let shown: Vec<Line<'static>> = lines.into_iter().skip(*top).take(room).collect();
    frame.render_widget(Paragraph::new(shown), body);
    let mut keys = vec![
        Span::styled("esc", style::bold(theme().text)),
        Span::styled(" close", style::fg(theme().faint)),
    ];
    if open > 0 {
        keys.extend([
            Span::styled("   c", style::bold(theme().text)),
            Span::styled(" clear", style::fg(theme().faint)),
        ]);
    }
    if overflows {
        keys.extend([
            Span::styled("   u d", style::bold(theme().text)),
            Span::styled(" scroll", style::fg(theme().faint)),
        ]);
    }
    let keys = Line::from(keys);
    frame.render_widget(
        Paragraph::new(keys),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    room
}

struct Item<'a> {
    raised: &'a Entry,
    closed: Option<(u64, Event)>,
}

fn items(entries: &[Entry]) -> Vec<Item<'_>> {
    let mut items: Vec<Item<'_>> = Vec::new();
    for entry in entries {
        match entry.event {
            Event::Raised => items.push(Item {
                raised: entry,
                closed: None,
            }),
            Event::Resolved | Event::Dismissed => {
                if let Some(item) = items
                    .iter_mut()
                    .rev()
                    .find(|item| item.raised.key == entry.key && item.closed.is_none())
                {
                    item.closed = Some((entry.at, entry.event));
                }
            }
        }
    }
    items.reverse();
    items
}

fn open_count(entries: &[Entry]) -> usize {
    items(entries)
        .iter()
        .filter(|item| item.closed.is_none())
        .count()
}

fn day_label(at: u64, now: u64) -> String {
    let day = |secs: u64| log::local_time(secs, c"%Y-%m-%d");
    if day(at) == day(now) {
        "Today".into()
    } else if day(at) == day(now.saturating_sub(DAY)) {
        "Yesterday".into()
    } else {
        log::local_time(at, c"%A %d %B")
    }
}

pub fn feed(entries: &[Entry], now: u64, width: usize) -> Vec<Line<'static>> {
    let items = items(entries);
    if items.is_empty() {
        return vec![
            Line::from(Span::styled("No alerts yet.", style::fg(theme().dim))),
            Line::from(Span::styled(
                "One is raised when an agent needs you, a machine stops answering or stays critical, or a plan passes 90%.",
                style::fg(theme().faint),
            )),
        ];
    }
    let mut lines = Vec::new();
    let mut last_day = String::new();
    for item in items {
        let day = day_label(item.raised.at, now);
        if day != last_day {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                day.clone(),
                style::bold(theme().dim),
            )));
            last_day = day;
        }
        let color = match (item.closed, item.raised.severity) {
            (Some(_), _) => theme().dim,
            (None, Severity::Error) => theme().crit,
            (None, _) => theme().warn,
        };
        let outcome = match item.closed {
            Some((_, Event::Dismissed)) => "dismissed".into(),
            Some((at, _)) => format!(
                "resolved after {}",
                crate::viewer::units::duration(Duration::from_secs(
                    at.saturating_sub(item.raised.at)
                ))
            ),
            None => "open".into(),
        };
        lines.push(spread_line(
            vec![
                Span::styled(
                    format!("{}  ", log::local_time(item.raised.at, c"%H:%M")),
                    style::fg(theme().faint),
                ),
                Span::styled(
                    if item.closed.is_some() {
                        "○ "
                    } else {
                        "● "
                    },
                    style::fg(color),
                ),
                Span::styled(item.raised.text.clone(), style::fg(color)),
            ],
            vec![Span::styled(
                if item.raised.machine.is_empty() {
                    outcome
                } else {
                    format!("{} · {outcome}", item.raised.machine)
                },
                style::fg(theme().faint),
            )],
            width,
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: u64, event: Event, key: &str, text: &str) -> Entry {
        Entry {
            at,
            event,
            key: key.into(),
            machine: "mac".into(),
            text: text.into(),
            severity: Severity::Permission,
        }
    }

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

    #[test]
    fn the_feed_groups_alerts_by_day_newest_first_with_how_each_was_resolved() {
        let now = 1_794_830_400;
        let entries = [
            entry(now - DAY - 600, Event::Raised, "a", "old one needs you"),
            entry(now - DAY - 480, Event::Resolved, "a", "old one needs you"),
            entry(now - 300, Event::Raised, "b", "deploy needs you"),
            entry(now - 60, Event::Raised, "c", "homelab stopped answering"),
            entry(now - 30, Event::Resolved, "c", "homelab stopped answering"),
        ];
        let lines = texts(&feed(&entries, now, 80));
        assert_eq!(lines[0], "Today");
        assert!(lines[1].contains("homelab stopped answering"), "{lines:?}");
        assert!(lines[1].ends_with("mac · resolved after 30s"), "{lines:?}");
        assert!(lines[2].ends_with("mac · open"), "{lines:?}");
        assert_eq!(lines[4], "Yesterday");
        assert!(lines[5].ends_with("resolved after 2m"), "{lines:?}");
        assert_eq!(open_count(&entries), 1);
    }
}
