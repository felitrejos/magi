use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::clip;
use crate::viewer::style::{self, Theme, theme};

const CELL_GAP: usize = 2;
const NARROWEST_COLUMN: usize = 3;
const TAB: &str = "    ";

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

type Paint = fn(&Theme) -> Color;

static PAINTS: LazyLock<Vec<(Scope, Paint)>> = LazyLock::new(|| {
    let paints: [(&str, Paint); 22] = [
        ("comment", |t| t.faint),
        ("punctuation.definition.comment", |t| t.faint),
        ("string", |t| t.accent),
        ("constant.character.escape", |t| t.second),
        ("constant.numeric", |t| t.crit),
        ("constant.language", |t| t.crit),
        ("constant.other", |t| t.crit),
        ("keyword.operator", |t| t.text),
        ("keyword", |t| t.second),
        ("storage.type", |t| t.second),
        ("storage.modifier", |t| t.second),
        ("entity.name.function", |t| t.codex),
        ("support.function", |t| t.codex),
        ("variable.function", |t| t.codex),
        ("meta.function-call", |t| t.codex),
        ("entity.name.type", |t| t.warn),
        ("entity.name.class", |t| t.warn),
        ("entity.name.struct", |t| t.warn),
        ("support.type", |t| t.warn),
        ("support.class", |t| t.warn),
        ("entity.name.tag", |t| t.crit),
        ("entity.other.attribute-name", |t| t.warn),
    ];
    paints
        .into_iter()
        .filter_map(|(name, paint)| Some((Scope::new(name).ok()?, paint)))
        .collect()
});

pub(super) fn tool_name(name: &str) -> String {
    match name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    {
        Some((_, tool)) if !tool.is_empty() => tool.replace('_', " "),
        _ => name.to_string(),
    }
}

pub fn markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut writer = Writer::new(width);
    for event in Parser::new_ext(text, options) {
        writer.event(event);
    }
    writer.flush();
    writer.lines
}

enum Nest {
    Quote,
    Item {
        marker: Option<String>,
        width: usize,
    },
    TooDeep,
}

impl Nest {
    fn width(&self) -> usize {
        match self {
            Nest::Quote => 2,
            Nest::Item { width, .. } => *width,
            Nest::TooDeep => 0,
        }
    }
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<Vec<Span<'static>>>>,
    row: Vec<Vec<Span<'static>>>,
    cell: Vec<Span<'static>>,
    has_head: bool,
}

struct Code {
    language: String,
    body: String,
}

struct Writer {
    width: usize,
    lines: Vec<Line<'static>>,
    inline: Vec<Span<'static>>,
    styles: Vec<Style>,
    nest: Vec<Nest>,
    lists: Vec<Option<u64>>,
    gap: bool,
    code: Option<Code>,
    table: Option<Table>,
}

impl Writer {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
            inline: Vec::new(),
            styles: vec![style::fg(theme().prose)],
            nest: Vec::new(),
            lists: Vec::new(),
            gap: false,
            code: None,
            table: None,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        let t = theme();
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text, self.style()),
            Event::Code(text) | Event::InlineMath(text) => {
                self.push(text.to_string(), self.style().fg(t.second));
            }
            Event::DisplayMath(text) => self.text(&text, self.style().fg(t.second)),
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text, style::fg(t.faint)),
            Event::FootnoteReference(name) => self.push(format!("[^{name}]"), style::fg(t.faint)),
            Event::SoftBreak => self.push(" ".to_string(), self.style()),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.begin_block();
                let width = self.width.saturating_sub(self.prefix_width());
                let rule = Span::styled("─".repeat(width), style::fg(t.rule));
                let mut spans = self.prefix(true);
                spans.push(rule);
                self.lines.push(Line::from(spans));
                self.gap = true;
            }
            Event::TaskListMarker(done) => {
                let (mark, color) = if done {
                    ("✓ ", t.accent)
                } else {
                    ("○ ", t.faint)
                };
                self.push(mark.to_string(), style::fg(color));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        let t = theme();
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.begin_block(),
            Tag::Heading { level, .. } => {
                self.begin_block();
                let mut heading = self.style().add_modifier(Modifier::BOLD);
                if level == HeadingLevel::H1 {
                    heading = heading.add_modifier(Modifier::UNDERLINED);
                }
                self.styles.push(heading);
            }
            Tag::BlockQuote(_) => {
                self.begin_block();
                self.nest_into(Nest::Quote);
                self.styles
                    .push(self.style().fg(t.dim).add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.begin_block();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some(Code {
                    language,
                    body: String::new(),
                });
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.begin_block();
                } else {
                    self.flush();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "• ".to_string(),
                };
                let width = marker.width();
                self.nest_into(Nest::Item {
                    marker: Some(marker),
                    width,
                });
            }
            Tag::Table(_) => {
                self.begin_block();
                self.table = Some(Table::default());
            }
            Tag::TableHead | Tag::TableRow | Tag::TableCell => {}
            Tag::Emphasis => self
                .styles
                .push(self.style().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(self.style().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.styles
                    .push(self.style().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } | Tag::Image { .. } => {
                self.styles
                    .push(self.style().fg(t.codex).add_modifier(Modifier::UNDERLINED));
            }
            _ => self.styles.push(self.style()),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => {
                self.flush();
                self.gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.nest.pop();
                self.styles.pop();
                self.gap = true;
            }
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.code_block(&code);
                }
                self.gap = true;
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                self.gap = self.lists.is_empty();
            }
            TagEnd::Item => {
                self.flush();
                if let Some(Nest::Item {
                    marker: Some(_), ..
                }) = self.nest.last()
                {
                    let marker = self.prefix(true);
                    self.lines.push(Line::from(marker));
                }
                self.nest.pop();
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    table.has_head |= tag == TagEnd::TableHead;
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table_block(table);
                }
                self.gap = true;
            }
            _ => {
                self.styles.pop();
            }
        }
    }

    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn text(&mut self, text: &str, style: Style) {
        if let Some(code) = &mut self.code {
            code.body.push_str(text);
            return;
        }
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.flush();
            }
            if !part.is_empty() {
                self.push(part.to_string(), style);
            }
        }
    }

    fn push(&mut self, text: String, style: Style) {
        let span = Span::styled(text.replace('\t', TAB), style);
        match &mut self.table {
            Some(table) => table.cell.push(span),
            None => self.inline.push(span),
        }
    }

    fn begin_block(&mut self) {
        self.flush();
        if self.gap && !self.lines.is_empty() {
            let bars = self.prefix(false);
            self.lines.push(Line::from(bars));
        }
        self.gap = false;
    }

    fn flush(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        let first = self.prefix(true);
        let rest = self.prefix(false);
        self.lines.extend(wrap(spans, self.width, &first, &rest));
    }

    fn nest_into(&mut self, nest: Nest) {
        let fits = self.prefix_width() + nest.width() <= self.width / 2;
        self.nest.push(if fits { nest } else { Nest::TooDeep });
    }

    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let t = theme();
        let mut spans = Vec::new();
        for nest in &mut self.nest {
            match nest {
                Nest::Quote => spans.push(Span::styled("│ ", style::fg(t.rule))),
                Nest::Item { marker, width } => {
                    let marker = if first { marker.take() } else { None };
                    spans.push(match marker {
                        Some(marker) => Span::styled(marker, style::fg(t.dim)),
                        None => Span::raw(" ".repeat(*width)),
                    });
                }
                Nest::TooDeep => {}
            }
        }
        spans
    }

    fn prefix_width(&self) -> usize {
        self.nest.iter().map(Nest::width).sum()
    }

    fn code_block(&mut self, code: &Code) {
        let room = self.width.saturating_sub(self.prefix_width() + 2);
        for (index, spans) in highlight(&code.body, &code.language)
            .into_iter()
            .enumerate()
        {
            let mut line = self.prefix(index == 0);
            line.push(Span::raw("  "));
            line.extend(clip(spans, room));
            self.lines.push(Line::from(line));
        }
    }

    fn table_block(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let cell_width = |cell: &[Span<'_>]| cell.iter().map(Span::width).sum::<usize>();
        let mut widths = vec![0; columns];
        for row in &table.rows {
            for (column, cell) in row.iter().enumerate() {
                widths[column] = widths[column].max(cell_width(cell));
            }
        }
        let room = self
            .width
            .saturating_sub(self.prefix_width() + CELL_GAP * (columns - 1));
        while widths.iter().sum::<usize>() > room {
            let Some(widest) = widths
                .iter_mut()
                .max()
                .filter(|width| **width > NARROWEST_COLUMN)
            else {
                break;
            };
            *widest -= 1;
        }
        let t = theme();
        for (index, row) in table.rows.into_iter().enumerate() {
            let head = table.has_head && index == 0;
            let mut line = self.prefix(index == 0);
            for (column, width) in widths.iter().enumerate() {
                let cell = row.get(column).cloned().unwrap_or_default();
                let cell: Vec<Span<'static>> = if head {
                    cell.into_iter()
                        .map(|span| span.patch_style(Style::new().add_modifier(Modifier::BOLD)))
                        .collect()
                } else {
                    cell
                };
                let cell = clip(cell, *width);
                let used = cell_width(&cell);
                line.extend(cell);
                if column + 1 < columns {
                    line.push(Span::raw(" ".repeat(width - used + CELL_GAP)));
                }
            }
            self.lines.push(Line::from(line));
            if head {
                let mut rule = self.prefix(false);
                let total = widths.iter().sum::<usize>() + CELL_GAP * (columns - 1);
                rule.push(Span::styled("─".repeat(total), style::fg(t.rule)));
                self.lines.push(Line::from(rule));
            }
        }
    }
}

fn highlight(code: &str, language: &str) -> Vec<Vec<Span<'static>>> {
    let t = theme();
    let plain = style::fg(t.text);
    let syntaxes = &*SYNTAXES;
    let syntax = Some(language)
        .filter(|language| !language.is_empty())
        .and_then(|language| syntaxes.find_syntax_by_token(language))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut state = Some(ParseState::new(syntax));
    let mut stack = ScopeStack::new();
    let mut lines = Vec::new();
    for line in code.split_inclusive('\n') {
        let ops = state.as_mut().map(|state| state.parse_line(line, syntaxes));
        let mut spans = Vec::new();
        let piece = |text: &str, style: Style, spans: &mut Vec<Span<'static>>| {
            let text = text.trim_end_matches(['\n', '\r']).replace('\t', TAB);
            if !text.is_empty() {
                spans.push(Span::styled(text, style));
            }
        };
        match ops {
            Some(Ok(ops)) => {
                let mut at = 0;
                for (until, op) in ops {
                    piece(&line[at..until], paint(&stack, &t, plain), &mut spans);
                    if stack.apply(&op).is_err() {
                        state = None;
                    }
                    at = until;
                }
                piece(&line[at..], paint(&stack, &t, plain), &mut spans);
            }
            Some(Err(_)) | None => {
                state = None;
                piece(line, plain, &mut spans);
            }
        }
        lines.push(spans);
    }
    lines
}

fn paint(stack: &ScopeStack, t: &Theme, plain: Style) -> Style {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|scope| {
            PAINTS
                .iter()
                .find(|(selector, _)| selector.is_prefix_of(*scope))
                .map(|(_, paint)| style::fg(paint(t)))
        })
        .unwrap_or(plain)
}

fn wrap(
    spans: Vec<Span<'static>>,
    width: usize,
    first: &[Span<'static>],
    rest: &[Span<'static>],
) -> Vec<Line<'static>> {
    let indent: usize = first.iter().map(Span::width).sum();
    let width = width.saturating_sub(indent).max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        for word in span.content.split_inclusive(' ') {
            let word_width = word.trim_end().width();
            if used > 0 && used + word_width > width {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            let mut left = word;
            while used == 0 && left.trim_end().width() > width {
                let cut = split_at_width(left, width);
                row.push(Span::styled(left[..cut].to_string(), span.style));
                rows.push(std::mem::take(&mut row));
                left = &left[cut..];
            }
            if !(used == 0 && left.trim().is_empty() && !rows.is_empty()) {
                row.push(Span::styled(left.to_string(), span.style));
                used += left.width();
            }
        }
    }
    rows.push(row);
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let mut line = if index == 0 {
                first.to_vec()
            } else {
                rest.to_vec()
            };
            line.extend(row);
            Line::from(line)
        })
        .collect()
}

fn split_at_width(text: &str, width: usize) -> usize {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += c.width().unwrap_or(0);
        if used > width {
            return index.max(c.len_utf8());
        }
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn styled<'a>(lines: &'a [Line<'_>], text: &str) -> &'a Span<'a> {
        lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(text))
            .unwrap_or_else(|| panic!("no span holds {text:?} in {:?}", texts(lines)))
    }

    #[test]
    fn text_wraps_between_words_and_only_splits_words_wider_than_the_panel() {
        let plain = |text: &str| vec![Span::raw(text.to_string())];
        assert_eq!(
            texts(&wrap(plain("the agent side builds now"), 12, &[], &[])),
            ["the agent ", "side builds ", "now"]
        );
        assert_eq!(
            texts(&wrap(plain("ab abcdefghijkl"), 5, &[], &[])),
            ["ab ", "abcde", "fghij", "kl"]
        );
        assert_eq!(
            texts(&wrap(
                plain("one two three"),
                9,
                &[Span::raw("• ")],
                &[Span::raw("  ")]
            )),
            ["• one two ", "  three"]
        );
    }

    #[test]
    fn inline_markup_becomes_style_not_symbols() {
        let lines = markdown(
            "Run **cargo test** then *read* `out.txt` and ~~skip~~ it.",
            80,
        );
        assert_eq!(
            texts(&lines),
            ["Run cargo test then read out.txt and skip it."]
        );
        assert!(
            styled(&lines, "cargo")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            styled(&lines, "read")
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert_eq!(styled(&lines, "out.txt").style.fg, Some(theme().second));
        assert!(
            styled(&lines, "skip")
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn blocks_are_separated_by_one_blank_line_and_lists_keep_their_indent() {
        let text = "# Plan\n\nFirst do this:\n\n- build the crate\n- run the tests across every target we ship\n  1. mac\n  2. linux\n\nDone.";
        assert_eq!(
            texts(&markdown(text, 30)),
            [
                "Plan",
                "",
                "First do this:",
                "",
                "• build the crate",
                "• run the tests across every ",
                "  target we ship",
                "  1. mac",
                "  2. linux",
                "",
                "Done.",
            ]
        );
    }

    #[test]
    fn quotes_and_rules_draw_their_own_marks() {
        assert_eq!(
            texts(&markdown("> careful here\n\n---\n\nafter", 10)),
            ["│ careful ", "│ here", "", "──────────", "", "after"]
        );
    }

    #[test]
    fn blank_lines_inside_a_quote_keep_the_bar_and_empty_items_keep_their_marker() {
        assert_eq!(texts(&markdown("> a\n>\n> b", 20)), ["│ a", "│ ", "│ b"]);
        assert_eq!(texts(&markdown("- \n- b", 20)), ["• ", "• b"]);
    }

    #[test]
    fn deep_nesting_stops_indenting_at_half_the_panel() {
        let text = format!("{} deep", ">".repeat(3000));
        let lines = markdown(&text, 20);
        assert_eq!(texts(&lines), ["│ │ │ │ │ deep"]);
        assert!(lines[0].spans.len() <= 6);
    }

    #[test]
    fn code_blocks_are_indented_highlighted_and_never_wrapped() {
        let lines = markdown(
            "```rust\nfn main() { let x = 42; } // a long trailing comment\n```",
            30,
        );
        assert_eq!(lines.len(), 1);
        let text = &texts(&lines)[0];
        assert!(text.starts_with("  fn main()"), "{text}");
        assert!(text.width() <= 30, "{text}");
        assert_eq!(styled(&lines, "fn").style.fg, Some(theme().second));
        assert_eq!(styled(&lines, "main").style.fg, Some(theme().codex));
        assert_eq!(styled(&lines, "42").style.fg, Some(theme().crit));
    }

    #[test]
    fn unknown_languages_are_shown_plain() {
        let lines = markdown("```nothing-like-this\nkeep me\n```", 30);
        assert_eq!(texts(&lines), ["  keep me"]);
        assert_eq!(styled(&lines, "keep me").style.fg, Some(theme().text));
    }

    #[test]
    fn tables_line_up_their_columns_and_shrink_to_fit() {
        let text =
            "| Machine | Load |\n|---|---|\n| homelab | idle |\n| mac | busy with a long build |";
        assert_eq!(
            texts(&markdown(text, 80)),
            [
                "Machine  Load".to_string(),
                "─".repeat(31),
                "homelab  idle".to_string(),
                "mac      busy with a long build".to_string(),
            ]
        );
        for line in texts(&markdown(text, 20)) {
            assert!(line.width() <= 20, "{line:?}");
        }
    }

    #[test]
    fn mcp_tools_show_by_their_own_name() {
        assert_eq!(tool_name("mcp__ccd_turn__update_plan"), "update plan");
        assert_eq!(
            tool_name("mcp__plugin_cielago_cielago__run_elsewhere"),
            "run elsewhere"
        );
        assert_eq!(tool_name("mcp__broken"), "mcp__broken");
        assert_eq!(tool_name("Edit"), "Edit");
    }
}
