use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::alerts::{Alerts, Event, Severity};
use super::chart;
use super::model::{
    self, AgentRow, Level, LinkState, Machine, Model, Presence, Row, Strain, Subsystem,
};
use super::style::{self, ACCENT, CRIT, DIM, FAINT, GOOD, SELECTED, SEND, TEXT, WARN};
use super::units;
use crate::log;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::proto::{Agent, AgentSession, FULL, Plan, Sample, SessionState, Waiting};

const WIDE: u16 = 100;
const PHONE: u16 = 60;
const GAP: usize = 2;
const LABEL: usize = 9;

pub fn render(frame: &mut Frame, model: &Model, now: Instant) {
    let area = frame.area().inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    if area.width < 28 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("magi · enlarge the pane").style(style::fg(DIM)),
            frame.area(),
        );
        return;
    }
    let rows = model.rows(now);
    let agents = model.agents(now);
    let strains = Model::strains(&rows);
    let room = area.height.saturating_sub(4 + 9);
    let machines_height = (rows.len() as u16 + 1).min(room / 2).max(2);
    let agents_height = (agents.len().max(1) as u16 + 1)
        .min(room.saturating_sub(machines_height))
        .max(2);
    let [header, _, machines, _, agent_list, rule, detail, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(machines_height),
        Constraint::Length(1),
        Constraint::Length(agents_height),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(frame, header, &rows, &agents, &strains);
    render_table(frame, machines, model, &rows, now);
    render_agents(frame, agent_list, model, &agents, rows.len(), now);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(rule.width as usize),
            style::fg(FAINT),
        )),
        rule,
    );
    let plans = model.plans(now);
    match rows.get(model.selected) {
        _ if model.showing_alerts => render_alerts(frame, detail, &model.alerts),
        Some(row) => render_detail(frame, detail, Some(row), model.port, now),
        None => render_agent_detail(
            frame,
            detail,
            agents.get(model.selected - rows.len()),
            &plans,
        ),
    }
    let problem = model
        .topology
        .problem
        .clone()
        .or_else(|| model.alerts.problem().map(str::to_string))
        .or_else(|| model.notices(now).into_iter().next());
    render_footer(
        frame,
        footer,
        model.alerts.active().count(),
        problem.as_deref(),
    );
}

fn render_header(
    frame: &mut Frame,
    area: Rect,
    rows: &[Row<'_>],
    agents: &[AgentRow<'_>],
    strains: &[Strain; 3],
) {
    let lost = rows
        .iter()
        .filter(|row| {
            matches!(
                row.state(),
                Some(LinkState::Lost { .. } | LinkState::Rejected(_))
            )
        })
        .count();
    let machines = rows.len();
    let mut left = vec![
        Span::styled("magi", style::bold(ACCENT)),
        Span::styled(
            format!(
                "   {machines} {}",
                if machines == 1 { "machine" } else { "machines" }
            ),
            style::fg(DIM),
        ),
    ];
    if lost > 0 {
        left.push(Span::styled(" · ", style::fg(FAINT)));
        left.push(Span::styled(format!("{lost} lost"), style::fg(CRIT)));
    }
    if !agents.is_empty() {
        left.push(Span::styled(
            format!(
                " · {} {}",
                agents.len(),
                if agents.len() == 1 { "agent" } else { "agents" }
            ),
            style::fg(DIM),
        ));
    }
    let waiting = agents
        .iter()
        .filter(|agent| agent.live && matches!(agent.session.state, SessionState::Waiting(_)))
        .count();
    if waiting > 0 {
        left.push(Span::styled(" · ", style::fg(FAINT)));
        left.push(Span::styled(
            format!(
                "{waiting} {} you",
                if waiting == 1 { "needs" } else { "need" }
            ),
            style::bold(WARN),
        ));
    }
    let clock = log::clock();
    let mut right = health(strains, rows);
    right.push(Span::styled(format!("   {}", &clock[..5]), style::fg(DIM)));
    let width = |spans: &[Span<'_>]| {
        spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum::<usize>()
    };
    if width(&left) + width(&right) + 2 > area.width as usize {
        right.retain(|span| span.content.trim_start().len() == 5);
    }
    if width(&left) + width(&right) + 1 > area.width as usize {
        left.retain(|span| !span.content.contains(" agent"));
    }
    let left = clip(
        left,
        (area.width as usize).saturating_sub(width(&right) + 1),
    );
    frame.render_widget(Paragraph::new(Line::from(left)), area);
    frame.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        area,
    );
}

fn health(strains: &[Strain; 3], rows: &[Row<'_>]) -> Vec<Span<'static>> {
    if !rows.iter().any(|row| row.state() == Some(&LinkState::Live)) {
        return Vec::new();
    }
    let worst = strains
        .iter()
        .filter(|strain| strain.level != Level::Normal)
        .max_by(|a, b| {
            a.level
                .partial_cmp(&b.level)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.worst.total_cmp(&b.worst))
        });
    match worst.and_then(|strain| strain.culprit.as_deref().map(|culprit| (strain, culprit))) {
        Some((strain, culprit)) => {
            let word = if strain.level == Level::Critical {
                "critical"
            } else {
                "high"
            };
            vec![Span::styled(
                format!(
                    "{culprit} {} {word} · {}",
                    strain.subsystem.name(),
                    units::percent(strain.worst).trim()
                ),
                style::fg(style::level(strain.level)),
            )]
        }
        None => vec![Span::styled("all normal", style::fg(GOOD))],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Column {
    Cpu,
    Memory,
    Recent,
    Disk,
    Network,
    Uptime,
}

impl Column {
    const PRIORITY: [Column; 6] = [
        Column::Cpu,
        Column::Memory,
        Column::Disk,
        Column::Network,
        Column::Recent,
        Column::Uptime,
    ];

    fn label(self, compact: bool) -> &'static str {
        match self {
            Column::Cpu => "cpu",
            Column::Memory if compact => "mem",
            Column::Memory => "memory",
            Column::Recent => "recent",
            Column::Disk => "disk",
            Column::Network => "network",
            Column::Uptime => "up",
        }
    }
}

struct TablePlan {
    columns: Vec<Column>,
    meter: usize,
}

impl TablePlan {
    fn width(&self, column: Column) -> usize {
        match column {
            Column::Cpu | Column::Memory => self.meter + usize::from(self.meter > 0) + 4,
            Column::Recent => 16,
            Column::Disk => 4,
            Column::Network => 17,
            Column::Uptime => 7,
        }
    }

    fn fit(available: usize) -> Self {
        let meter = if available >= 64 {
            10
        } else if available >= 34 {
            6
        } else {
            0
        };
        let mut plan = TablePlan {
            columns: Vec::new(),
            meter,
        };
        let mut used = 0;
        for column in Column::PRIORITY {
            let width = plan.width(column) + GAP;
            if used + width > available {
                break;
            }
            used += width;
            plan.columns.push(column);
        }
        plan.columns.sort();
        plan
    }
}

fn render_table(frame: &mut Frame, area: Rect, model: &Model, rows: &[Row<'_>], now: Instant) {
    let longest = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(8);
    let name_width = longest
        .clamp(6, 24)
        .min((area.width as usize * 2 / 5).max(6));
    let lead = 2 + name_width + GAP;
    let plan = TablePlan::fit((area.width as usize).saturating_sub(lead));
    let visible = (area.height as usize).saturating_sub(1);
    let first = if model.selected < rows.len() {
        model.selected.saturating_sub(visible.saturating_sub(1))
    } else {
        0
    };

    let mut header = vec![
        Span::raw(" ".repeat(2)),
        Span::styled(pad("machine", name_width + GAP), style::fg(FAINT)),
    ];
    for column in &plan.columns {
        let width = plan.width(*column);
        let text = column.label(plan.meter == 0);
        let label = if matches!(column, Column::Disk | Column::Uptime) {
            pad_left(text, width)
        } else {
            pad(text, width)
        };
        header.push(Span::styled(
            format!("{label}{}", " ".repeat(GAP)),
            style::fg(FAINT),
        ));
    }
    let mut lines = vec![Line::from(header)];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  waiting for this machine's agent…",
            style::fg(DIM),
        )));
    }
    for (index, row) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = index == model.selected;
        let mut spans = vec![
            Span::styled(if selected { "▎ " } else { "  " }, style::fg(ACCENT)),
            Span::styled(
                pad(&truncate(&row.name, name_width), name_width + GAP),
                if selected {
                    style::bold(TEXT)
                } else {
                    style::fg(TEXT)
                },
            ),
        ];
        spans.extend(table_cells(
            row,
            &plan,
            model.port,
            now,
            (area.width as usize).saturating_sub(lead),
        ));
        let line = Line::from(spans);
        lines.push(if selected {
            line.style(Style::new().bg(SELECTED))
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn table_cells(
    row: &Row<'_>,
    plan: &TablePlan,
    port: u16,
    now: Instant,
    room: usize,
) -> Vec<Span<'static>> {
    let live = row.state() == Some(&LinkState::Live);
    let (Some(machine), true) = (row.machine(), live) else {
        return vec![Span::styled(
            truncate(&status(row, port, now), room),
            style::fg(status_color(row)),
        )];
    };
    let Some(sample) = &machine.latest else {
        return vec![Span::styled("connecting…", style::fg(DIM))];
    };
    let gap = || Span::raw(" ".repeat(GAP));
    let mut spans = Vec::new();
    for column in &plan.columns {
        match column {
            Column::Cpu => spans.extend(gauge(
                machine.smoothed_cpu(),
                plan.meter,
                style::load(machine.smoothed_cpu()),
            )),
            Column::Memory => {
                let memory = machine.memory_fraction();
                spans.extend(gauge(
                    memory,
                    plan.meter,
                    style::level(model::grade(Subsystem::Memory, memory)),
                ));
            }
            Column::Recent => {
                let samples = plan.width(Column::Recent);
                spans.push(Span::styled(
                    chart::spark(&machine.cpu.tail(samples), samples),
                    style::fg(ACCENT),
                ));
            }
            Column::Disk => {
                let disk = machine.fullest_filesystem();
                let color = match model::grade(Subsystem::Storage, disk) {
                    Level::Normal => TEXT,
                    level => style::level(level),
                };
                spans.push(Span::styled(units::percent(disk), style::fg(color)));
            }
            Column::Network => spans.push(Span::styled(
                pad(
                    &format!(
                        "↓{} ↑{}",
                        units::rate(sample.network.rx_per_sec),
                        units::rate(sample.network.tx_per_sec)
                    ),
                    plan.width(Column::Network),
                ),
                style::fg(DIM),
            )),
            Column::Uptime => spans.push(Span::styled(
                pad_left(
                    &units::duration(Duration::from_secs(sample.uptime_secs)),
                    plan.width(Column::Uptime),
                ),
                style::fg(DIM),
            )),
        }
        spans.push(gap());
    }
    spans
}

fn gauge(fraction: f32, width: usize, color: Color) -> Vec<Span<'static>> {
    let percent = Span::styled(units::percent(fraction), style::fg(TEXT));
    if width == 0 {
        return vec![percent];
    }
    let (filled, track) = chart::meter(fraction, width);
    vec![
        Span::styled(filled, style::fg(color)),
        Span::styled(track, style::fg(FAINT)),
        Span::raw(" "),
        percent,
    ]
}

fn status(row: &Row<'_>, port: u16, now: Instant) -> String {
    match &row.presence {
        Presence::Searching => "looking for its agent…".into(),
        Presence::NoAgent => format!("no agent on port {port} here · run `magi agent install`"),
        Presence::Agent(link) => match &link.state {
            LinkState::Lost { since, reason } => format!(
                "lost {} · {reason} · retrying",
                units::duration(now.duration_since(*since))
            ),
            LinkState::Rejected(reason) => format!("refused: {reason}"),
            LinkState::Connecting | LinkState::Live => "connecting…".into(),
        },
    }
}

fn status_color(row: &Row<'_>) -> Color {
    match row.state() {
        Some(LinkState::Lost { .. } | LinkState::Rejected(_)) => CRIT,
        _ => DIM,
    }
}

fn role(row: &Row<'_>) -> &'static str {
    if row.here {
        "this machine"
    } else {
        "on your tailnet"
    }
}

fn render_detail(frame: &mut Frame, area: Rect, row: Option<&Row<'_>>, port: u16, now: Instant) {
    let Some(row) = row else { return };
    if area.height < 3 {
        return;
    }
    let [title, notice, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let Some(machine) = row.machine() else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(row.name.clone(), style::bold(TEXT)),
                Span::styled(format!("   {}", role(row)), style::fg(DIM)),
            ])),
            title,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                truncate(&status(row, port, now), notice.width as usize),
                style::fg(status_color(row)),
            )),
            notice,
        );
        return;
    };
    let mut facts = vec![
        Span::styled(row.name.clone(), style::bold(TEXT)),
        Span::styled(format!("   {}", role(row)), style::fg(DIM)),
    ];
    let mut details = vec![
        machine.identity.os.clone(),
        machine.identity.cpu_model.clone(),
    ];
    if let Some(sample) = &machine.latest {
        details.push(format!("{} cores", sample.cores.len()));
        details.push(format!(
            "up {}",
            units::duration(Duration::from_secs(sample.uptime_secs))
        ));
    }
    facts.push(Span::styled(
        format!(
            " · {}",
            details
                .into_iter()
                .filter(|detail| !detail.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        ),
        style::fg(FAINT),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(clip(facts, title.width as usize))),
        title,
    );

    let muted = row.state() != Some(&LinkState::Live);
    if muted {
        frame.render_widget(
            Paragraph::new(Span::styled(
                truncate(&status(row, port, now), notice.width as usize),
                style::fg(status_color(row)),
            )),
            notice,
        );
    }
    let Some(sample) = &machine.latest else {
        return;
    };
    let paint = Paint { muted };
    if body.width >= WIDE {
        let [left, _, right] = Layout::horizontal([
            Constraint::Percentage(52),
            Constraint::Length(4),
            Constraint::Min(0),
        ])
        .areas(body);
        render_cpu(
            frame,
            left,
            machine,
            sample,
            &paint,
            left.height.saturating_sub(3).clamp(3, 14) as usize,
        );
        render_resources(frame, right, machine, sample, &paint);
    } else {
        let chart_height: usize = if body.width < PHONE { 4 } else { 5 };
        let cpu_height = chart_height as u16 + 2;
        let [top, _, bottom] = Layout::vertical([
            Constraint::Length(cpu_height),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(body);
        render_cpu(frame, top, machine, sample, &paint, chart_height);
        render_resources(frame, bottom, machine, sample, &paint);
    }
}

struct Paint {
    muted: bool,
}

impl Paint {
    fn fg(&self, color: Color) -> Style {
        style::fg(if self.muted { FAINT } else { color })
    }

    fn bold(&self, color: Color) -> Style {
        if self.muted {
            style::fg(FAINT)
        } else {
            style::bold(color)
        }
    }
}

fn render_cpu(
    frame: &mut Frame,
    area: Rect,
    machine: &Machine,
    sample: &Sample,
    paint: &Paint,
    chart_height: usize,
) {
    const AXIS: usize = 5;
    let width = (area.width as usize).saturating_sub(AXIS);
    if width < 4 || area.height < 2 {
        return;
    }
    let cpu = machine.smoothed_cpu();
    let window = machine.interval * width as u32;
    let mut lines = vec![Line::from(vec![
        Span::styled(pad("cpu", LABEL), paint.fg(DIM)),
        Span::styled(
            units::percent(cpu).trim().to_string(),
            paint.bold(style::load(cpu)),
        ),
        Span::styled(
            format!(
                "   avg {} · peak {} over {}   load {:.2}",
                units::percent(machine.cpu.recent_mean(width)).trim(),
                units::percent(machine.cpu.recent_max(width)).trim(),
                units::duration(window),
                sample.load[0]
            ),
            paint.fg(DIM),
        ),
    ])];
    let chart_height = chart_height
        .min((area.height as usize).saturating_sub(2))
        .max(1);
    for (index, row) in chart::columns(&machine.cpu.tail(width), width, chart_height)
        .into_iter()
        .enumerate()
    {
        let band = (chart_height - index) as f32 / chart_height as f32;
        let axis = if index == 0 {
            "100%"
        } else if index == chart_height / 2 && chart_height > 2 {
            " 50%"
        } else if index == chart_height - 1 {
            "  0%"
        } else {
            ""
        };
        let midline = index == chart_height / 2 && chart_height > 2;
        let mut spans = vec![Span::styled(pad(axis, AXIS), style::fg(FAINT))];
        let cells: String = row
            .iter()
            .map(|cell| cell.unwrap_or(if midline { '┈' } else { ' ' }))
            .collect();
        spans.push(Span::styled(cells, paint.fg(style::load(band))));
        lines.push(Line::from(spans));
    }
    let core_width = width.saturating_sub(2);
    let spaced = sample.cores.len() * 2 <= core_width;
    let mut cores = vec![Span::styled(pad("", AXIS), style::fg(FAINT))];
    for core in sample
        .cores
        .iter()
        .take(if spaced { core_width / 2 } else { core_width })
    {
        let fraction = f32::from(*core) / f32::from(FULL);
        cores.push(Span::styled(
            format!(
                "{}{}",
                chart::level(fraction),
                if spaced { " " } else { "" }
            ),
            paint.fg(style::load(fraction)),
        ));
    }
    cores.push(Span::styled(
        format!(" {} cores", sample.cores.len()),
        style::fg(FAINT),
    ));
    lines.push(Line::from(cores));
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_resources(
    frame: &mut Frame,
    area: Rect,
    machine: &Machine,
    sample: &Sample,
    paint: &Paint,
) {
    let width = area.width as usize;
    if width < 24 || area.height == 0 {
        return;
    }
    let detail_width = 15;
    let meter_width = width
        .saturating_sub(LABEL + 5 + detail_width + 2)
        .clamp(4, 30);
    let mut lines = Vec::new();

    let memory = &sample.memory;
    lines.push(meter_line(
        "memory",
        "",
        model::fraction(memory.used, memory.total),
        meter_width,
        &of(memory.used, memory.total),
        Subsystem::Memory,
        paint,
    ));
    if memory.swap_total > 0 {
        lines.push(meter_line(
            "swap",
            "",
            model::fraction(memory.swap_used, memory.swap_total),
            meter_width,
            &of(memory.swap_used, memory.swap_total),
            Subsystem::Memory,
            paint,
        ));
    }
    lines.push(Line::default());

    let mount_width = sample
        .filesystems
        .iter()
        .map(|fs| fs.mount.chars().count())
        .max()
        .unwrap_or(1)
        .clamp(1, 12);
    let fs_meter = meter_width.saturating_sub(mount_width + 1).max(4);
    for (index, filesystem) in sample.filesystems.iter().enumerate() {
        let label = if index == 0 { "storage" } else { "" };
        let mount = pad(
            &truncate_left(&filesystem.mount, mount_width),
            mount_width + 1,
        );
        let fraction = model::fraction(filesystem.used, filesystem.total);
        lines.push(meter_line(
            label,
            &mount,
            fraction,
            fs_meter,
            &of(filesystem.used, filesystem.total),
            Subsystem::Storage,
            paint,
        ));
    }
    lines.push(Line::default());

    let spark_width = width.saturating_sub(LABEL + 12);
    let scale = machine
        .rx
        .recent_max(spark_width)
        .max(machine.tx.recent_max(spark_width))
        .max(1024.0);
    let normalized = |values: Vec<f32>| {
        values
            .into_iter()
            .map(|value| value / scale)
            .collect::<Vec<f32>>()
    };
    lines.push(Line::from(vec![
        Span::styled(pad("network", LABEL), paint.fg(DIM)),
        Span::styled(
            pad(&format!("↓ {}", units::rate(sample.network.rx_per_sec)), 12),
            paint.fg(TEXT),
        ),
        Span::styled(
            chart::spark(&normalized(machine.rx.tail(spark_width)), spark_width),
            paint.fg(ACCENT),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(LABEL)),
        Span::styled(
            pad(&format!("↑ {}", units::rate(sample.network.tx_per_sec)), 12),
            paint.fg(TEXT),
        ),
        Span::styled(
            chart::spark(&normalized(machine.tx.tail(spark_width)), spark_width),
            paint.fg(SEND),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(pad("disk i/o", LABEL), paint.fg(DIM)),
        Span::styled(
            format!(
                "read {} · write {}",
                units::rate(sample.disk.read_per_sec),
                units::rate(sample.disk.write_per_sec)
            ),
            paint.fg(TEXT),
        ),
    ]));
    frame.render_widget(Paragraph::new(lines), area);
}

fn meter_line(
    label: &str,
    mount: &str,
    fraction: f32,
    width: usize,
    detail: &str,
    subsystem: Subsystem,
    paint: &Paint,
) -> Line<'static> {
    let (filled, track) = chart::meter(fraction, width);
    Line::from(vec![
        Span::styled(pad(label, LABEL), paint.fg(DIM)),
        Span::styled(mount.to_string(), paint.fg(TEXT)),
        Span::styled(
            filled,
            paint.fg(style::level(model::grade(subsystem, fraction))),
        ),
        Span::styled(track, style::fg(FAINT)),
        Span::styled(format!(" {} ", units::percent(fraction)), paint.fg(TEXT)),
        Span::styled(detail.to_string(), paint.fg(DIM)),
    ])
}

fn of(used: u64, total: u64) -> String {
    format!("{} of {}", units::bytes(used), units::bytes(total))
}

fn render_agents(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    agents: &[AgentRow<'_>],
    offset: usize,
    now: Instant,
) {
    let width = area.width as usize;
    let mut title = vec![Span::styled("  agents", style::fg(FAINT))];
    let plans = model.plans(now);
    let named = plans.len() > 1;
    for plan in plans {
        title.push(Span::styled(
            format!("   {}", plan_text(plan, width < WIDE as usize, named)),
            style::fg(DIM),
        ));
    }
    let mut lines = vec![Line::from(clip(title, width))];
    if agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no coding agents running · Claude Code and Codex sessions show up here",
            style::fg(DIM),
        )));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let machine_width = agents
        .iter()
        .map(|agent| agent.machine.chars().count())
        .max()
        .unwrap_or(8)
        .min(20);
    let show_machine = width >= 70;
    let show_details = width >= 96;
    let state_width = if width < 70 { 16 } else { 24 };
    let fixed = 2
        + GAP
        + state_width
        + GAP
        + 9
        + if show_machine { machine_width + GAP } else { 0 }
        + if show_details { 9 + 6 + 6 + 3 * GAP } else { 0 };
    let title_width = width.saturating_sub(fixed).clamp(10, 40);
    let visible = (area.height as usize).saturating_sub(1);
    let selected = model.selected.checked_sub(offset);
    let first = selected
        .unwrap_or(0)
        .saturating_sub(visible.saturating_sub(1));
    for (index, agent) in agents.iter().enumerate().skip(first).take(visible) {
        let is_selected = selected == Some(index);
        let session = agent.session;
        let (state, color) = session_state(session, agent.live);
        let mut spans = vec![
            Span::styled(if is_selected { "▎ " } else { "  " }, style::fg(ACCENT)),
            Span::styled(
                pad(&truncate(&session.title, title_width), title_width + GAP),
                if is_selected {
                    style::bold(TEXT)
                } else {
                    style::fg(TEXT)
                },
            ),
            Span::styled(
                pad(&truncate(&state, state_width), state_width + GAP),
                if matches!(session.state, SessionState::Waiting(_)) && agent.live {
                    style::bold(color)
                } else {
                    style::fg(color)
                },
            ),
        ];
        if show_machine {
            spans.push(Span::styled(
                pad(
                    &truncate(&agent.machine, machine_width),
                    machine_width + GAP,
                ),
                style::fg(DIM),
            ));
        }
        if show_details {
            spans.push(Span::styled(
                pad(&units::model(&session.model), 9 + GAP),
                style::fg(DIM),
            ));
            spans.push(Span::styled(
                pad_left(&context_text(session), 6),
                style::fg(DIM),
            ));
            spans.push(Span::raw(" ".repeat(GAP)));
            spans.push(Span::styled(
                pad_left(
                    &units::duration(Duration::from_secs(u64::from(session.age_secs))),
                    6,
                ),
                style::fg(DIM),
            ));
            spans.push(Span::raw(" ".repeat(GAP)));
        }
        spans.push(Span::styled(pad_left(&cost(session), 9), style::fg(TEXT)));
        let line = Line::from(clip(spans, width));
        lines.push(if is_selected {
            line.style(Style::new().bg(SELECTED))
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn session_state(session: &AgentSession, live: bool) -> (String, Color) {
    if !live {
        return ("machine lost".into(), FAINT);
    }
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    match &session.state {
        SessionState::Waiting(Waiting::Permission(Some(tool))) => {
            (format!("needs you · {tool}"), WARN)
        }
        SessionState::Waiting(Waiting::Permission(None)) => ("needs you · approve".into(), WARN),
        SessionState::Waiting(Waiting::Input(reason)) => (format!("needs you · {reason}"), WARN),
        SessionState::Working { tool: Some(tool) } => (format!("working · {tool}"), GOOD),
        SessionState::Working { tool: None } => ("working".into(), GOOD),
        SessionState::Idle => (format!("idle {since}"), DIM),
    }
}

fn cost(session: &AgentSession) -> String {
    let dollars = units::dollars(session.cost_micros);
    if session.cost_complete {
        dollars
    } else {
        format!("≥{dollars}")
    }
}

fn context_text(session: &AgentSession) -> String {
    match session.context_used {
        Some(used) => units::percent(f32::from(used) / f32::from(FULL))
            .trim()
            .to_string(),
        None => units::count(session.context_tokens),
    }
}

fn plan_text(plan: &Plan, compact: bool, named: bool) -> String {
    let windows = plan
        .windows
        .iter()
        .map(|window| {
            let used = units::percent(f32::from(window.used) / f32::from(FULL))
                .trim()
                .to_string();
            if compact {
                format!("{} {used}", window.label())
            } else {
                format!(
                    "{} {used} · resets in {}",
                    window.long_label(),
                    until(window.resets_at)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(if compact { " · " } else { "   " });
    if named {
        format!("{} {windows}", short_name(plan.agent))
    } else {
        windows
    }
}

fn short_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude",
        Agent::Codex => "Codex",
    }
}

fn until(epoch_secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    units::duration(Duration::from_secs(epoch_secs.saturating_sub(now)))
}

fn render_agent_detail(
    frame: &mut Frame,
    area: Rect,
    agent: Option<&AgentRow<'_>>,
    plans: &[&Plan],
) {
    let Some(agent) = agent else { return };
    let session = agent.session;
    let width = area.width as usize;
    let (state, color) = session_state(session, agent.live);
    let since = units::duration(Duration::from_secs(u64::from(session.state_secs)));
    let title = vec![
        Span::styled(session.title.clone(), style::bold(TEXT)),
        Span::styled(
            format!(
                "   {} · {} on {}",
                session.project,
                session.agent.name(),
                agent.machine
            ),
            style::fg(DIM),
        ),
    ];
    let notice = match session.state {
        SessionState::Idle => state.clone(),
        _ => format!("{state} · for {since}"),
    };
    let tokens = session.tokens;
    let mut lines = vec![
        Line::from(clip(title, width)),
        Line::from(Span::styled(
            notice,
            if matches!(session.state, SessionState::Waiting(_)) {
                style::bold(color)
            } else {
                style::fg(color)
            },
        )),
        Line::default(),
        labelled(
            "model",
            vec![Span::styled(units::model(&session.model), style::fg(TEXT))],
        ),
    ];
    let context = match session.context_used {
        Some(used) => {
            let fraction = f32::from(used) / f32::from(FULL);
            let (filled, track) = chart::meter(fraction, 20);
            vec![
                Span::styled(filled, style::fg(style::load(fraction))),
                Span::styled(track, style::fg(FAINT)),
                Span::styled(
                    format!(
                        " {}  {} tokens",
                        units::percent(fraction).trim(),
                        units::count(session.context_tokens)
                    ),
                    style::fg(TEXT),
                ),
            ]
        }
        None => vec![Span::styled(
            format!("{} tokens", units::count(session.context_tokens)),
            style::fg(TEXT),
        )],
    };
    lines.push(labelled("context", context));
    lines.push(labelled(
        "tokens",
        vec![Span::styled(
            format!(
                "in {} · out {} · cache read {} · cache write {}",
                units::count(tokens.input),
                units::count(tokens.output),
                units::count(tokens.cache_read),
                units::count(tokens.cache_write)
            ),
            style::fg(TEXT),
        )],
    ));
    let cost = vec![
        Span::styled(cost(session), style::bold(TEXT)),
        Span::styled(
            if session.cost_complete {
                "   at API list prices since it started, subagents included"
            } else {
                "   so far; a model without a known price is left out"
            },
            style::fg(FAINT),
        ),
    ];
    lines.push(labelled("cost", cost));
    lines.push(labelled(
        "running",
        vec![Span::styled(
            units::duration(Duration::from_secs(u64::from(session.age_secs))),
            style::fg(TEXT),
        )],
    ));
    if let Some(plan) = plans.iter().find(|plan| plan.agent == session.agent) {
        lines.push(Line::default());
        for window in &plan.windows {
            let label = window.long_label();
            let fraction = f32::from(window.used) / f32::from(FULL);
            let (filled, track) = chart::meter(fraction, 20);
            lines.push(labelled(
                &label,
                vec![
                    Span::styled(filled, style::fg(style::load(fraction))),
                    Span::styled(track, style::fg(FAINT)),
                    Span::styled(
                        format!(
                            " {}  resets in {}",
                            units::percent(fraction).trim(),
                            until(window.resets_at)
                        ),
                        style::fg(TEXT),
                    ),
                ],
            ));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!("session {}", session.id),
        style::fg(FAINT),
    )));
    let lines: Vec<Line<'static>> = lines
        .into_iter()
        .map(|line| Line::from(clip(line.spans, width)))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn labelled(label: &str, mut value: Vec<Span<'static>>) -> Line<'static> {
    value.insert(0, Span::styled(pad(label, LABEL), style::fg(DIM)));
    Line::from(value)
}

fn render_footer(frame: &mut Frame, area: Rect, active_alerts: usize, problem: Option<&str>) {
    let mut keys = vec![Span::styled("↑↓ select   a alerts", style::fg(FAINT))];
    if active_alerts > 0 {
        keys.push(Span::styled(
            format!(" ({active_alerts})"),
            style::bold(WARN),
        ));
    }
    keys.push(Span::styled("   q quit", style::fg(FAINT)));
    let used = keys
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    frame.render_widget(Paragraph::new(Line::from(keys)), area);
    if let Some(problem) = problem {
        let room = (area.width as usize).saturating_sub(used + 2);
        frame.render_widget(
            Paragraph::new(Span::styled(truncate(problem, room), style::fg(WARN)))
                .alignment(Alignment::Right),
            area,
        );
    }
}

fn render_alerts(frame: &mut Frame, area: Rect, alerts: &Alerts) {
    let width = area.width as usize;
    let active = alerts.active().count();
    let mut title = vec![
        Span::styled("alerts", style::bold(TEXT)),
        Span::styled(
            format!("   {active} active"),
            if active > 0 {
                style::bold(WARN)
            } else {
                style::fg(DIM)
            },
        ),
    ];
    if let Some(path) = alerts.path() {
        title.push(Span::styled(
            format!("   history in {}", home_relative(path)),
            style::fg(FAINT),
        ));
    }
    let mut lines = vec![Line::from(clip(title, width))];
    let today = log::local_time(log::now_secs(), c"%Y-%m-%d");
    let mut history = alerts.history().peekable();
    if history.peek().is_none() {
        lines.push(Line::from(Span::styled(
            "no alerts yet · one is raised when an agent needs you, a machine stops answering or stays critical, or a plan passes 90%",
            style::fg(DIM),
        )));
    }
    for entry in history.take((area.height as usize).saturating_sub(1)) {
        let when = if log::local_time(entry.at, c"%Y-%m-%d") == today {
            log::local_time(entry.at, c"%H:%M")
        } else {
            log::local_time(entry.at, c"%b %d %H:%M")
        };
        let (text, color) = match entry.event {
            Event::Raised => (
                entry.text.clone(),
                match entry.severity {
                    Severity::Error => CRIT,
                    Severity::Permission | Severity::Question => WARN,
                },
            ),
            Event::Resolved => (format!("resolved · {}", entry.text), DIM),
        };
        lines.push(Line::from(clip(
            vec![
                Span::styled(pad_left(&when, 12), style::fg(FAINT)),
                Span::raw("  "),
                Span::styled(text, style::fg(color)),
            ],
            width,
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn home_relative(path: &std::path::Path) -> String {
    let shown = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if shown.starts_with(&home) => format!("~{}", &shown[home.len()..]),
        _ => shown,
    }
}

fn clip(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let mut room = width;
    let mut clipped = Vec::new();
    for span in spans {
        let len = span.content.chars().count();
        if len <= room {
            room -= len;
            clipped.push(span);
            continue;
        }
        if room > 0 {
            clipped.push(Span::styled(truncate(&span.content, room), span.style));
        }
        break;
    }
    clipped
}

fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        truncate(text, width)
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

fn pad_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        truncate(text, width)
    } else {
        format!("{}{text}", " ".repeat(width - len))
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn truncate_left(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    let tail: String = text.chars().skip(len - width.saturating_sub(1)).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::proto::{
        AgentSession, DiskIo, FIVE_HOURS, Filesystem, Identity, Memory, NetIo, ONE_WEEK, Plan,
        SessionState, Tokens, Waiting, Welcome, Window,
    };
    use crate::viewer::model::{Endpoint, Link, LinkUpdate};

    fn welcome(node: &str) -> LinkUpdate {
        LinkUpdate::Welcome(Box::new(Welcome {
            interval_ms: 1000,
            identity: Identity {
                hostname: node.into(),
                tailscale_node: Some(node.into()),
                os: "Ubuntu 24.04.1 LTS".into(),
                kernel: "Linux 6.8.0-49-generic".into(),
                arch: "x86_64".into(),
                cpu_model: "AMD Ryzen 7 5700U with Radeon Graphics".into(),
                agent_version: "0.1.0".into(),
            },
        }))
    }

    fn agent(title: &str, state: SessionState, context_used: Option<u16>) -> AgentSession {
        AgentSession {
            agent: Agent::ClaudeCode,
            id: format!("{title}-0b7a3c1e-6d2f-4e1a-9c55-2f8e1d0a7b44"),
            title: title.into(),
            project: "magi".into(),
            state,
            state_secs: 754,
            model: "claude-opus-5-5".into(),
            context_tokens: 831_053,
            context_used,
            tokens: Tokens {
                input: 940,
                output: 598_164,
                cache_read: 149_160_864,
                cache_write: 1_608_265,
            },
            cost_micros: 51_380_000,
            cost_complete: true,
            age_secs: 7_900,
        }
    }

    fn agents() -> Vec<AgentSession> {
        vec![
            agent(
                "refactor the collectors with a very long session title",
                SessionState::Working {
                    tool: Some("Bash".into()),
                },
                Some(3_400),
            ),
            agent(
                "deploy fix",
                SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
                None,
            ),
            agent("notes", SessionState::Idle, Some(800)),
        ]
    }

    fn sample(load: f32, cores: usize) -> Sample {
        Sample {
            uptime_secs: 3 * 86_400 + 7_200,
            load: [load * 16.0, 3.0, 2.5],
            cpu: (load * f32::from(FULL)) as u16,
            cores: (0..cores)
                .map(|core| ((core * 997) % 10_001) as u16)
                .collect(),
            memory: Memory {
                total: 32 << 30,
                used: ((32u64 << 30) as f32 * load) as u64,
                swap_total: 2 << 30,
                swap_used: 1 << 29,
            },
            filesystems: (0..3)
                .map(|n| Filesystem {
                    mount: format!("/mnt/volume-with-a-long-name-{n}"),
                    total: 1 << 40,
                    used: (n as u64 + 1) << 38,
                })
                .collect(),
            disk: DiskIo {
                read_per_sec: u64::MAX / 3,
                write_per_sec: 12 << 20,
            },
            network: NetIo {
                rx_per_sec: 1 << 30,
                tx_per_sec: 0,
            },
            sessions: agents(),
            plans: vec![
                Plan {
                    agent: Agent::ClaudeCode,
                    windows: vec![
                        Window {
                            minutes: FIVE_HOURS,
                            used: 1_200,
                            resets_at: 4_000_000_000,
                        },
                        Window {
                            minutes: ONE_WEEK,
                            used: 4_025,
                            resets_at: 4_000_000_000,
                        },
                    ],
                },
                Plan {
                    agent: Agent::Codex,
                    windows: vec![Window {
                        minutes: FIVE_HOURS,
                        used: 300,
                        resets_at: 4_000_000_000,
                    }],
                },
            ],
            notices: Vec::new(),
        }
    }

    fn fleet(machines: usize, now: Instant) -> Model {
        let mut model = Model::new(2015, now - Duration::from_secs(60), Alerts::in_memory(now));
        for index in 0..machines {
            let endpoint = if index == 0 {
                Endpoint::Local
            } else {
                Endpoint::Tailnet(Ipv4Addr::new(
                    100,
                    64,
                    (index / 250) as u8,
                    (index % 250) as u8 + 1,
                ))
            };
            let node = if index == 0 {
                "host-0".to_string()
            } else {
                format!("host-{index}-with-an-unusually-long-machine-name")
            };
            model.links.insert(endpoint, Link::connecting());
            model.apply(endpoint, welcome(&node), now);
            for tick in 0..240 {
                let load = ((tick + index * 7) % 100) as f32 / 99.0;
                model.apply(
                    endpoint,
                    LinkUpdate::Sample(Box::new(sample(load, 4 + index % 64))),
                    now,
                );
            }
            if index % 5 == 3 {
                model.apply(
                    endpoint,
                    LinkUpdate::Lost("agent not running".into()),
                    now - Duration::from_secs(42),
                );
            }
        }
        model
    }

    fn draw(model: &Model, width: u16, height: u16, now: Instant) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, model, now)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn renders_every_size_from_phone_to_ultrawide_without_panicking() {
        let now = Instant::now();
        for machines in [0, 1, 2, 7, 60] {
            let mut model = fleet(machines, now);
            for (width, height) in [
                (1, 1),
                (27, 7),
                (28, 8),
                (40, 12),
                (60, 30),
                (84, 24),
                (120, 40),
                (320, 90),
                (1000, 300),
            ] {
                for selected in [0, machines / 2, machines + 10] {
                    model.selected = selected;
                    draw(&model, width, height, now);
                }
            }
        }
    }

    #[test]
    fn wide_screen_reads_in_plain_words() {
        let now = Instant::now();
        let mut model = fleet(4, now);
        for _ in 0..10 {
            model.apply(
                Endpoint::Local,
                LinkUpdate::Sample(Box::new(sample(1.0, 8))),
                now,
            );
        }
        let screen = draw(&model, 160, 40, now);
        for text in [
            "magi",
            "critical · 100%",
            "machine",
            "memory",
            "storage",
            "network",
            "lost",
            "this machine",
            "100%",
        ] {
            assert!(screen.contains(text), "missing {text:?}\n{screen}");
        }
        for jargon in ["MELCHIOR", "BALTHASAR", "CASPER", "PATTERN", "NOMINAL"] {
            assert!(!screen.contains(jargon), "{jargon} is back");
        }
    }

    #[test]
    fn no_line_overflows_the_pane() {
        let now = Instant::now();
        let model = fleet(4, now);
        for (width, height) in [(46, 30), (80, 24), (120, 40)] {
            let screen = draw(&model, width, height, now);
            for line in screen.lines() {
                assert!(
                    !line.trim_end().ends_with('/'),
                    "a value was cut mid-way at {width}x{height}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn phone_width_keeps_names_cpu_and_memory() {
        let now = Instant::now();
        let screen = draw(&fleet(2, now), 46, 30, now);
        assert!(screen.contains("magi"));
        assert!(screen.contains("host-0"));
        assert!(screen.contains("cpu") && screen.contains("memory"));
    }

    #[test]
    fn columns_drop_by_priority_and_meters_shrink_first() {
        assert_eq!(TablePlan::fit(5).columns, vec![]);
        let narrow = TablePlan::fit(13);
        assert_eq!(
            (narrow.meter, narrow.columns.clone()),
            (0, vec![Column::Cpu, Column::Memory])
        );
        let wide = TablePlan::fit(1_000);
        assert_eq!(wide.meter, 10);
        assert_eq!(
            wide.columns,
            vec![
                Column::Cpu,
                Column::Memory,
                Column::Recent,
                Column::Disk,
                Column::Network,
                Column::Uptime
            ]
        );
    }

    #[test]
    fn agents_that_need_you_come_first_and_show_in_the_header() {
        let now = Instant::now();
        let model = fleet(2, now);
        let screen = draw(&model, 160, 40, now);
        assert!(screen.contains("2 need you"), "{screen}");
        let agent_lines: Vec<&str> = screen
            .lines()
            .filter(|line| line.contains("opus 5.5"))
            .collect();
        assert!(
            agent_lines[0].contains("needs you · Bash"),
            "{agent_lines:?}"
        );
        assert!(
            agent_lines.last().unwrap().contains("idle 12m"),
            "{agent_lines:?}"
        );
        assert!(screen.contains("Claude 5-hour 12%"), "{screen}");
        assert!(screen.contains("Codex 5-hour 3%"), "{screen}");
    }

    #[test]
    fn selecting_an_agent_shows_its_detail_and_keeps_the_machines_visible() {
        let now = Instant::now();
        let mut model = fleet(2, now);
        model.selected = 2;
        let screen = draw(&model, 140, 40, now);
        for text in [
            "host-0",
            "needs you · Bash · for 12m",
            "at API list prices since it started",
            "$51.38",
            "149M",
            "5-hour",
        ] {
            assert!(screen.contains(text), "missing {text:?}\n{screen}");
        }
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench"]
    fn bench_viewer() {
        use std::cell::Cell;
        use std::rc::Rc;

        use ratatui::backend::CrosstermBackend;
        use ratatui::{TerminalOptions, Viewport};

        struct Counting(Rc<Cell<usize>>);
        impl std::io::Write for Counting {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.set(self.0.get() + buf.len());
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let now = Instant::now();
        for (machines, width, height) in [(2, 120, 40), (2, 46, 30), (60, 240, 70)] {
            let model = fleet(machines, now);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let frames = 300;
            let started = Instant::now();
            for _ in 0..frames {
                terminal.draw(|frame| render(frame, &model, now)).unwrap();
            }
            println!(
                "render {machines:>2} machines at {width}x{height}: {:?} per frame",
                started.elapsed() / frames
            );
        }

        let mut model = fleet(2, now);
        let update = sample(0.5, 64);
        let applies = 200_000;
        let started = Instant::now();
        for _ in 0..applies {
            model.apply(
                Endpoint::Local,
                LinkUpdate::Sample(Box::new(update.clone())),
                now,
            );
        }
        println!("apply one sample: {:?}", started.elapsed() / applies);

        let written = Rc::new(Cell::new(0));
        let viewport = Viewport::Fixed(Rect::new(0, 0, 120, 40));
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(Counting(Rc::clone(&written))),
            TerminalOptions { viewport },
        )
        .unwrap();
        terminal.draw(|frame| render(frame, &model, now)).unwrap();
        let first = written.get();
        let frames = 120;
        for tick in 0..frames {
            let load = (tick % 100) as f32 / 99.0;
            model.apply(
                Endpoint::Local,
                LinkUpdate::Sample(Box::new(sample(load, 10))),
                now,
            );
            terminal.draw(|frame| render(frame, &model, now)).unwrap();
        }
        println!(
            "terminal bytes at 120x40: first frame {first}, then {} per update",
            (written.get() - first) / frames
        );
    }
}
