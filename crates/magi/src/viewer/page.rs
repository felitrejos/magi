use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::input::{self, Input, Key};
use super::style::{self, theme};
use super::terminal;
use super::view::markdown;

pub struct Page {
    pub title: String,
    pub diff: bool,
}

pub fn run(path: &Path, page: Page, remove: bool) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    if remove {
        let _ = fs::remove_file(path);
    }
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
            let mut height = 0;
            terminal.draw(|frame| {
                height = draw(frame, &page, &text, &mut top);
            })?;
            let step = height.max(1);
            match inputs.recv() {
                Ok(Input::Key(Key::Char('q') | Key::Escape | Key::Interrupt)) | Err(_) => {
                    return Ok(());
                }
                Ok(Input::Closed) => return Ok(()),
                Ok(Input::Key(Key::Down | Key::Char('j') | Key::Enter)) => top += 1,
                Ok(Input::Key(Key::Up | Key::Char('k'))) => top = top.saturating_sub(1),
                Ok(Input::Key(Key::Char(' ' | 'f'))) => top += step,
                Ok(Input::Key(Key::Char('b'))) => top = top.saturating_sub(step),
                Ok(Input::Key(Key::First | Key::Char('g'))) => top = 0,
                Ok(Input::Key(Key::Last | Key::Char('G'))) => top = usize::MAX,
                Ok(_) => {}
            }
        }
    })();
    ratatui::try_restore()?;
    io::stdout().flush()?;
    result
}

fn draw(frame: &mut Frame, page: &Page, text: &str, top: &mut usize) -> usize {
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme().panel));
    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    if inner.height < 3 || inner.width < 10 {
        return 0;
    }
    let width = inner.width as usize;
    let body = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 3);
    let room = body.height as usize;
    let lines = if page.diff {
        diff_lines(text)
    } else {
        markdown::markdown(text, width)
    };
    *top = (*top).min(lines.len().saturating_sub(room));
    let shown: Vec<Line<'static>> = lines.iter().skip(*top).take(room).cloned().collect();
    frame.render_widget(Paragraph::new(shown), body);
    let total = lines.len();
    let position = if total > room {
        let percent = ((*top + room).min(total) * 100) / total.max(1);
        format!("{percent}%")
    } else {
        String::new()
    };
    let header = Line::from(vec![
        Span::styled(page.title.clone(), style::bold(theme().text)),
        Span::styled(format!("  {position}"), style::fg(theme().faint)),
    ]);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let keys = Line::from(vec![
        Span::styled("esc", style::bold(theme().text)),
        Span::styled(" close   ", style::fg(theme().faint)),
        Span::styled("j k", style::bold(theme().text)),
        Span::styled(" scroll   ", style::fg(theme().faint)),
        Span::styled("space b", style::bold(theme().text)),
        Span::styled(" page", style::fg(theme().faint)),
    ]);
    frame.render_widget(
        Paragraph::new(keys),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    room
}

fn diff_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            let line = line.replace('\t', "    ");
            let color = if line.starts_with("+++") || line.starts_with("---") {
                return Line::from(Span::styled(line, style::bold(theme().dim)));
            } else if line.starts_with("diff ") || line.starts_with("new file") {
                return Line::from(Span::styled(line, style::bold(theme().text)));
            } else if line.starts_with('+') {
                theme().accent
            } else if line.starts_with('-') {
                theme().crit
            } else if line.starts_with("@@") {
                theme().second
            } else {
                theme().dim
            };
            Line::from(Span::styled(line, style::fg(color)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_colours_additions_removals_and_hunks() {
        let lines = diff_lines("diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n same\tline");
        let colour = |index: usize| lines[index].spans[0].style.fg;
        assert_eq!(colour(2), Some(theme().crit));
        assert_eq!(colour(3), Some(theme().accent));
        assert_eq!(colour(1), Some(theme().second));
        assert_eq!(lines[4].spans[0].content, " same    line");
    }
}
