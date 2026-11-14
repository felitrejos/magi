use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Composed, Cursor, clip, pad, panel, selected_style, spans_width, truncate};
use crate::proto::FULL;
use crate::viewer::graphics::{Chart, Graphics, Kind, Series};
use crate::viewer::model::{
    self, HISTORY, Level, LinkState, Machine, Model, Presence, Row, Subsystem,
};
use crate::viewer::style::{self, ACCENT, CRIT, DIM, FAINT, GOOD, TEAL, TEXT};
use crate::viewer::units;

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
    let block = panel(vec![Span::styled("Machines", style::bold(TEXT))], None);
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
        let marker = Span::styled(if selected { "▎" } else { " " }, style::fg(ACCENT));
        let name_style = if selected {
            style::bold(TEXT)
        } else {
            style::fg(TEXT)
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
                    style::fg(FAINT),
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
        line.push(Span::styled(format!("{label} "), style::fg(DIM)));
        line.bar(
            bar,
            value,
            style::level(model::grade(subsystem, value)),
            format!("{label}:{index}"),
        );
        line.push(Span::styled(
            format!("{:>5}  ", units::percent(value).trim()),
            style::fg(TEXT),
        ));
    }
    let disk = machine.fullest_filesystem();
    line.push(Span::styled("disk ", style::fg(DIM)));
    line.push(Span::styled(
        units::percent(disk).trim().to_string(),
        style::fg(level_text(disk, Subsystem::Storage)),
    ));
}

fn level_text(value: f32, subsystem: Subsystem) -> Color {
    match model::grade(subsystem, value) {
        Level::Normal => TEXT,
        level => style::level(level),
    }
}

fn compact_gauges(machine: &Machine, room: usize) -> Vec<Span<'static>> {
    let text = |label: &str, value: f32, subsystem| {
        vec![
            Span::styled(format!("{label} "), style::fg(DIM)),
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
        Some(LinkState::Lost { .. } | LinkState::Rejected(_)) => CRIT,
        _ => DIM,
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
    let block = panel(
        vec![Span::styled(row.name.clone(), style::bold(TEXT))],
        Some(vec![Span::styled(
            if row.here {
                "this machine"
            } else {
                "on your tailnet"
            },
            style::fg(DIM),
        )]),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let width = inner.width as usize;
    let mut cursor = Cursor::new(inner);
    let machine = row.machine();
    if let Some(machine) = machine {
        cursor.line(frame, Line::from(clip(facts(machine), width)));
    }
    if row.state() != Some(&LinkState::Live) {
        cursor.line(frame, Line::from(status_span(row, port, now, width)));
    }
    let Some((machine, sample)) =
        machine.and_then(|machine| machine.latest.as_ref().map(|sample| (machine, sample)))
    else {
        return;
    };
    cursor.skip();
    let disks: Vec<_> = sample.filesystems.iter().take(3).collect();
    let fixed_lines = 3 + 3 + 1 + disks.len() as u16;
    let room = cursor.left().saturating_sub(fixed_lines);
    let with_cores = room >= 12 && !sample.cores.is_empty();
    let chart_room = room.saturating_sub(if with_cores { 3 } else { 0 });
    let cpu_rows = (chart_room * 40 / 100).max(2);
    let memory_rows = (chart_room * 25 / 100).max(2);
    let network_rows = chart_room.saturating_sub(cpu_rows + memory_rows).max(2);
    let plot_width = (width.saturating_sub(GUTTER as usize)).max(1);
    let window = (plot_width * graphics.samples_per_cell()).min(HISTORY);
    let stretches = graphics.stretches_history();
    let drawn = |samples: usize| if stretches { samples } else { plot_width };

    let cpu = machine.smoothed_cpu();
    cursor.line(
        frame,
        section(
            "CPU",
            vec![
                Span::styled(
                    units::percent(cpu).trim().to_string(),
                    style::bold(level_text(cpu, Subsystem::Cpu)),
                ),
                Span::styled(format!("  load {:.2}", sample.load[0]), style::fg(DIM)),
            ],
            width,
        ),
    );
    let cpu_history = machine.cpu.tail(window);
    plot(
        &mut cursor,
        frame,
        graphics,
        Plot {
            rows: cpu_rows,
            slot: "cpu",
            chart: Chart {
                kind: Kind::Area,
                max: 1.0,
                series: vec![Series {
                    values: &cpu_history,
                    color: ACCENT,
                }],
            },
            scale: percent_scale(),
            shown: Some(span(drawn(cpu_history.len()), machine.interval)),
        },
    );
    if with_cores {
        let cores: Vec<f32> = sample
            .cores
            .iter()
            .map(|core| f32::from(*core) / f32::from(FULL))
            .collect();
        cursor.line(
            frame,
            section(
                &format!("{} cores, now", cores.len()),
                vec![Span::styled(
                    format!(
                        "busiest {}",
                        units::percent(cores.iter().copied().fold(0.0, f32::max)).trim()
                    ),
                    style::fg(DIM),
                )],
                width,
            ),
        );
        plot(
            &mut cursor,
            frame,
            graphics,
            Plot {
                rows: 2,
                slot: "cores",
                chart: Chart {
                    kind: Kind::Bars,
                    max: 1.0,
                    series: vec![Series {
                        values: &cores,
                        color: ACCENT,
                    }],
                },
                scale: Scale {
                    top: "100%".into(),
                    middle: None,
                    bottom: "0%".into(),
                },
                shown: None,
            },
        );
    }

    let memory = machine.memory_fraction();
    let mut memory_facts = vec![
        Span::styled(
            units::percent(memory).trim().to_string(),
            style::bold(level_text(memory, Subsystem::Memory)),
        ),
        Span::styled(
            format!(
                "  {} of {}",
                units::bytes(sample.memory.used),
                units::bytes(sample.memory.total)
            ),
            style::fg(DIM),
        ),
    ];
    if sample.memory.swap_total > 0 {
        memory_facts.push(Span::styled(
            format!(
                " · swap {}",
                units::percent(model::fraction(
                    sample.memory.swap_used,
                    sample.memory.swap_total
                ))
                .trim()
            ),
            style::fg(DIM),
        ));
    }
    cursor.line(frame, section("Memory", memory_facts, width));
    let memory_history = machine.memory.tail(window);
    plot(
        &mut cursor,
        frame,
        graphics,
        Plot {
            rows: memory_rows,
            slot: "memory",
            chart: Chart {
                kind: Kind::Area,
                max: 1.0,
                series: vec![Series {
                    values: &memory_history,
                    color: TEAL,
                }],
            },
            scale: percent_scale(),
            shown: Some(span(drawn(memory_history.len()), machine.interval)),
        },
    );

    cursor.line(
        frame,
        section(
            "Network",
            vec![
                Span::styled(
                    format!("↓ download {}", units::rate(sample.network.rx_per_sec)),
                    style::fg(ACCENT),
                ),
                Span::styled(
                    format!("   ↑ upload {}", units::rate(sample.network.tx_per_sec)),
                    style::fg(GOOD),
                ),
            ],
            width,
        ),
    );
    let rx = machine.rx.tail(window);
    let tx = machine.tx.tail(window);
    let peak = rx.iter().chain(&tx).copied().fold(1.0, f32::max) * 1.1;
    plot(
        &mut cursor,
        frame,
        graphics,
        Plot {
            rows: network_rows,
            slot: "network",
            chart: Chart {
                kind: Kind::Area,
                max: peak,
                series: vec![
                    Series {
                        values: &rx,
                        color: ACCENT,
                    },
                    Series {
                        values: &tx,
                        color: GOOD,
                    },
                ],
            },
            scale: Scale {
                top: units::rate(peak as u64),
                middle: Some(units::rate((peak / 2.0) as u64)),
                bottom: "0".into(),
            },
            shown: Some(span(drawn(rx.len()), machine.interval)),
        },
    );

    cursor.line(
        frame,
        section(
            "Storage",
            vec![Span::styled(
                format!(
                    "read {} · write {}",
                    units::rate(sample.disk.read_per_sec),
                    units::rate(sample.disk.write_per_sec)
                ),
                style::fg(DIM),
            )],
            width,
        ),
    );
    for fs in disks {
        let used = model::fraction(fs.used, fs.total);
        let label_width = (width / 3).clamp(4, 28);
        let size = format!(
            "  {:>5}  {} of {}",
            units::percent(used).trim(),
            units::bytes(fs.used),
            units::bytes(fs.total)
        );
        let bar = width
            .saturating_sub(label_width + 1 + size.chars().count())
            .min(24);
        let mut line = Composed::of(vec![Span::styled(
            pad(&truncate_left(&fs.mount, label_width), label_width + 1),
            style::fg(DIM),
        )]);
        line.bar(
            bar,
            used,
            style::level(model::grade(Subsystem::Storage, used)),
            format!("disk:{}", fs.mount),
        );
        line.push(Span::styled(size, style::fg(TEXT)));
        cursor.composed(frame, graphics, line);
    }
}

struct Scale {
    top: String,
    middle: Option<String>,
    bottom: String,
}

struct Plot<'a> {
    rows: u16,
    slot: &'static str,
    chart: Chart<'a>,
    scale: Scale,
    shown: Option<Duration>,
}

fn plot(cursor: &mut Cursor, frame: &mut Frame, graphics: &mut Graphics, plot: Plot) {
    let Plot {
        rows,
        slot,
        chart,
        scale,
        shown,
    } = plot;
    let rows = rows.min(cursor.left());
    if rows == 0 {
        return;
    }
    let gutter = GUTTER.min(cursor.area.width.saturating_sub(1));
    let label = |text: &str| {
        Line::from(Span::styled(
            format!("{text:>width$} ", width = gutter.saturating_sub(1) as usize),
            style::fg(FAINT),
        ))
    };
    let top = cursor.y;
    let bottom = top + rows - 1;
    let mut place = |y: u16, text: &str| {
        frame.render_widget(
            Paragraph::new(label(text)),
            Rect::new(cursor.area.x, y, gutter, 1),
        );
    };
    place(top, &scale.top);
    if let Some(middle) = scale.middle.filter(|_| rows >= 5 && rows % 2 == 1) {
        place(top + rows / 2, &middle);
    }
    if bottom > top {
        place(bottom, &scale.bottom);
    }
    graphics.chart(
        frame.buffer_mut(),
        Rect::new(
            cursor.area.x + gutter,
            top,
            cursor.area.width - gutter,
            rows,
        ),
        slot.to_string(),
        &chart,
    );
    cursor.y += rows;
    if let Some(shown) = shown {
        let width = (cursor.area.width - gutter) as usize;
        cursor.line(
            frame,
            Line::from(vec![
                Span::raw(" ".repeat(gutter as usize)),
                Span::styled(
                    pad(
                        &format!("{} ago", units::duration(shown)),
                        width.saturating_sub(3),
                    ),
                    style::fg(FAINT),
                ),
                Span::styled("now", style::fg(FAINT)),
            ]),
        );
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
    let mut details = vec![
        machine.identity.os.clone(),
        machine.identity.cpu_model.clone(),
    ];
    if let Some(sample) = &machine.latest {
        details.push(format!("{} cores", sample.cores.len()));
        details.push(units::bytes(sample.memory.total));
        details.push(format!(
            "up {}",
            units::duration(Duration::from_secs(sample.uptime_secs))
        ));
    }
    vec![Span::styled(
        details
            .into_iter()
            .filter(|detail| !detail.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        style::fg(DIM),
    )]
}

fn section(title: &str, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    super::spread(
        vec![Span::styled(title.to_string(), style::bold(TEXT))],
        right,
        width,
    )
}

fn truncate_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    let tail: String = text.chars().skip(len - width.saturating_sub(1)).collect();
    format!("…{tail}")
}
