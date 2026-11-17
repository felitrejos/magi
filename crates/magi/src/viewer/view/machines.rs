use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Cursor, body, clip, draw_panel, spread, truncate};
use crate::proto::{CoreKind, FULL};
use crate::viewer::graphics::{Chart, Graphics, Kind, Series};
use crate::viewer::model::{
    self, HISTORY, Level, LinkState, Machine, Model, Presence, Row, Subsystem,
};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const TWO_COLUMNS: u16 = 70;
const TILE_GAP: u16 = 3;
const ROW_GAP: u16 = 1;
const MIN_LABELLED_ROWS: u16 = 5;
const CORE_GAP: u16 = 3;
const MIN_CORE_WIDTH: u16 = 18;
const CORE_PERCENT: u16 = 4;
const CORE_LABEL: u16 = 3;
const MAX_FILESYSTEMS: usize = 3;
const FS_PERCENT: usize = 4;

fn level_text(value: f32, subsystem: Subsystem) -> Color {
    match model::grade(subsystem, value) {
        Level::Normal => theme().text,
        level => style::level(level),
    }
}

fn short_os(os: &str) -> &str {
    os.split_whitespace().next().unwrap_or(os)
}

fn status_span(row: &Row<'_>, port: u16, now: Instant, room: usize) -> Span<'static> {
    let color = match row.state() {
        Some(LinkState::Lost { .. } | LinkState::Rejected(_)) => theme().crit,
        _ => theme().dim,
    };
    Span::styled(truncate(&status(row, port, now), room), style::fg(color))
}

fn status(row: &Row<'_>, port: u16, now: Instant) -> String {
    match &row.presence {
        Presence::Searching => "looking for its agent…".into(),
        Presence::NoAgent => format!("no agent on port {port} · run `magi agent install`"),
        Presence::Agent(link) => match &link.state {
            LinkState::Lost { since, reason } => format!(
                "lost {} ago · {reason} · retrying",
                units::duration(now.duration_since(*since))
            ),
            LinkState::Rejected(reason) => format!("refused: {reason}"),
            LinkState::Connecting | LinkState::Live => "connecting…".into(),
        },
    }
}

pub fn render_detail(
    frame: &mut Frame,
    area: Rect,
    row: &Row<'_>,
    model: &Model,
    now: Instant,
    graphics: &mut Graphics,
) {
    let orphans = model.orphaned_forwards(&row.name, now);
    let port = model.port;
    if area.height < 4 || area.width < 16 {
        return;
    }
    let place = if row.here {
        "this machine"
    } else {
        "on your tailnet"
    };
    draw_panel(
        frame,
        area,
        vec![Span::styled(row.name.clone(), style::bold(theme().text))],
        vec![Span::styled(place.to_string(), style::fg(theme().dim))],
    );
    let area = body(area);
    let width = area.width as usize;
    let mut cursor = Cursor::new(Rect::new(area.x, area.y - 1, area.width, area.height + 1));
    if let Some(machine) = row.machine() {
        cursor.line(frame, Line::from(clip(facts(machine), width)));
    }
    for forward in &orphans {
        let ports: Vec<String> = forward
            .ports
            .iter()
            .map(|port| format!("localhost:{port}"))
            .collect();
        let short: String = forward.session.chars().take(8).collect();
        cursor.line(
            frame,
            Line::from(clip(
                vec![
                    Span::styled(
                        format!(
                            "{} → {} for a session that left  ",
                            ports.join(", "),
                            forward.host
                        ),
                        style::fg(theme().warn),
                    ),
                    Span::styled(
                        format!("magi forwards close {short}"),
                        style::fg(theme().dim),
                    ),
                ],
                width,
            )),
        );
    }
    let Some((machine, sample)) = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)))
    else {
        cursor.line(frame, Line::from(status_span(row, port, now, width)));
        return;
    };
    cursor.skip();
    let columns: u16 = if area.width >= TWO_COLUMNS { 2 } else { 1 };
    let tile_width = (area.width - (columns - 1) * TILE_GAP) / columns;
    let window = (usize::from(tile_width) * graphics.samples_per_cell()).min(HISTORY);
    let tiles = tiles(machine, sample, window, tile_width);
    let sections = (tiles.len() as u16 + 2).div_ceil(columns);
    let height = (cursor.left() + ROW_GAP) / sections;
    let rect = |index: u16| {
        let (column, line) = (index % columns, index / columns);
        Rect::new(
            area.x + column * (tile_width + TILE_GAP),
            cursor.y + line * height,
            tile_width,
            height.saturating_sub(ROW_GAP),
        )
    };
    for (index, tile) in tiles.iter().enumerate() {
        draw_tile(frame, graphics, rect(index as u16), tile, machine.interval);
    }
    let next = tiles.len() as u16;
    draw_cores(frame, graphics, rect(next), machine, sample, window);
    draw_processes(frame, rect(next + 1), sample);
}

type Note = (Vec<Span<'static>>, Vec<Span<'static>>);

struct Tile {
    title: &'static str,
    value: Span<'static>,
    detail: Vec<Span<'static>>,
    notes: Vec<Note>,
    max: f32,
    scale: Scale,
    series: Vec<(Vec<f32>, Color)>,
}

struct Scale {
    top: String,
    bottom: String,
}

fn percent_scale() -> Scale {
    Scale {
        top: "100%".into(),
        bottom: "0%".into(),
    }
}

fn rate_scale(peak: f32) -> Scale {
    Scale {
        top: units::rate(peak as u64),
        bottom: "0".into(),
    }
}

fn tiles(machine: &Machine, sample: &crate::proto::Sample, window: usize, width: u16) -> Vec<Tile> {
    let room = usize::from(width).saturating_sub(FS_PERCENT + 3);
    let cpu = machine.smoothed_cpu();
    let mut cpu_detail = vec![Span::styled(
        format!("load {:.2}", sample.load[0]),
        style::fg(theme().dim),
    )];
    if let Some(degrees) = sample.temperature {
        cpu_detail.push(Span::styled(
            format!(" · {degrees:.0} °C"),
            style::fg(temperature_color(degrees)),
        ));
    }
    let memory = machine.memory_fraction();
    let mut memory_detail = format!(
        "{} of {}",
        units::bytes(sample.memory.used),
        units::bytes(sample.memory.total)
    );
    if sample.memory.swap_used > 0 {
        memory_detail.push_str(&format!(
            " · swap {}",
            units::bytes(sample.memory.swap_used)
        ));
    }
    let rates = |first: &model::History, second: &model::History| {
        let (first, second) = (first.tail(window), second.tail(window));
        let peak = first.iter().chain(&second).copied().fold(1.0, f32::max) * 1.15;
        (
            peak,
            vec![(first, theme().accent), (second, theme().second)],
        )
    };
    let (network_peak, network) = rates(&machine.rx, &machine.tx);
    let (disk_peak, disk) = rates(&machine.disk_read, &machine.disk_write);
    let filesystems = sample
        .filesystems
        .iter()
        .take(MAX_FILESYSTEMS)
        .map(|fs| {
            let used = model::fraction(fs.used, fs.total);
            let size = format!("{} of {}", units::bytes(fs.used), units::bytes(fs.total));
            (
                vec![
                    percent_span(used, Subsystem::Storage),
                    Span::raw("  "),
                    Span::styled(
                        ending(&fs.mount, room.saturating_sub(size.width())),
                        style::fg(theme().dim),
                    ),
                ],
                vec![Span::styled(size, style::fg(theme().faint))],
            )
        })
        .collect();
    vec![
        Tile {
            title: "CPU",
            value: percent_span(cpu, Subsystem::Cpu),
            detail: cpu_detail,
            notes: Vec::new(),
            max: 1.0,
            scale: percent_scale(),
            series: vec![(machine.cpu.tail(window), theme().accent)],
        },
        Tile {
            title: "Memory",
            value: percent_span(memory, Subsystem::Memory),
            detail: vec![Span::styled(memory_detail, style::fg(theme().dim))],
            notes: Vec::new(),
            max: 1.0,
            scale: percent_scale(),
            series: vec![(machine.memory.tail(window), theme().second)],
        },
        Tile {
            title: "Network",
            value: Span::styled(
                format!("↓ {}", units::rate(sample.network.rx_per_sec)),
                style::bold(theme().accent),
            ),
            detail: vec![Span::styled(
                format!("↑ {}", units::rate(sample.network.tx_per_sec)),
                style::bold(theme().second),
            )],
            notes: Vec::new(),
            max: network_peak,
            scale: rate_scale(network_peak),
            series: network,
        },
        Tile {
            title: "Disk",
            value: Span::styled(
                format!("read {}", units::rate(sample.disk.read_per_sec)),
                style::bold(theme().accent),
            ),
            detail: vec![Span::styled(
                format!("write {}", units::rate(sample.disk.write_per_sec)),
                style::bold(theme().second),
            )],
            notes: filesystems,
            max: disk_peak,
            scale: rate_scale(disk_peak),
            series: disk,
        },
    ]
}

fn draw_tile(
    frame: &mut Frame,
    graphics: &mut Graphics,
    rect: Rect,
    tile: &Tile,
    interval: Duration,
) {
    let rect = rect.intersection(frame.area());
    if rect.height == 0 {
        return;
    }
    let width = rect.width as usize;
    let mut cursor = Cursor::new(rect);
    cursor.line(
        frame,
        spread(
            vec![
                Span::styled(tile.title, style::bold(theme().text)),
                Span::raw("  "),
                tile.value.clone(),
            ],
            tile.detail.clone(),
            width,
        ),
    );
    for (left, right) in &tile.notes {
        cursor.line(frame, spread(left.clone(), right.clone(), width));
    }
    let samples = tile.series.first().map_or(0, |(values, _)| values.len());
    let chart = Chart {
        kind: Kind::Area,
        max: tile.max,
        guide: false,
        series: tile
            .series
            .iter()
            .map(|(values, color)| Series {
                values,
                color: *color,
            })
            .collect(),
    };
    draw_scaled(
        frame,
        graphics,
        cursor.rest(),
        format!("detail:{}", tile.title),
        chart,
        &tile.scale,
        interval * samples.saturating_sub(1) as u32,
    );
}

fn draw_scaled(
    frame: &mut Frame,
    graphics: &mut Graphics,
    area: Rect,
    slot: String,
    mut chart: Chart,
    scale: &Scale,
    shown: Duration,
) {
    if area.height == 0 {
        return;
    }
    let labelled = area.height >= MIN_LABELLED_ROWS;
    let rows = area.height - if labelled { 2 } else { 0 };
    let top = area.y + u16::from(labelled);
    chart.guide = rows >= 3;
    graphics.chart(
        frame.buffer_mut(),
        Rect::new(area.x, top, area.width, rows),
        slot,
        &chart,
    );
    if !labelled {
        return;
    }
    let faint = |text: String| Span::styled(text, style::fg(theme().faint));
    let width = usize::from(area.width);
    frame.render_widget(
        Paragraph::new(Line::from(faint(scale.top.clone()))),
        Rect::new(area.x, area.y, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new(spread(
            vec![faint(format!(
                "{} · {} ago",
                scale.bottom,
                units::duration(shown)
            ))],
            vec![faint("now".into())],
            width,
        )),
        Rect::new(area.x, top + rows, area.width, 1),
    );
}

fn draw_cores(
    frame: &mut Frame,
    graphics: &mut Graphics,
    area: Rect,
    machine: &Machine,
    sample: &crate::proto::Sample,
    window: usize,
) {
    let area = area.intersection(frame.area());
    let cores = &sample.cores;
    if area.height < 2 || cores.is_empty() {
        return;
    }
    let load = |index: usize| f32::from(cores[index]) / f32::from(FULL);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("Cores", style::bold(theme().text)))),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let groups = core_groups(&machine.identity.core_kinds, cores.len());
    let titled = groups.len() > 1;
    let titles = if titled {
        groups.len() as u16 * 2 - 1
    } else {
        0
    };
    let grid = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let space = grid.height.saturating_sub(titles);
    let sizes: Vec<u16> = groups
        .iter()
        .map(|group| group.cores.len() as u16)
        .collect();
    let columns = core_columns(&sizes, grid.width, space);
    let lines: u16 = sizes.iter().map(|size| size.div_ceil(columns)).sum();
    let cell_height = (space / lines.max(1)).max(1);
    let cell_width = (grid.width + CORE_GAP) / columns - CORE_GAP;
    let bottom = grid.y + grid.height;
    let mut y = grid.y;
    for (position, group) in groups.iter().enumerate() {
        if titled {
            if position > 0 {
                y += 1;
            }
            if y >= bottom {
                return;
            }
            let average = group.cores.iter().map(|index| load(*index)).sum::<f32>()
                / group.cores.len() as f32;
            frame.render_widget(
                Paragraph::new(spread(
                    vec![Span::styled(group.title, style::fg(theme().dim))],
                    vec![Span::styled(
                        format!(
                            "{} cores · {}",
                            group.cores.len(),
                            units::percent(average).trim()
                        ),
                        style::fg(theme().faint),
                    )],
                    usize::from(grid.width),
                )),
                Rect::new(grid.x, y, grid.width, 1),
            );
            y += 1;
        }
        let group_lines = (group.cores.len() as u16).div_ceil(columns);
        for (number, core) in group.cores.iter().enumerate() {
            let number = number as u16;
            let cell = Rect::new(
                grid.x + number / group_lines * (cell_width + CORE_GAP),
                y + number % group_lines * cell_height,
                cell_width,
                cell_height,
            );
            if cell.y >= bottom {
                continue;
            }
            let history = machine
                .core_history
                .get(*core)
                .map_or_else(Vec::new, |history| history.tail(window));
            let fraction = load(*core);
            draw_core(
                frame,
                graphics,
                Rect {
                    height: cell.height.min(bottom - cell.y),
                    ..cell
                },
                Core {
                    slot: format!("core:{core}"),
                    label: number + 1,
                    fraction,
                    history: &history,
                    color: match model::grade(Subsystem::Cpu, fraction) {
                        Level::Normal => group.color,
                        level => style::level(level),
                    },
                },
            );
        }
        y += group_lines * cell_height;
    }
}

struct CoreGroup {
    title: &'static str,
    color: Color,
    cores: Vec<usize>,
}

fn core_groups(kinds: &[CoreKind], count: usize) -> Vec<CoreGroup> {
    let of = |kind: CoreKind| -> Vec<usize> {
        (0..count)
            .filter(|index| kinds.get(*index) == Some(&kind))
            .collect()
    };
    let (performance, efficiency) = (of(CoreKind::Performance), of(CoreKind::Efficiency));
    if kinds.len() != count || performance.is_empty() || efficiency.is_empty() {
        return vec![CoreGroup {
            title: "",
            color: theme().accent,
            cores: (0..count).collect(),
        }];
    }
    vec![
        CoreGroup {
            title: "Performance",
            color: theme().accent,
            cores: performance,
        },
        CoreGroup {
            title: "Efficiency",
            color: theme().second,
            cores: efficiency,
        },
    ]
}

struct Core<'a> {
    slot: String,
    label: u16,
    fraction: f32,
    history: &'a [f32],
    color: Color,
}

fn draw_core(frame: &mut Frame, graphics: &mut Graphics, cell: Rect, core: Core) {
    let label_width = CORE_LABEL;
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(
                "{:>width$}",
                core.label,
                width = usize::from(label_width) - 1
            ),
            style::fg(theme().faint),
        )),
        Rect::new(cell.x, cell.y, label_width, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::from(percent_span(core.fraction, Subsystem::Cpu)).right_aligned()),
        Rect::new(cell.x + cell.width - CORE_PERCENT, cell.y, CORE_PERCENT, 1),
    );
    let chart_height = if cell.height >= 3 {
        cell.height - 1
    } else {
        cell.height
    };
    graphics.chart(
        frame.buffer_mut(),
        Rect::new(
            cell.x + label_width,
            cell.y,
            cell.width.saturating_sub(label_width + CORE_PERCENT + 1),
            chart_height,
        ),
        core.slot,
        &Chart {
            kind: Kind::Area,
            max: 1.0,
            guide: false,
            series: vec![Series {
                values: core.history,
                color: core.color,
            }],
        },
    );
}

fn core_columns(groups: &[u16], width: u16, space: u16) -> u16 {
    let largest = groups.iter().copied().max().unwrap_or(1).max(1);
    let widest = ((width + CORE_GAP) / (MIN_CORE_WIDTH + CORE_GAP)).clamp(1, largest);
    let lines = |columns: u16| -> u16 { groups.iter().map(|size| size.div_ceil(columns)).sum() };
    let fits = |least: u16| (1..=widest).find(|columns| lines(*columns) * least <= space);
    fits(2).or_else(|| fits(1)).unwrap_or(widest)
}

fn draw_processes(frame: &mut Frame, area: Rect, sample: &crate::proto::Sample) {
    let area = area.intersection(frame.area());
    let width = area.width as usize;
    let mut cursor = Cursor::new(area);
    cursor.line(
        frame,
        spread(
            vec![Span::styled("Using it", style::bold(theme().text))],
            vec![Span::styled(
                format!("{:>6}  {:>8}", "CPU", "memory"),
                style::fg(theme().faint),
            )],
            width,
        ),
    );
    for process in &sample.processes {
        if cursor.left() == 0 {
            return;
        }
        let session = process
            .session
            .as_ref()
            .and_then(|id| sample.sessions.iter().find(|session| session.id == *id));
        let mut left = vec![Span::styled(process.name.clone(), style::fg(theme().text))];
        if let Some(session) = session {
            let (mark, color) = style::agent(session.agent);
            left.push(Span::styled(format!("  {mark} "), style::fg(color)));
            left.push(Span::styled(session.title.clone(), style::fg(theme().dim)));
        }
        let cpu = process.cpu.map_or_else(
            || "–".to_string(),
            |millicores| format!("{}%", millicores / 10),
        );
        cursor.line(
            frame,
            spread(
                left,
                vec![Span::styled(
                    format!("{cpu:>6}  {:>8}", units::bytes(process.memory)),
                    style::fg(theme().dim),
                )],
                width,
            ),
        );
    }
}

fn ending(text: &str, width: usize) -> String {
    if text.width() <= width {
        text.to_string()
    } else {
        format!("…{}", super::tail(text, width.saturating_sub(1)))
    }
}

fn percent_span(value: f32, subsystem: Subsystem) -> Span<'static> {
    Span::styled(
        units::percent(value).trim().to_string(),
        style::bold(level_text(value, subsystem)),
    )
}

fn temperature_color(degrees: f32) -> Color {
    if degrees >= 90.0 {
        theme().crit
    } else if degrees >= 80.0 {
        theme().warn
    } else {
        theme().dim
    }
}

fn facts(machine: &Machine) -> Vec<Span<'static>> {
    let mut details = vec![short_os(&machine.identity.os).to_string()];
    if let Some(sample) = &machine.latest {
        details.push(format!("{} cores", sample.cores.len()));
        details.push(units::bytes(sample.memory.total));
        details.push(format!(
            "up {}",
            units::duration(Duration::from_secs(sample.uptime_secs))
        ));
    }
    details.push(machine.identity.cpu_model.clone());
    vec![Span::styled(
        details
            .into_iter()
            .filter(|detail| !detail.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        style::fg(theme().dim),
    )]
}
