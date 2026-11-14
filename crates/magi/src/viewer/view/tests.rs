use std::net::Ipv4Addr;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use std::time::Duration;

use super::*;
use crate::proto::{Agent, Sample};
use crate::proto::{
    AgentSession, DiskIo, FIVE_HOURS, Filesystem, Identity, Memory, NetIo, ONE_WEEK, Plan,
    SessionState, Tokens, Waiting, Welcome, Window,
};
use crate::viewer::alerts::Alerts;
use crate::viewer::graphics::Graphics;
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
    terminal
        .draw(|frame| render(frame, model, now, &mut Graphics::cells_only()))
        .unwrap();
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
fn the_chart_span_counts_the_samples_drawn_at_the_real_interval() {
    assert_eq!(
        machines::span(240, Duration::from_millis(500)),
        Duration::from_secs(120)
    );
    assert_eq!(
        machines::span(10, Duration::from_millis(1500)),
        Duration::from_secs(15)
    );
}

#[test]
fn the_machine_list_stays_at_the_top_while_an_agent_is_selected() {
    let now = Instant::now();
    let mut model = fleet(7, now);
    model.selected = 7 + 1;
    let screen = draw(&model, 60, 30, now);
    assert!(screen.contains("host-0"), "{screen}");
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
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| render(frame, &model, now, &mut Graphics::pixels(8, 16)))
                    .unwrap();
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
        "memory critical",
        "Machines",
        "Agents",
        "Memory",
        "Storage",
        "Network",
        "lost",
        "this machine",
        "100%",
    ] {
        assert!(screen.contains(text), "missing {text:?}\n{screen}");
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
    assert!(screen.contains("cpu") && screen.contains("mem"));
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
    assert!(screen.contains("Claude 5h"), "{screen}");
    assert!(
        screen.contains("12%") && screen.contains("Codex 5h"),
        "{screen}"
    );
    assert!(screen.contains("✻ deploy fix"), "{screen}");
}

#[test]
fn selecting_an_agent_shows_its_detail_and_keeps_the_machines_visible() {
    let now = Instant::now();
    let mut model = fleet(2, now);
    model.selected = 2;
    let screen = draw(&model, 140, 40, now);
    for text in [
        "host-0",
        "needs you · Bash",
        "for 12m",
        "at API list prices",
        "$51.38",
        "149M",
        "5-hour",
        "Claude Code",
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
            terminal
                .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only()))
                .unwrap();
        }
        println!(
            "render {machines:>2} machines at {width}x{height}: {:?} per frame",
            started.elapsed() / frames
        );
    }

    let mut pixels = Graphics::pixels(8, 16);
    let mut terminal = Terminal::new(TestBackend::new(160, 45)).unwrap();
    let mut model = fleet(2, now);
    let mut uploaded = 0;
    let mut slowest = Duration::ZERO;
    let frames = 60;
    for tick in 0..frames {
        let load = (tick % 100) as f32 / 99.0;
        model.apply(
            Endpoint::Local,
            LinkUpdate::Sample(Box::new(sample(load, 10))),
            now,
        );
        let started = Instant::now();
        terminal
            .draw(|frame| render(frame, &model, now, &mut pixels))
            .unwrap();
        let mut out = Vec::new();
        pixels.flush(&mut out).unwrap();
        slowest = slowest.max(started.elapsed());
        uploaded += out.len();
    }
    println!(
        "kitty charts at 160x45: {} bytes per update, slowest frame with drawing {:?}",
        uploaded / frames,
        slowest
    );

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
    terminal
        .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only()))
        .unwrap();
    let first = written.get();
    let frames = 120;
    for tick in 0..frames {
        let load = (tick % 100) as f32 / 99.0;
        model.apply(
            Endpoint::Local,
            LinkUpdate::Sample(Box::new(sample(load, 10))),
            now,
        );
        terminal
            .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only()))
            .unwrap();
    }
    println!(
        "terminal bytes at 120x40: first frame {first}, then {} per update",
        (written.get() - first) / frames
    );
}
