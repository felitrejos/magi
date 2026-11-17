use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{Cursor, body, clip, draw_panel, panel, spread, tail};
use crate::proto::Agent;
use crate::viewer::launcher::{self, Launcher, Listing};
use crate::viewer::style::{self, theme};

const FOOT: u16 = 7;
const LABEL: usize = 9;

pub fn float(frame: &mut Frame, launcher: Option<&Launcher>) {
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme().panel));
    render(
        frame,
        area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        }),
        launcher,
    );
}

pub fn render(frame: &mut Frame, area: Rect, launcher: Option<&Launcher>) {
    if area.height < FOOT + 5 || area.width < 24 {
        return;
    }
    draw_panel(
        frame,
        area,
        panel(
            vec![Span::styled("New session", style::bold(theme().text))],
            None,
        ),
    );
    let inner = body(area);
    let width = inner.width as usize;
    let mut cursor = Cursor::new(inner);
    let Some(launcher) = launcher else {
        cursor.line(frame, faint("asking your machines for their projects…"));
        return;
    };
    cursor.line(frame, filter_line(launcher, width));
    cursor.skip();
    let list = Rect::new(
        inner.x,
        cursor.y,
        inner.width,
        inner.height.saturating_sub(2 + FOOT),
    );
    draw_projects(frame, list, launcher);
    let mut cursor = Cursor::new(Rect::new(
        inner.x,
        inner.y + inner.height - FOOT,
        inner.width,
        FOOT,
    ));
    cursor.line(
        frame,
        Line::from(Span::styled("─".repeat(width), style::fg(theme().rule))),
    );
    cursor.line(frame, machine_line(launcher, width));
    cursor.line(frame, folder_line(launcher, width));
    cursor.line(frame, agent_line(launcher, width));
    cursor.line(frame, model_line(launcher, width));
    cursor.skip();
    cursor.line(frame, keys_line(width));
}

fn faint(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), style::fg(theme().faint)))
}

fn label(text: &str) -> Span<'static> {
    Span::styled(format!("{text:<LABEL$}"), style::fg(theme().dim))
}

fn pill(text: &str, chosen: bool, color: ratatui::style::Color) -> Span<'static> {
    if chosen {
        Span::styled(format!(" {text} "), style::bold(color).bg(theme().selected))
    } else {
        Span::styled(format!(" {text} "), style::fg(theme().dim))
    }
}

fn filter_line(launcher: &Launcher, width: usize) -> Line<'static> {
    let typed = if launcher.filter.is_empty() {
        Span::styled("type to find a project", style::fg(theme().faint))
    } else {
        Span::styled(
            tail(&launcher.filter, width.saturating_sub(4)),
            style::fg(theme().text),
        )
    };
    Line::from(vec![
        Span::styled("› ", style::fg(theme().accent)),
        typed,
        Span::styled("▏", style::fg(theme().accent)),
    ])
}

fn draw_projects(frame: &mut Frame, area: Rect, launcher: &Launcher) {
    let width = area.width as usize;
    let notes: Vec<Line<'static>> = launcher
        .menu
        .machines
        .iter()
        .filter_map(|place| match &place.listing {
            Listing::Asking => Some(faint(&format!("asking {} for its projects…", place.tag))),
            Listing::Failed(reason) => Some(Line::from(clip(
                vec![Span::styled(
                    format!("{}: {reason}", place.tag),
                    style::fg(theme().warn),
                )],
                width,
            ))),
            Listing::Listed => None,
        })
        .collect();
    let room = (area.height as usize).saturating_sub(notes.len() + 1);
    let shown = launcher.shown();
    let mut cursor = Cursor::new(area);
    if shown.is_empty() && !launcher.menu.projects.is_empty() {
        cursor.line(frame, faint("No project matches."));
    }
    let selected = launcher.selected.min(shown.len().saturating_sub(1));
    let first = selected.saturating_sub(room.saturating_sub(1));
    for (index, project) in shown.iter().enumerate().skip(first).take(room) {
        let chosen = index == selected;
        let mut left = vec![
            Span::styled(if chosen { "▎" } else { " " }, style::fg(theme().accent)),
            Span::styled(
                project.name.clone(),
                if chosen {
                    style::bold(theme().text)
                } else {
                    style::fg(theme().text)
                },
            ),
        ];
        if let Some(group) = &project.group {
            left.push(Span::styled(format!("  {group}"), style::fg(theme().faint)));
        }
        if project.active {
            left.push(Span::styled("  active", style::fg(theme().accent)));
        }
        let machines = project
            .folders
            .iter()
            .map(|folder| launcher.menu.machines[folder.machine].tag.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        let right = vec![Span::styled(machines, style::fg(theme().faint))];
        let mut line = spread(clip(left, width / 2), right, width);
        if chosen {
            line = line.style(Style::new().bg(theme().selected));
        }
        cursor.line(frame, line);
    }
    let hidden = launcher.hidden();
    if hidden > 0 {
        cursor.line(
            frame,
            faint(&format!(
                "  {hidden} more · type to find {}",
                if hidden == 1 { "it" } else { "them" }
            )),
        );
    }
    for note in notes {
        cursor.line(frame, note);
    }
}

fn machine_line(launcher: &Launcher, width: usize) -> Line<'static> {
    let mut spans = vec![label("Machine")];
    let Some(project) = launcher.current() else {
        spans.push(Span::styled("—", style::fg(theme().faint)));
        return Line::from(spans);
    };
    let at = launcher.folder();
    for (index, folder) in project.folders.iter().enumerate() {
        let place = &launcher.menu.machines[folder.machine];
        spans.push(pill(&place.tag, Some(index) == at, theme().text));
    }
    if let Some(place) = launcher.place() {
        spans.push(Span::styled(
            format!("  {}", launcher::load(place)),
            style::fg(theme().faint),
        ));
    }
    if at.is_some() && at == launcher.menu.suggested(project) {
        spans.push(Span::styled(" · suggested", style::fg(theme().accent)));
    }
    Line::from(clip(spans, width))
}

fn folder_line(launcher: &Launcher, width: usize) -> Line<'static> {
    let dir = launcher
        .choice()
        .map(|choice| launcher::home_relative(&choice.dir))
        .unwrap_or_default();
    Line::from(clip(
        vec![
            label("Folder"),
            Span::styled(format!(" {dir}"), style::fg(theme().faint)),
        ],
        width,
    ))
}

fn agent_line(launcher: &Launcher, width: usize) -> Line<'static> {
    let codex = launcher.pick().codex;
    let (_, claude) = style::agent(Agent::ClaudeCode);
    let (_, codex_color) = style::agent(Agent::Codex);
    Line::from(clip(
        vec![
            label("Agent"),
            pill("Claude", !codex, claude),
            pill("Codex", codex, codex_color),
        ],
        width,
    ))
}

fn model_line(launcher: &Launcher, width: usize) -> Line<'static> {
    let chosen = launcher.pick();
    let mut spans = vec![label("Model")];
    spans.extend(
        launcher::picks(launcher.place())
            .iter()
            .filter(|pick| pick.codex == chosen.codex)
            .map(|pick| pill(&pick.label, *pick == chosen, theme().text)),
    );
    let asking = chosen.codex
        && launcher
            .place()
            .is_some_and(|place| place.codex_models.is_none());
    if asking {
        spans.push(Span::styled(
            "  asking for models…",
            style::fg(theme().faint),
        ));
    }
    Line::from(clip(spans, width))
}

fn keys_line(width: usize) -> Line<'static> {
    let keys: Vec<Span<'static>> = [
        ("enter", "start"),
        ("↑↓", "project"),
        ("←→", "machine"),
        ("⇥", "model"),
        ("esc", "cancel"),
    ]
    .iter()
    .enumerate()
    .flat_map(|(index, (key, what))| {
        [
            Span::styled(
                format!("{}{key}", if index == 0 { "" } else { "   " }),
                style::bold(theme().text),
            ),
            Span::styled(format!(" {what}"), style::fg(theme().faint)),
        ]
    })
    .collect();
    Line::from(clip(keys, width))
}
