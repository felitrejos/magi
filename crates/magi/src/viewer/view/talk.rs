use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::markdown::{self, tool_name};
use super::{clip, spread, truncate};
use crate::proto::{Change, Said, Step, StepState};
use crate::viewer::model::TalkScroll;
use crate::viewer::style::{self, theme};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BAR_ROOM: u16 = 2;
const BUBBLE_SHARE: usize = 4;
const BUBBLE_OF: usize = 5;
const MAX_RUNNING_SHOWN: usize = 3;
const COMMANDS: [&str; 4] = ["Bash", "shell", "exec_command", "local_shell"];
const EDITS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"];
const READS: [&str; 2] = ["Read", "view_image"];
const SEARCHES: [&str; 2] = ["Grep", "Glob"];
const WEB: [&str; 2] = ["WebFetch", "WebSearch"];
const AGENTS: [&str; 2] = ["Agent", "Task"];

#[derive(Clone, Copy)]
pub struct Talk<'a> {
    pub said: &'a [Said],
    pub shown: &'a str,
    pub scroll: &'a TalkScroll,
    pub working: bool,
    pub expanded: bool,
    pub activity: Option<&'a str>,
    pub search: Option<(&'a str, usize)>,
}

#[derive(Clone, Copy)]
struct Tool<'a> {
    name: &'a str,
    detail: &'a str,
    change: Option<Change>,
    running_since: Option<u64>,
}

enum Piece<'a> {
    Prompt(&'a str),
    Reply(&'a str),
    Tools(Vec<Tool<'a>>),
    Plan(&'a [Step]),
}

fn pieces(said: &[Said]) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    for entry in said {
        match entry {
            Said::You(text) => pieces.push(Piece::Prompt(text)),
            Said::Reply(text) => pieces.push(Piece::Reply(text)),
            Said::Plan(steps) => pieces.push(Piece::Plan(steps)),
            Said::Tool {
                name,
                detail,
                change,
                running_since,
            } => {
                let tool = Tool {
                    name,
                    detail,
                    change: *change,
                    running_since: *running_since,
                };
                match pieces.last_mut() {
                    Some(Piece::Tools(tools)) => tools.push(tool),
                    _ => pieces.push(Piece::Tools(vec![tool])),
                }
            }
        }
    }
    pieces
}

pub fn render(frame: &mut Frame, area: Rect, talk: Talk<'_>) {
    if area.height == 0 || area.width < 8 {
        return;
    }
    let width = usize::from(area.width - BAR_ROOM);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let pieces = pieces(talk.said);
    let busy = talk.working
        && pieces.iter().any(|piece| match piece {
            Piece::Tools(tools) => tools.iter().any(|tool| tool.running_since.is_some()),
            _ => false,
        });
    let plan_last = matches!(pieces.last(), Some(Piece::Plan(_)));
    let thinking = talk.activity.filter(|_| talk.working && !busy);
    let count = pieces.len();
    let mut links = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if let Some(activity) = thinking.filter(|_| plan_last && index + 1 == count) {
            lines.push(Line::default());
            lines.push(running_line(activity, None, now, width));
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(match piece {
            Piece::Prompt(text) => prompt(text, width),
            Piece::Reply(text) => {
                let (lines, found) = markdown::markdown_links(text, width);
                links.extend(found);
                lines
            }
            Piece::Tools(tools) => tool_lines(&tools, talk, now, width),
            Piece::Plan(steps) => plan_lines(steps, width),
        });
    }
    if let Some(activity) = thinking.filter(|_| !plan_last) {
        lines.push(Line::default());
        lines.push(running_line(activity, None, now, width));
    }
    talk.scroll.set_links(links);
    let room = usize::from(area.height);
    let back = talk.scroll.place(talk.shown, lines.len(), room);
    let back = match talk.search.filter(|(query, _)| !query.is_empty()) {
        Some((query, index)) => {
            let found = highlight(&mut lines, query, index);
            talk.scroll.found(found.len());
            match found.len().checked_sub(1 + index % found.len().max(1)) {
                Some(at) => {
                    talk.scroll.reveal(found[at], lines.len(), room);
                    talk.scroll.place(talk.shown, lines.len(), room)
                }
                None => back,
            }
        }
        None => {
            talk.scroll.found(0);
            back
        }
    };
    let end = lines.len() - back;
    let start = end.saturating_sub(room);
    let top = area.y + (room - (end - start)) as u16;
    for (offset, line) in lines.drain(start..end).enumerate() {
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, top + offset as u16, area.width - BAR_ROOM, 1),
        );
    }
    let total = end + back;
    if total > room {
        let thumb = (room * room / total).max(1);
        let most = total - room;
        let at = (room - thumb) * (most - back.min(most)) / most.max(1);
        let x = area.x + area.width - 1;
        for row in at..at + thumb {
            frame.render_widget(
                Paragraph::new(Span::styled("┃", style::fg(theme().rule))),
                Rect::new(x, area.y + row as u16, 1, 1),
            );
        }
    }
}

fn highlight(lines: &mut [Line<'static>], query: &str, index: usize) -> Vec<usize> {
    let query = query.to_ascii_lowercase();
    let total: usize = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.to_ascii_lowercase().matches(&query).count())
                .sum::<usize>()
        })
        .sum();
    let current = total.checked_sub(1 + index % total.max(1));
    let t = theme();
    let mut found = Vec::new();
    let mut ordinal = 0;
    for (number, line) in lines.iter_mut().enumerate() {
        let mut spans = Vec::with_capacity(line.spans.len());
        for span in line.spans.drain(..) {
            let lower = span.content.to_ascii_lowercase();
            if !lower.contains(&query) {
                spans.push(span);
                continue;
            }
            let text = span.content.to_string();
            let mut at = 0;
            for (start, _) in lower.match_indices(&query) {
                if start > at {
                    spans.push(Span::styled(text[at..start].to_string(), span.style));
                }
                let end = start + query.len();
                let mark = if Some(ordinal) == current {
                    span.style.fg(t.panel).bg(t.warn)
                } else {
                    span.style.bg(t.warn_tint)
                };
                spans.push(Span::styled(text[start..end].to_string(), mark));
                found.push(number);
                ordinal += 1;
                at = end;
            }
            if at < text.len() {
                spans.push(Span::styled(text[at..].to_string(), span.style));
            }
        }
        line.spans = spans;
    }
    found
}

fn prompt(text: &str, width: usize) -> Vec<Line<'static>> {
    let t = theme();
    let inner = (width * BUBBLE_SHARE / BUBBLE_OF).saturating_sub(4).max(8);
    let rows: Vec<String> = text
        .trim()
        .lines()
        .flat_map(|line| wrap_words(line, inner))
        .collect();
    let widest = rows.iter().map(|row| row.width()).max().unwrap_or(0);
    let bubble = (widest + 4).min(width);
    let indent = " ".repeat(width - bubble);
    let edge = |glyph: &str| {
        Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(glyph.repeat(bubble), style::fg(t.raised)),
        ])
    };
    let mut lines = vec![edge("▄")];
    for row in rows {
        let pad = bubble.saturating_sub(row.width() + 2);
        lines.push(Line::from(vec![
            Span::raw(indent.clone()),
            Span::styled(
                format!("  {row}{}", " ".repeat(pad)),
                style::fg(t.text).bg(t.raised),
            ),
        ]));
    }
    lines.push(edge("▀"));
    lines
}

fn wrap_words(line: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    for word in line.split_whitespace() {
        let row = rows.last_mut().expect("rows starts with one row");
        if !row.is_empty() && row.width() + 1 + word.width() > width {
            rows.push(truncate(word, width));
        } else {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(&truncate(word, width));
        }
    }
    rows
}

fn tool_lines(tools: &[Tool<'_>], talk: Talk<'_>, now: u64, width: usize) -> Vec<Line<'static>> {
    let t = theme();
    let running = |tool: &Tool<'_>| tool.running_since.filter(|_| talk.working);
    let spinner = SPINNER[(now % SPINNER.len() as u64) as usize];
    if talk.expanded {
        return tools
            .iter()
            .map(|tool| {
                let mut spans = vec![
                    match running(tool) {
                        Some(_) => Span::styled(format!("{spinner} "), style::fg(t.claude)),
                        None => Span::styled("· ", style::fg(t.faint)),
                    },
                    Span::styled(tool_name(tool.name), style::fg(t.dim)),
                ];
                if !tool.detail.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", tool.detail),
                        style::fg(t.faint),
                    ));
                }
                let mut right = counts(tool.change);
                if let Some(since) = running(tool) {
                    right.push(Span::styled(
                        format!("  {}", elapsed(now.saturating_sub(since))),
                        style::fg(t.faint),
                    ));
                }
                spread(spans, right, width)
            })
            .collect();
    }
    let (busy, done): (Vec<Tool<'_>>, Vec<Tool<'_>>) =
        tools.iter().partition(|tool| running(tool).is_some());
    let mut lines = Vec::new();
    if !done.is_empty() {
        let total = done.iter().filter_map(|tool| tool.change).fold(
            Change {
                added: 0,
                removed: 0,
            },
            |sum, change| Change {
                added: sum.added + change.added,
                removed: sum.removed + change.removed,
            },
        );
        let mut spans = vec![Span::styled(summary(&done), style::fg(t.faint))];
        spans.extend(counts(Some(total)));
        spans.push(Span::styled("  ›", style::fg(t.faint)));
        lines.push(Line::from(clip(spans, width)));
    }
    let shown: Vec<(String, Option<u64>)> = match (talk.activity, busy.first()) {
        (Some(activity), Some(first)) => vec![(activity.to_string(), first.running_since)],
        _ => busy
            .iter()
            .take(MAX_RUNNING_SHOWN)
            .map(|tool| (doing(tool), tool.running_since))
            .collect(),
    };
    for (what, since) in shown {
        lines.push(running_line(&what, since, now, width));
    }
    lines
}

fn running_line(what: &str, since: Option<u64>, now: u64, width: usize) -> Line<'static> {
    let t = theme();
    let spinner = SPINNER[(now % SPINNER.len() as u64) as usize];
    let mut spans = vec![
        Span::styled(format!("{spinner} "), style::fg(t.claude)),
        Span::styled(what.to_string(), style::fg(t.dim)),
    ];
    if let Some(since) = since {
        spans.push(Span::styled(
            format!("  {}", elapsed(now.saturating_sub(since))),
            style::fg(t.faint),
        ));
    }
    Line::from(clip(spans, width))
}

fn counts(change: Option<Change>) -> Vec<Span<'static>> {
    let t = theme();
    let mut spans = Vec::new();
    if let Some(Change { added, removed }) = change {
        if added > 0 {
            spans.push(Span::styled(format!(" +{added}"), style::fg(t.accent)));
        }
        if removed > 0 {
            spans.push(Span::styled(format!(" −{removed}"), style::fg(t.crit)));
        }
    }
    spans
}

fn file_name(detail: &str) -> &str {
    detail.rsplit('/').next().unwrap_or(detail)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        format!("{count} {many}")
    }
}

fn summary(tools: &[Tool<'_>]) -> String {
    let of = |kinds: &[&str]| -> Vec<&Tool<'_>> {
        tools
            .iter()
            .filter(|tool| kinds.contains(&tool.name))
            .collect()
    };
    let mut parts = Vec::new();
    let commands = of(&COMMANDS).len();
    if commands > 0 {
        parts.push(format!("ran {}", plural(commands, "a command", "commands")));
    }
    let mut edited: Vec<&str> = of(&EDITS)
        .iter()
        .map(|tool| file_name(tool.detail))
        .collect();
    edited.sort_unstable();
    edited.dedup();
    match edited.as_slice() {
        [] => {}
        [one] => parts.push(format!("edited {one}")),
        [one, two] => parts.push(format!("edited {one} and {two}")),
        many => parts.push(format!("edited {} files", many.len())),
    }
    let reads = of(&READS);
    match reads.as_slice() {
        [] => {}
        [one] => parts.push(format!("read {}", file_name(one.detail))),
        many => parts.push(format!("read {} files", many.len())),
    }
    let searches = of(&SEARCHES).len();
    if searches > 0 {
        parts.push(format!(
            "searched {}",
            plural(searches, "the code", "times")
        ));
    }
    let web = of(&WEB).len();
    if web > 0 {
        parts.push(format!("checked {}", plural(web, "the web", "web pages")));
    }
    let agents = of(&AGENTS).len();
    if agents > 0 {
        parts.push(format!("ran {}", plural(agents, "an agent", "agents")));
    }
    let known: Vec<&str> = [&COMMANDS[..], &EDITS, &READS, &SEARCHES, &WEB, &AGENTS].concat();
    let mut others: Vec<String> = tools
        .iter()
        .filter(|tool| !known.contains(&tool.name))
        .map(|tool| tool_name(tool.name))
        .collect();
    others.dedup();
    if !others.is_empty() {
        parts.push(format!("used {}", others.join(", ")));
    }
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

fn doing(tool: &Tool<'_>) -> String {
    let file = file_name(tool.detail);
    if COMMANDS.contains(&tool.name) {
        "Running a command".into()
    } else if EDITS.contains(&tool.name) {
        format!("Editing {file}")
    } else if READS.contains(&tool.name) {
        format!("Reading {file}")
    } else if SEARCHES.contains(&tool.name) {
        "Searching the code".into()
    } else if WEB.contains(&tool.name) {
        "Checking the web".into()
    } else if AGENTS.contains(&tool.name) {
        "Running an agent".into()
    } else {
        format!("Using {}", tool_name(tool.name))
    }
}

fn elapsed(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

fn plan_lines(steps: &[Step], width: usize) -> Vec<Line<'static>> {
    let t = theme();
    let done = steps
        .iter()
        .filter(|step| step.state == StepState::Done)
        .count();
    let fill = Style::new().bg(t.raised);
    let row = |spans: Vec<Span<'static>>| {
        let spans: Vec<Span<'static>> = spans
            .into_iter()
            .map(|span| {
                let style = span.style.bg(t.raised);
                span.style(style)
            })
            .collect();
        spread(spans, Vec::new(), width).style(fill)
    };
    let edge = |glyph: &str| Line::from(Span::styled(glyph.repeat(width), style::fg(t.raised)));
    let mut lines = vec![
        edge("▄"),
        row(vec![Span::styled(
            format!("  Plan · {done} of {} done", steps.len()),
            style::fg(t.faint),
        )]),
    ];
    for step in steps {
        let (mark, mark_color, text_color) = match step.state {
            StepState::Done => ("✓", t.accent, t.faint),
            StepState::Doing => ("●", t.claude, t.text),
            StepState::Todo => ("○", t.faint, t.dim),
        };
        lines.push(row(clip(
            vec![
                Span::styled(format!("  {mark} "), style::fg(mark_color)),
                Span::styled(step.text.clone(), style::fg(text_color)),
            ],
            width.saturating_sub(1),
        )));
    }
    lines.push(edge("▀"));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, detail: &str, running_since: Option<u64>) -> Said {
        Said::Tool {
            name: name.into(),
            detail: detail.into(),
            change: None,
            running_since,
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

    fn tools_of(said: &[Said]) -> Vec<Tool<'_>> {
        match pieces(said).into_iter().next() {
            Some(Piece::Tools(tools)) => tools,
            _ => panic!("no tools"),
        }
    }

    #[test]
    fn a_run_of_tool_calls_reads_as_one_quiet_line_and_the_running_one_counts_its_time() {
        let said = [
            tool("Bash", "cargo test", None),
            tool("Bash", "cargo build", None),
            tool("Edit", "src/machines.rs", None),
            tool("Read", "src/main.rs", None),
            tool("Bash", "sh install.sh", Some(1_000)),
        ];
        let scroll = TalkScroll::default();
        let talk = Talk {
            said: &said,
            shown: "s",
            scroll: &scroll,
            working: true,
            expanded: false,
            activity: None,
            search: None,
        };
        let lines = texts(&tool_lines(&tools_of(&said), talk, 1_205, 80));
        assert_eq!(
            lines[0],
            "Ran 2 commands, edited machines.rs, read main.rs  ›"
        );
        assert!(lines[1].ends_with("Running a command  3m 25s"), "{lines:?}");
        let idle = Talk {
            working: false,
            ..talk
        };
        let lines = texts(&tool_lines(&tools_of(&said), idle, 1_205, 80));
        assert_eq!(
            lines,
            ["Ran 3 commands, edited machines.rs, read main.rs  ›"]
        );
        let open = Talk {
            expanded: true,
            ..talk
        };
        let lines = texts(&tool_lines(&tools_of(&said), open, 1_205, 80));
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("· Bash  cargo test"), "{lines:?}");
    }

    fn screen(said: &[Said], shown: &str, scroll: &TalkScroll) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 6)).unwrap();
        terminal
            .draw(|frame| {
                let talk = Talk {
                    said,
                    shown,
                    scroll,
                    working: false,
                    expanded: false,
                    activity: None,
                    search: None,
                };
                render(frame, frame.area(), talk);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..6)
            .map(|y| (0..30).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn scrolling_back_holds_its_place_as_output_arrives_and_another_session_starts_at_the_end() {
        let mut said: Vec<Said> = (0..10).map(|n| Said::Reply(format!("reply {n}"))).collect();
        let scroll = TalkScroll::default();
        let rows = screen(&said, "a", &scroll);
        assert!(rows[5].starts_with("reply 9"), "{rows:?}");
        assert!(rows.iter().any(|row| row.ends_with('┃')), "{rows:?}");
        scroll.scroll(4);
        let rows = screen(&said, "a", &scroll);
        assert!(rows[5].starts_with("reply 7"), "{rows:?}");
        said.push(Said::Reply("reply 10".into()));
        let rows = screen(&said, "a", &scroll);
        assert!(rows[5].starts_with("reply 7"), "{rows:?}");
        scroll.scroll(1_000);
        let rows = screen(&said, "a", &scroll);
        assert!(rows[0].starts_with("reply 0"), "{rows:?}");
        let rows = screen(&said, "b", &scroll);
        assert!(rows[5].starts_with("reply 10"), "{rows:?}");
    }

    #[test]
    fn the_agents_own_activity_replaces_the_generic_running_line() {
        let said = [
            tool("Bash", "cargo test", None),
            tool("Bash", "cargo build", Some(1_000)),
        ];
        let scroll = TalkScroll::default();
        let talk = Talk {
            said: &said,
            shown: "s",
            scroll: &scroll,
            working: true,
            expanded: false,
            activity: Some("building the release"),
            search: None,
        };
        let lines = texts(&tool_lines(&tools_of(&said), talk, 1_065, 80));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[1].ends_with("building the release  1m 5s"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_search_highlights_its_matches_counts_them_and_brings_the_current_one_into_view() {
        let said: Vec<Said> = (0..30)
            .map(|n| {
                Said::Reply(format!(
                    "reply {n} {}",
                    if n % 10 == 3 { "Needle" } else { "hay" }
                ))
            })
            .collect();
        let scroll = TalkScroll::default();
        let draw = |index: usize| -> (Vec<String>, ratatui::style::Style) {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 6)).unwrap();
            terminal
                .draw(|frame| {
                    let talk = Talk {
                        said: &said,
                        shown: "s",
                        scroll: &scroll,
                        working: false,
                        expanded: false,
                        activity: None,
                        search: Some(("needle", index)),
                    };
                    render(frame, frame.area(), talk);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let rows = (0..6)
                .map(|y| (0..30).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            let marked = (0..6)
                .flat_map(|y| (0..30).map(move |x| (x, y)))
                .find(|(x, y)| buffer[(*x, *y)].bg == theme().warn)
                .map(|(x, y)| buffer[(x, y)].style())
                .unwrap_or_default();
            (rows, marked)
        };
        let (rows, marked) = draw(0);
        assert_eq!(scroll.matches(), 3);
        assert!(
            rows.iter().any(|row| row.starts_with("reply 23 Needle")),
            "{rows:?}"
        );
        assert_eq!(marked.bg, Some(theme().warn));
        let (rows, _) = draw(2);
        assert!(
            rows.iter().any(|row| row.starts_with("reply 3 Needle")),
            "{rows:?}"
        );
    }

    #[test]
    fn your_message_sits_in_a_raised_block_on_the_right() {
        let lines = prompt("fix the build", 40);
        let rows = texts(&lines);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].trim_end().ends_with("fix the build"), "{rows:?}");
        assert!(rows[1].starts_with("                       "), "{rows:?}");
        assert_eq!(lines[1].spans[1].style.bg, Some(theme().raised));
    }

    #[test]
    fn the_plan_counts_what_is_done_and_marks_each_step() {
        let steps = [
            Step {
                text: "Find how macOS orders cores".into(),
                state: StepState::Done,
            },
            Step {
                text: "Report core kinds".into(),
                state: StepState::Doing,
            },
            Step {
                text: "Group the section".into(),
                state: StepState::Todo,
            },
        ];
        let rows = texts(&plan_lines(&steps, 50));
        assert!(rows[1].starts_with("  Plan · 1 of 3 done"), "{rows:?}");
        assert!(rows[2].starts_with("  ✓ Find"), "{rows:?}");
        assert!(rows[3].starts_with("  ● Report"), "{rows:?}");
        assert!(rows[4].starts_with("  ○ Group"), "{rows:?}");
    }
}
