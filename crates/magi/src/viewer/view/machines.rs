use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Composed, Cursor, clip, icon, pad, panel, selected_style, spans_width, truncate};
use crate::proto::FULL;
use crate::viewer::graphics::{Chart, Graphics, Kind, Series};
use crate::viewer::model::{
    self, HISTORY, Level, LinkState, Machine, Model, Presence, Row, Subsystem,
};
use crate::viewer::style::{self, theme};
use crate::viewer::units;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const GUTTER: u16 = 9;

pub fn render_list(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    rows: &[Row<'_>],
    wide: bool,
    now: Instant,
    graphics: &mut Graphics,
) {
    let block = panel(
        vec![Span::styled("Machines", style::bold(theme().text))],
        None,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let per_row = if wide { 2 } else { 1 };
    let visible = (inner.height as usize / per_row).max(1);
    let first = if model.selected < rows.len() {
        model.selected.saturating_sub(visible - 1)
    } else {
        0
    };
    let mut cursor = Cursor::new(inner);
    for (index, row) in rows.iter().enumerate().skip(first).take(visible) {
        if cursor.left() == 0 {
            break;
        }
        let selected = index == model.selected;
        let marker = Span::styled(if selected { "▎" } else { " " }, style::fg(theme().accent));
        let name_style = if selected {
            style::bold(theme().text)
        } else {
            style::fg(theme().text)
        };
        let live = row
            .machine()
            .filter(|_| row.state() == Some(&LinkState::Live))
            .and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)));
        if wide {
            let facts = live.map_or_else(Vec::new, |(machine, sample)| {
                vec![Span::styled(
                    format!(
                        "{} · up {}",
                        short_os(&machine.identity.os),
                        units::duration(Duration::from_secs(sample.uptime_secs))
                    ),
                    style::fg(theme().faint),
                )]
            });
            Composed::of(vec![
                marker.clone(),
                Span::styled(row.name.clone(), name_style),
            ])
            .spread(Composed::of(facts), width)
            .draw(frame, graphics, cursor.row(), selected_style(selected));
            cursor.skip();
            if cursor.left() == 0 {
                break;
            }
            let mut second = Composed::of(vec![Span::raw(" ")]);
            match live {
                Some((machine, _)) => gauges(&mut second, machine, index, width - 1),
                None => second.push(status_span(row, model.port, now, width - 1)),
            }
            second.draw(frame, graphics, cursor.row(), selected_style(selected));
            cursor.skip();
        } else {
            let name_width = (width / 2).clamp(6, 24);
            let mut spans = vec![
                marker,
                Span::raw(" "),
                Span::styled(
                    pad(&truncate(&row.name, name_width), name_width + 1),
                    name_style,
                ),
            ];
            let room = width.saturating_sub(spans_width(&spans));
            match live {
                Some((machine, _)) => spans.extend(compact_gauges(machine, room)),
                None => spans.push(status_span(row, model.port, now, room)),
            }
            Composed::of(spans).draw(frame, graphics, cursor.row(), selected_style(selected));
            cursor.skip();
        }
    }
}

fn gauges(line: &mut Composed, machine: &Machine, index: usize, width: usize) {
    let bar = (width.saturating_sub(31) / 2).clamp(3, 14);
    for (label, value, subsystem) in [
        ("cpu", machine.smoothed_cpu(), Subsystem::Cpu),
        ("mem", machine.memory_fraction(), Subsystem::Memory),
    ] {
        line.push(Span::styled(format!("{label} "), style::fg(theme().dim)));
        line.bar(
            bar,
            value,
            style::level(model::grade(subsystem, value)),
            format!("{label}:{index}"),
        );
        line.push(Span::styled(
            format!("{:>5}  ", units::percent(value).trim()),
            style::fg(theme().text),
        ));
    }
    let disk = machine.fullest_filesystem();
    line.push(Span::styled("disk ", style::fg(theme().dim)));
    line.push(Span::styled(
        units::percent(disk).trim().to_string(),
        style::fg(level_text(disk, Subsystem::Storage)),
    ));
}

fn level_text(value: f32, subsystem: Subsystem) -> Color {
    match model::grade(subsystem, value) {
        Level::Normal => theme().text,
        level => style::level(level),
    }
}

fn compact_gauges(machine: &Machine, room: usize) -> Vec<Span<'static>> {
    let text = |label: &str, value: f32, subsystem| {
        vec![
            Span::styled(format!("{label} "), style::fg(theme().dim)),
            Span::styled(
                format!("{:<5}", units::percent(value).trim()),
                style::fg(level_text(value, subsystem)),
            ),
        ]
    };
    let mut spans = text("cpu", machine.smoothed_cpu(), Subsystem::Cpu);
    spans.extend(text("mem", machine.memory_fraction(), Subsystem::Memory));
    if room >= 30 {
        spans.extend(text(
            "disk",
            machine.fullest_filesystem(),
            Subsystem::Storage,
        ));
    }
    spans
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
    let area = area.inner(Margin {
        horizontal: 2,
        vertical: 0,
    });
    if area.height < 2 || area.width < 12 {
        return;
    }
    let width = area.width as usize;
    let mut cursor = Cursor::new(area);
    let mut header = vec![
        Span::styled(row.name.clone(), style::bold(theme().text)),
        Span::styled(
            if row.here {
                "  this machine"
            } else {
                "  on your tailnet"
            },
            style::fg(theme().dim),
        ),
    ];
    if let Some(machine) = row.machine() {
        header.push(Span::styled("  ·  ", style::fg(theme().faint)));
        header.extend(facts(machine));
    }
    cursor.line(frame, Line::from(clip(header, width)));
    let Some((machine, sample)) = row
        .machine()
        .filter(|_| row.state() == Some(&LinkState::Live))
        .and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)))
    else {
        cursor.line(frame, Line::from(status_span(row, port, now, width)));
        return;
    };
    let disks: Vec<_> = sample.filesystems.iter().take(3).collect();
    let plan = Plan::fit(cursor.left().saturating_sub(1 + disks.len() as u16));
    let plot_width = (width.saturating_sub(GUTTER as usize)).max(1);
    let window = (plot_width * graphics.samples_per_cell()).min(HISTORY);
    let stretches = graphics.stretches_history();
    let shown = |samples: usize| {
        span(
            if stretches { samples } else { plot_width },
            machine.interval,
        )
    };

    let mut sections = Vec::with_capacity(3);
    let cpu = machine.smoothed_cpu();
    let busiest = sample
        .cores
        .iter()
        .map(|core| f32::from(*core) / f32::from(FULL))
        .fold(0.0, f32::max);
    let cpu_history = machine.cpu.tail(window);
    sections.push(ChartSection {
        icon: "◉",
        title: "CPU",
        short: vec![Span::styled(
            units::percent(cpu).trim().to_string(),
            style::bold(level_text(cpu, Subsystem::Cpu)),
        )],
        value: vec![
            Span::styled(
                units::percent(cpu).trim().to_string(),
                style::bold(level_text(cpu, Subsystem::Cpu)),
            ),
            Span::styled(
                format!(
                    " · busiest core {} · load {:.2}",
                    units::percent(busiest).trim(),
                    sample.load[0]
                ),
                style::fg(theme().dim),
            ),
        ],
        slot: "cpu",
        chart: Chart {
            kind: Kind::Area,
            max: 1.0,
            guide: false,
            series: vec![Series {
                values: &cpu_history,
                color: theme().accent,
            }],
        },
        scale: percent_scale(),
        shown: shown(cpu_history.len()),
    });

    let memory = machine.memory_fraction();
    let mut memory_value = vec![
        Span::styled(
            units::percent(memory).trim().to_string(),
            style::bold(level_text(memory, Subsystem::Memory)),
        ),
        Span::styled(
            format!(
                " · {} of {}",
                units::bytes(sample.memory.used),
                units::bytes(sample.memory.total)
            ),
            style::fg(theme().dim),
        ),
    ];
    if sample.memory.swap_used > 0 {
        memory_value.push(Span::styled(
            format!(" · swap {}", units::bytes(sample.memory.swap_used)),
            style::fg(theme().dim),
        ));
    }
    let memory_history = machine.memory.tail(window);
    sections.push(ChartSection {
        icon: "◫",
        title: "Memory",
        short: memory_value[..2].to_vec(),
        value: memory_value,
        slot: "memory",
        chart: Chart {
            kind: Kind::Area,
            max: 1.0,
            guide: false,
            series: vec![Series {
                values: &memory_history,
                color: theme().second,
            }],
        },
        scale: percent_scale(),
        shown: shown(memory_history.len()),
    });

    let rx = machine.rx.tail(window);
    let tx = machine.tx.tail(window);
    let peak = rx.iter().chain(&tx).copied().fold(1.0, f32::max) * 1.1;
    sections.push(ChartSection {
        icon: "◎",
        title: "Network",
        short: vec![
            Span::styled(
                format!("↓ {}", units::rate(sample.network.rx_per_sec)),
                style::fg(theme().accent),
            ),
            Span::styled(
                format!("  ↑ {}", units::rate(sample.network.tx_per_sec)),
                style::fg(theme().second),
            ),
        ],
        value: vec![
            Span::styled(
                format!("↓ download {}", units::rate(sample.network.rx_per_sec)),
                style::fg(theme().accent),
            ),
            Span::styled(
                format!("   ↑ upload {}", units::rate(sample.network.tx_per_sec)),
                style::fg(theme().second),
            ),
        ],
        slot: "network",
        chart: Chart {
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
        scale: Scale {
            top: units::rate(peak as u64),
            middle: Some(units::rate((peak / 2.0) as u64)),
            bottom: "0".into(),
        },
        shown: shown(rx.len()),
    });

    for (section, rows) in sections.into_iter().zip(plan.rows).take(plan.charts) {
        if plan.gap {
            cursor.skip();
        }
        chart_section(&mut cursor, frame, graphics, section, rows, plan.axis);
    }
    if plan.gap {
        cursor.skip();
    }
    cursor.line(
        frame,
        super::rule(
            icon("◧"),
            "Storage",
            vec![Span::styled(
                format!(
                    "read {} · write {}",
                    units::rate(sample.disk.read_per_sec),
                    units::rate(sample.disk.write_per_sec)
                ),
                style::fg(theme().dim),
            )],
            width,
        ),
    );
    for fs in disks {
        let used = model::fraction(fs.used, fs.total);
        let label_width = (width / 4).clamp(6, 20);
        let size = format!(
            "  {:>5}  {} of {}",
            units::percent(used).trim(),
            units::bytes(fs.used),
            units::bytes(fs.total)
        );
        let bar = width.saturating_sub(label_width + 1 + size.width()).min(28);
        let mut line = Composed::of(vec![Span::styled(
            pad(&truncate_left(&fs.mount, label_width), label_width + 1),
            style::fg(theme().dim),
        )]);
        line.bar(
            bar,
            used,
            style::level(model::grade(Subsystem::Storage, used)),
            format!("disk:{}", fs.mount),
        );
        line.push(Span::styled(size, style::fg(theme().text)));
        cursor.composed(frame, graphics, line);
    }
}

struct Scale {
    top: String,
    middle: Option<String>,
    bottom: String,
}

struct ChartSection<'a> {
    icon: &'static str,
    title: &'static str,
    value: Vec<Span<'static>>,
    short: Vec<Span<'static>>,
    slot: &'static str,
    chart: Chart<'a>,
    scale: Scale,
    shown: Duration,
}

fn chart_section(
    cursor: &mut Cursor,
    frame: &mut Frame,
    graphics: &mut Graphics,
    section: ChartSection,
    rows: u16,
    axis: bool,
) {
    let ChartSection {
        icon: symbol,
        title,
        value,
        short,
        slot,
        mut chart,
        scale,
        shown,
    } = section;
    let width = cursor.area.width as usize;
    let fits = title.width() + spans_width(&value) + 12 <= width;
    cursor.line(
        frame,
        super::rule(icon(symbol), title, if fits { value } else { short }, width),
    );
    let rows = rows.min(cursor.left());
    if rows == 0 || cursor.area.width <= GUTTER {
        return;
    }
    let top = cursor.y;
    let bottom = top + rows - 1;
    let x = cursor.area.x;
    let mut place = |y: u16, text: &str| {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{text:>width$} ", width = GUTTER as usize - 1),
                style::fg(theme().faint),
            )),
            Rect::new(x, y, GUTTER, 1),
        );
    };
    if rows > 1 {
        place(top, &scale.top);
        place(bottom, &scale.bottom);
    }
    if let Some(middle) = scale.middle.filter(|_| rows >= 5 && rows % 2 == 1) {
        place(top + rows / 2, &middle);
        chart.guide = true;
    }
    graphics.chart(
        frame.buffer_mut(),
        Rect::new(x + GUTTER, top, cursor.area.width - GUTTER, rows),
        slot.to_string(),
        &chart,
    );
    cursor.y += rows;
    if !axis {
        return;
    }
    let axis = width.saturating_sub(GUTTER as usize);
    cursor.line(
        frame,
        Line::from(vec![
            Span::raw(" ".repeat(GUTTER as usize)),
            Span::styled(
                pad(
                    &format!("{} ago", units::duration(shown)),
                    axis.saturating_sub(3),
                ),
                style::fg(theme().faint),
            ),
            Span::styled("now", style::fg(theme().faint)),
        ]),
    );
}

struct Plan {
    charts: usize,
    rows: [u16; 3],
    axis: bool,
    gap: bool,
}

impl Plan {
    const WEIGHTS: [u16; 3] = [3, 2, 3];

    fn fit(room: u16) -> Self {
        for charts in (1..=3u16).rev() {
            for (gap, axis, least) in [
                (true, true, 2),
                (false, true, 2),
                (false, false, 2),
                (false, false, 1),
            ] {
                let fixed = charts * (1 + u16::from(axis) + least) + u16::from(gap) * (charts + 1);
                let Some(extra) = room.checked_sub(fixed) else {
                    continue;
                };
                let weights = &Self::WEIGHTS[..charts as usize];
                let total: u16 = weights.iter().sum();
                let mut rows = [0; 3];
                let mut given = 0;
                for (row, weight) in rows.iter_mut().zip(weights) {
                    let share = extra * weight / total;
                    *row = least + share;
                    given += share;
                }
                for row in rows.iter_mut().take((extra - given) as usize) {
                    *row += 1;
                }
                return Self {
                    charts: charts as usize,
                    rows,
                    axis,
                    gap,
                };
            }
        }
        Self {
            charts: 0,
            rows: [0; 3],
            axis: false,
            gap: false,
        }
    }
}

fn percent_scale() -> Scale {
    Scale {
        top: "100%".into(),
        middle: Some("50%".into()),
        bottom: "0%".into(),
    }
}

pub fn span(samples: usize, interval: Duration) -> Duration {
    interval * samples.saturating_sub(1) as u32
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

pub fn truncate_left(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let tail: Vec<&str> = text
        .graphemes(true)
        .rev()
        .scan(0, |used, cluster| {
            *used += cluster.width();
            (*used < width).then_some(cluster)
        })
        .collect();
    let tail: String = tail.into_iter().rev().collect();
    format!("…{tail}")
}
