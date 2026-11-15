use std::num::NonZeroU16;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::bounded;
use crate::proto::{BOLD, DIM, INVERSE, ITALIC, Run, Shade, UNDERLINE};
use crate::sessions::rex::Location;

const SIZE_KEPT_FOR: Duration = Duration::from_secs(5);
const REX_LIMIT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Capturer {
    size: Option<(String, Instant, (u16, u16))>,
}

#[derive(Deserialize)]
struct Size {
    columns: u16,
    rows: u16,
}

impl Capturer {
    pub fn capture(&mut self, pane: &Location, now: Instant) -> Result<Vec<Vec<Run>>, String> {
        let (rows, cols) = self.size(pane, now)?;
        let output = bounded::output(
            &mut pane.rex(
                "2s",
                &[
                    "capture",
                    "-s",
                    &pane.session,
                    "-b",
                    &pane.block,
                    "--format",
                    "vt",
                ],
            ),
            REX_LIMIT,
        )
        .map_err(|error| format!("can't run rex: {error}"))?;
        if !output.status.success() {
            return Err(failure(&output.stderr));
        }
        Ok(render(&output.stdout, rows, cols))
    }

    fn size(&mut self, pane: &Location, now: Instant) -> Result<(u16, u16), String> {
        if let Some((block, at, size)) = &self.size
            && *block == pane.block
            && now.duration_since(*at) < SIZE_KEPT_FOR
        {
            return Ok(*size);
        }
        let output = bounded::output(
            &mut pane.rex(
                "2s",
                &[
                    "do",
                    "-s",
                    &pane.session,
                    "-b",
                    &pane.block,
                    "com.superlogical.terminal.size",
                ],
            ),
            REX_LIMIT,
        )
        .map_err(|error| format!("can't run rex: {error}"))?;
        if !output.status.success() {
            return Err(failure(&output.stderr));
        }
        let size: Size = serde_json::from_slice(&output.stdout).map_err(|error| {
            format!("Rex reported the pane's size in a form magi can't read: {error}")
        })?;
        let size = (size.rows, size.columns);
        self.size = Some((pane.block.clone(), now, size));
        Ok(size)
    }
}

fn failure(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("rex failed")
        .trim_start_matches("Error: ");
    format!("Rex couldn't show the pane: {reason}")
}

pub fn render(bytes: &[u8], rows: u16, cols: u16) -> Vec<Vec<Run>> {
    let (Some(rows), Some(cols)) = (NonZeroU16::new(rows), NonZeroU16::new(cols)) else {
        return Vec::new();
    };
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(bytes);
    let screen = parser.screen();
    let mut lines: Vec<Vec<Run>> = (0..rows.get())
        .map(|row| line(screen, row, cols.get()))
        .collect();
    while lines.last().is_some_and(|line| is_blank(line)) {
        lines.pop();
    }
    lines
}

fn line(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let text = if cell.has_contents() {
            cell.contents()
        } else {
            " "
        };
        let (fg, bg, flags) = (shade(cell.fgcolor()), shade(cell.bgcolor()), flags(cell));
        match runs.last_mut() {
            Some(last) if last.fg == fg && last.bg == bg && last.flags == flags => {
                last.text.push_str(text);
            }
            _ => runs.push(Run {
                text: text.to_string(),
                fg,
                bg,
                flags,
            }),
        }
    }
    if let Some(last) = runs.last_mut()
        && last.bg == Shade::Default
        && last.flags & (INVERSE | UNDERLINE) == 0
    {
        last.text.truncate(last.text.trim_end().len());
    }
    runs.retain(|run| !run.text.is_empty());
    runs
}

fn is_blank(line: &[Run]) -> bool {
    line.iter().all(|run| {
        run.text.trim().is_empty() && run.bg == Shade::Default && run.flags & INVERSE == 0
    })
}

fn shade(color: vt100::Color) -> Shade {
    match color {
        vt100::Color::Default => Shade::Default,
        vt100::Color::Idx(index) => Shade::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Shade::Rgb(r, g, b),
    }
}

fn flags(cell: &vt100::Cell) -> u8 {
    [
        (cell.bold(), BOLD),
        (cell.dim(), DIM),
        (cell.italic(), ITALIC),
        (cell.underline(), UNDERLINE),
        (cell.inverse(), INVERSE),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |flags, (_, flag)| flags | flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, fg: Shade, flags: u8) -> Run {
        Run {
            text: text.into(),
            fg,
            bg: Shade::Default,
            flags,
        }
    }

    #[test]
    fn styled_text_becomes_runs_and_only_the_visible_screen_is_kept() {
        let mut bytes = Vec::new();
        for n in 0..10 {
            bytes.extend_from_slice(format!("old line {n}\r\n").as_bytes());
        }
        bytes.extend_from_slice(
            b"\x1b[1;38;2;217;119;87m\xe2\x9c\xbb Claude\x1b[0m ready\r\n> \x1b[3mtype here\x1b[0m",
        );
        let lines = render(&bytes, 4, 20);
        assert_eq!(
            lines,
            [
                vec![run("old line 8", Shade::Default, 0)],
                vec![run("old line 9", Shade::Default, 0)],
                vec![
                    run("✻ Claude", Shade::Rgb(217, 119, 87), BOLD),
                    run(" ready", Shade::Default, 0)
                ],
                vec![
                    run("> ", Shade::Default, 0),
                    run("type here", Shade::Default, ITALIC)
                ],
            ]
        );
    }

    #[test]
    fn trailing_blank_rows_go_but_highlighted_ones_stay() {
        assert_eq!(render(b"top\r\n\r\n\r\n", 5, 10).len(), 1);
        let highlighted = render(b"top\r\n\x1b[7m   \x1b[0m\r\n", 5, 10);
        assert_eq!(highlighted.len(), 2);
        assert_eq!(highlighted[1][0].flags, INVERSE);
        assert!(render(b"anything", 0, 10).is_empty());
    }

    #[test]
    fn wide_characters_take_one_run_slot_not_two() {
        let lines = render("日本 ok".as_bytes(), 1, 10);
        assert_eq!(lines, [vec![run("日本 ok", Shade::Default, 0)]]);
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_render"]
    fn bench_render() {
        let mut bytes = Vec::new();
        for n in 0..20_000 {
            bytes.extend_from_slice(
                format!(
                    "\x1b[38;5;{}mline {n} of a long scrollback with colour\x1b[0m\r\n",
                    n % 256
                )
                .as_bytes(),
            );
        }
        let started = Instant::now();
        let lines = render(&bytes, 50, 184);
        println!(
            "rendered {} KB of scrollback to {} rows in {:?}",
            bytes.len() / 1024,
            lines.len(),
            started.elapsed()
        );
    }
}
