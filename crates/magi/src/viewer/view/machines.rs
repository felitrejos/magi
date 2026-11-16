use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};

use super::{Composed, Cursor, Mark, body, clip, draw_panel, pad, panel, selected_style, truncate};
use crate::proto::FULL;
use crate::viewer::graphics::{Chart, Graphics, Kind, Series};
use crate::viewer::model::{
    self, HISTORY, Level, LinkState, Machine, Model, Presence, Row, Subsystem,
};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_width::UnicodeWidthStr;

const SIDEBAR_BAR: usize = 3;
const ICON_WIDTH: u16 = 2;
const ICON_GAP: &str = "   ";
const SIDEBAR_GAUGE: usize = SIDEBAR_BAR + 5;

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    rows: &[Row<'_>],
    marks: &HashMap<String, Mark>,
    now: Instant,
    graphics: &mut Graphics,
) {
    if area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let visible = area.height as usize;
    let first = if model.selected < rows.len() {
        model.selected.saturating_sub(visible - 1)
    } else {
        0
    };
    let name_width = rows
        .iter()
        .map(|row| {
            marks
                .get(&row.name)
                .map_or(row.name.width(), |mark| mark.tag.width())
        })
        .max()
        .unwrap_or(0)
        .clamp(6, width.saturating_sub(SIDEBAR_GAUGE + 5).max(6));
    let icons = graphics.has_pixels();
    let mut cursor = Cursor::new(area);
    for (index, row) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = index == model.selected;
        let name_style = if selected {
            style::bold(theme().text)
        } else {
            style::fg(theme().text)
        };
        let live = row
            .machine()
            .filter(|_| row.state() == Some(&LinkState::Live))
            .and_then(|machine| machine.latest.as_ref().map(|_| machine));
        let line = Composed::of(vec![
            Span::raw(" "),
            Span::raw(if icons { ICON_GAP } else { "" }),
            Span::styled(
                truncate(
                    marks
                        .get(&row.name)
                        .map_or(row.name.as_str(), |mark| mark.tag.as_str()),
                    name_width,
                ),
                name_style,
            ),
        ]);
        let room = width.saturating_sub(name_width + 2);
        let right = match live {
            Some(machine) => sidebar_gauge(machine, index),
            None => Composed::of(vec![status_span(row, model.port, now, room)]),
        };
        let at = cursor.row();
        line.spread(right, width)
            .draw(frame, graphics, at, selected_style(selected));
        if let Some(mark) = marks.get(&row.name).filter(|_| icons) {
            graphics.icon(
                frame.buffer_mut(),
                Rect::new(at.x + 1, at.y, ICON_WIDTH, 1),
                format!("mark:{}", row.name),
                &mark.symbols,
                if selected { theme().text } else { theme().dim },
            );
        }
        cursor.skip();
    }
}

fn sidebar_gauge(machine: &Machine, index: usize) -> Composed {
    let cpu = machine.smoothed_cpu();
    let memory = machine.memory_fraction();
    let mut gauge = Composed::default();
    if model::grade(Subsystem::Memory, memory) != Level::Normal {
        gauge.push(Span::styled(
            format!("mem {}  ", units::percent(memory).trim()),
            style::fg(level_text(memory, Subsystem::Memory)),
        ));
    }
    gauge.bar(
        SIDEBAR_BAR,
        cpu,
        style::level(model::grade(Subsystem::Cpu, cpu)),
        format!("sidebar:cpu:{index}"),
    );
    gauge.push(Span::styled(
        format!(" {:>4}", units::percent(cpu).trim()),
        style::fg(level_text(cpu, Subsystem::Cpu)),
    ));
    gauge
}

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
    port: u16,
    now: Instant,
    graphics: &mut Graphics,
) {
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
        panel(
            vec![Span::styled(row.name.clone(), style::bold(theme().text))],
            Some(vec![Span::styled(place, style::fg(theme().dim))]),
        ),
    );
    let area = body(area);
    let width = area.width as usize;
    let mut cursor = Cursor::new(Rect::new(area.x, area.y - 1, area.width, area.height + 1));
    if let Some(machine) = row.machine() {
        let mut line = facts(machine);
        if let Some(degrees) = machine
            .latest
            .as_ref()
            .and_then(|sample| sample.temperature)
        {
            line.push(Span::styled(
                format!(" · {degrees:.0} °C"),
                style::fg(temperature_color(degrees)),
            ));
        }
        cursor.line(frame, Line::from(clip(line, width)));
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
    let chart_width = ((width * 2 / 5) as u16).clamp(8, 64);
    let text_width = width.saturating_sub(chart_width as usize + 2);
    let window = (usize::from(chart_width) * graphics.samples_per_cell()).min(HISTORY);
    let chart_area =
        |y: u16, rows: u16| Rect::new(area.x + area.width - chart_width, y, chart_width, rows);

    let cpu = machine.smoothed_cpu();
    let busiest = sample
        .cores
        .iter()
        .map(|core| f32::from(*core) / f32::from(FULL))
        .fold(0.0, f32::max);
    let cpu_history = machine.cpu.tail(window);
    let memory = machine.memory_fraction();
    let memory_history = machine.memory.tail(window);
    let rx = machine.rx.tail(window);
    let tx = machine.tx.tail(window);
    let peak = rx.iter().chain(&tx).copied().fold(1.0, f32::max) * 1.1;
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
    let metrics = [
        (
            "CPU",
            percent_span(cpu, Subsystem::Cpu),
            format!(
                "busiest core {} · load {:.2}",
                units::percent(busiest).trim(),
                sample.load[0]
            ),
            Chart {
                kind: Kind::Area,
                max: 1.0,
                guide: false,
                series: vec![Series {
                    values: &cpu_history,
                    color: theme().accent,
                }],
            },
        ),
        (
            "Memory",
            percent_span(memory, Subsystem::Memory),
            memory_detail,
            Chart {
                kind: Kind::Area,
                max: 1.0,
                guide: false,
                series: vec![Series {
                    values: &memory_history,
                    color: theme().second,
                }],
            },
        ),
        (
            "Network",
            Span::styled(
                format!("↓ {}", units::rate(sample.network.rx_per_sec)),
                style::bold(theme().accent),
            ),
            format!("↑ {}", units::rate(sample.network.tx_per_sec)),
            Chart {
                kind: Kind::Area,
                max: peak,
                guide: false,
                series: vec![
                    Series {
                        values: &rx,
                        color: theme().accent,
                    },
                    Series {
                        values: &tx,
                        color: theme().second,
                    },
                ],
            },
        ),
    ];
    for (title, value, detail, chart) in metrics {
        if cursor.left() < 2 {
            return;
        }
        let y = cursor.y;
        cursor.line(frame, metric_line(title, value, &detail, text_width));
        graphics.chart(
            frame.buffer_mut(),
            chart_area(y, 2),
            format!("detail:{title}"),
            &chart,
        );
        cursor.skip();
        cursor.skip();
    }
    let disk_detail = format!(
        "read {} · write {}",
        units::rate(sample.disk.read_per_sec),
        units::rate(sample.disk.write_per_sec)
    );
    for (index, fs) in sample.filesystems.iter().take(3).enumerate() {
        if cursor.left() == 0 {
            return;
        }
        let used = model::fraction(fs.used, fs.total);
        let title = if index == 0 { "Disk" } else { "" };
        let mut detail = format!(
            "{}  {} of {}",
            fs.mount,
            units::bytes(fs.used),
            units::bytes(fs.total)
        );
        if index == 0 {
            detail.push_str(&format!(" · {disk_detail}"));
        }
        let y = cursor.y;
        cursor.line(
            frame,
            metric_line(
                title,
                percent_span(used, Subsystem::Storage),
                &detail,
                text_width,
            ),
        );
        graphics.meter(
            frame.buffer_mut(),
            chart_area(y, 1),
            format!("detail:disk:{}", fs.mount),
            used,
            style::level(model::grade(Subsystem::Storage, used)),
        );
    }
    cursor.skip();
    if cursor.left() == 0 {
        return;
    }
    cursor.line(frame, cores_line(&sample.cores, busiest, width));
    cursor.skip();
    if cursor.left() < 2 || sample.processes.is_empty() {
        return;
    }
    cursor.line(
        frame,
        spread_line(
            vec![Span::styled("Using it", style::bold(theme().dim))],
            vec![Span::styled(
                format!("{:>6}  {:>7}", "CPU", "memory"),
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
            spread_line(
                left,
                vec![Span::styled(
                    format!("{cpu:>6}  {:>7}", units::bytes(process.memory)),
                    style::fg(theme().dim),
                )],
                width,
            ),
        );
    }
}

fn metric_line(title: &str, value: Span<'static>, detail: &str, width: usize) -> Line<'static> {
    Line::from(clip(
        vec![
            Span::styled(pad(title, 9), style::fg(theme().dim)),
            value,
            Span::styled(format!("  {detail}"), style::fg(theme().faint)),
        ],
        width,
    ))
}

fn percent_span(value: f32, subsystem: Subsystem) -> Span<'static> {
    Span::styled(
        units::percent(value).trim().to_string(),
        style::bold(level_text(value, subsystem)),
    )
}

fn cores_line(cores: &[u16], busiest: f32, width: usize) -> Line<'static> {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let mut spans = vec![Span::styled(pad("Cores", 9), style::fg(theme().dim))];
    let room = width.saturating_sub(9 + 22) / 2;
    for core in cores.iter().take(room) {
        let fraction = f32::from(*core) / f32::from(FULL);
        let level = ((fraction * 7.0).round() as usize).min(7);
        spans.push(Span::styled(
            format!("{} ", LEVELS[level]),
            style::fg(style::level(model::grade(Subsystem::Cpu, fraction))),
        ));
    }
    spans.push(Span::styled(
        format!(" busiest {}", units::percent(busiest).trim()),
        style::fg(theme().faint),
    ));
    Line::from(clip(spans, width))
}

fn spread_line(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    super::spread(left, right, width)
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
