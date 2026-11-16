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

use super::graphics::Graphics;
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
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut terminal = ratatui::try_init()?;
    let reply = terminal::probe(&mut io::stdout())?;
    style::update(&reply);
    let mut graphics = Graphics::from_reply(&reply);
    let mut typeset = (!page.diff && graphics.cell().is_some())
        .then(|| typeset::Typeset::new(&text, &base))
        .flatten();
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
                height = draw(
                    frame,
                    &page,
                    &text,
                    &mut top,
                    &mut graphics,
                    typeset.as_mut(),
                );
            })?;
            graphics.flush(&mut io::stdout())?;
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
    let _ = graphics.clear(&mut io::stdout());
    ratatui::try_restore()?;
    io::stdout().flush()?;
    result
}

fn draw(
    frame: &mut Frame,
    page: &Page,
    text: &str,
    top: &mut usize,
    graphics: &mut Graphics,
    typeset: Option<&mut typeset::Typeset>,
) -> usize {
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
    let total = match (typeset, graphics.cell()) {
        (Some(typeset), Some((cell_width, cell_height))) => {
            typeset.fit(u32::from(body.width) * cell_width, cell_height);
            let total = typeset.rows();
            *top = (*top).min(total.saturating_sub(room));
            for offset in 0..room.min(total.saturating_sub(*top)) {
                let row = *top + offset;
                let strip = Rect::new(body.x, body.y + offset as u16, body.width, 1);
                graphics.strip(
                    frame.buffer_mut(),
                    strip,
                    format!("page:{row}"),
                    typeset.key(row),
                    |width, height| typeset.pixels(row, width, height),
                );
            }
            total
        }
        _ => {
            let lines = if page.diff {
                diff_lines(text)
            } else {
                markdown::markdown(text, width)
            };
            *top = (*top).min(lines.len().saturating_sub(room));
            let shown: Vec<Line<'static>> = lines.iter().skip(*top).take(room).cloned().collect();
            frame.render_widget(Paragraph::new(shown), body);
            lines.len()
        }
    };
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
        Span::styled("q", style::bold(theme().text)),
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

#[cfg(target_os = "macos")]
mod typeset {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::path::Path;

    use mddog::render::engine::Engine;

    use crate::viewer::style;

    pub struct Typeset {
        engine: Engine,
        scale: f64,
        width: u32,
        row_height: u32,
        page_height: f64,
        layout: u64,
    }

    impl Typeset {
        pub fn new(text: &str, base: &Path) -> Option<Self> {
            let name = if style::is_dark() {
                "github-dark"
            } else {
                "github-light"
            };
            let theme = mddog::theme::builtin(name)?;
            let scale = mddog::term::screen::display_scale();
            let engine = Engine::new(1.0, scale, true);
            engine.load(text, base, true, theme);
            Some(Self {
                engine,
                scale,
                width: 0,
                row_height: 1,
                page_height: 0.0,
                layout: 0,
            })
        }

        pub fn fit(&mut self, width: u32, row_height: u32) {
            if width == self.width && row_height == self.row_height {
                return;
            }
            self.engine.resize(f64::from(width) / self.scale);
            self.width = width;
            self.row_height = row_height.max(1);
            self.page_height = self.engine.layout().height;
            self.layout += 1;
        }

        pub fn rows(&self) -> usize {
            (self.page_height * self.scale / f64::from(self.row_height)).ceil() as usize
        }

        pub fn key(&self, row: usize) -> u64 {
            let mut hasher = DefaultHasher::new();
            (self.layout, row, self.width, self.row_height).hash(&mut hasher);
            hasher.finish()
        }

        pub fn pixels(&self, row: usize, width: u32, height: u32) -> Option<Vec<u8>> {
            let top = row as f64 * f64::from(self.row_height) / self.scale;
            let snapshot =
                self.engine
                    .snapshot(top, f64::from(height) / self.scale, self.scale, |len| {
                        Some(vec![0u8; len])
                    })?;
            (snapshot.width == width as usize && snapshot.height == height as usize)
                .then_some(snapshot.rgba)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod typeset {
    use std::path::Path;

    pub enum Typeset {}

    impl Typeset {
        pub fn new(_text: &str, _base: &Path) -> Option<Self> {
            None
        }

        pub fn fit(&mut self, _width: u32, _row_height: u32) {
            match *self {}
        }

        pub fn rows(&self) -> usize {
            match *self {}
        }

        pub fn key(&self, _row: usize) -> u64 {
            match *self {}
        }

        pub fn pixels(&self, _row: usize, _width: u32, _height: u32) -> Option<Vec<u8>> {
            match *self {}
        }
    }
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
