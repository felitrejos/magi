use std::net::Ipv4Addr;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use std::time::Duration;

use super::*;
use crate::proto::{Agent, Child, ChildKind, ChildState, Sample, TopProcess};
use crate::proto::{
    AgentSession, DiskIo, FIVE_HOURS, Filesystem, Identity, Memory, NetIo, ONE_WEEK, Plan,
    SessionState, Tokens, Waiting, Welcome, Window,
};
use crate::viewer::alerts::Alerts;
use crate::viewer::graphics::Graphics;
use crate::viewer::model::{Endpoint, Link, LinkUpdate};
use unicode_width::UnicodeWidthStr;

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
            core_kinds: Vec::new(),
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
        pane: None,
        usage: None,
        children: Vec::new(),
        branch: None,
        worktree: None,
        parent: None,
    }
}

fn agents() -> Vec<AgentSession> {
    let mut working = agent(
        "refactor the collectors with a very long session title",
        SessionState::Working {
            tool: Some("Bash".into()),
        },
        Some(3_400),
    );
    working.branch = Some("claude/quiet-fox".into());
    working.worktree = Some("quiet-fox".into());
    working.children = vec![
        Child {
            id: "toolu_a".into(),
            kind: ChildKind::Subagent,
            label: "Find the parser".into(),
            detail: "Explore".into(),
            state: ChildState::Running,
            secs: 40,
            task: None,
        },
        Child {
            id: "toolu_b".into(),
            kind: ChildKind::Shell,
            label: "pnpm dev".into(),
            detail: String::new(),
            state: ChildState::Done,
            secs: 300,
            task: Some("bash_1".into()),
        },
    ];
    let mut deploy = agent(
        "deploy fix",
        SessionState::Waiting(Waiting::Permission(Some("Bash".into()))),
        None,
    );
    deploy.project = "stampr-ops".into();
    let mut notes = agent("notes", SessionState::Idle, Some(800));
    notes.project = "stampr".into();
    vec![working, deploy, notes]
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
                as_of: 0,
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
                as_of: 0,
                windows: vec![Window {
                    minutes: FIVE_HOURS,
                    used: 300,
                    resets_at: 4_000_000_000,
                }],
            },
        ],
        notices: Vec::new(),
        forwards: Vec::new(),
        processes: vec![
            TopProcess {
                pid: 41,
                name: "claude".into(),
                cpu: Some(2_500),
                memory: 410 << 20,
                session: Some(agents()[0].id.clone()),
            },
            TopProcess {
                pid: 7,
                name: "WindowServer".into(),
                cpu: Some(120),
                memory: 180 << 20,
                session: None,
            },
        ],
        temperature: Some(61.0),
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

fn select_title(model: &mut Model, title: &str, now: Instant) {
    let index = model
        .listed(now)
        .iter()
        .position(|item| {
            matches!(&item.entry, crate::viewer::model::Entry::Session(agent) if agent.session.title.starts_with(title))
        })
        .unwrap();
    model.selected = model.rows(now).len() + index;
}

fn draw(model: &Model, width: u16, height: u16, now: Instant) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| render(frame, model, now, &mut Graphics::cells_only(), &mut None))
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
fn the_machine_view_reads_as_titled_charts_then_cores_then_what_uses_it() {
    let now = Instant::now();
    let screen = draw(&fleet(2, now), 160, 50, now);
    for text in [
        "61 °C",
        "CPU",
        "busiest 30%",
        "Memory",
        "swap",
        "Network",
        "Disk",
        "Cores",
        "Using it",
        "WindowServer",
        "claude",
        "✻ refactor the collectors",
        "250%",
    ] {
        assert!(screen.contains(text), "missing {text:?}\n{screen}");
    }
    let at = |text: &str| screen.lines().position(|line| line.contains(text)).unwrap();
    assert_eq!(at("CPU  "), at("Memory  "));
    assert_eq!(at("Network  "), at("Disk  "));
    assert!(at("Memory  ") < at("Disk  ") && at("Disk  ") < at("Cores"));
    assert_eq!(at("Cores"), at("Using it"));
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
                    .draw(|frame| {
                        render(frame, &model, now, &mut Graphics::pixels(8, 16), &mut None)
                    })
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
        "Needs you",
        "Memory",
        "Disk",
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
fn a_machine_is_tagged_by_its_first_word_unless_another_shares_it() {
    let names = ["felipes-macbook-pro", "homelab", "build.lan", "build-2"];
    assert_eq!(
        crate::viewer::model::machine_tag("felipes-macbook-pro", &names),
        "felipes"
    );
    assert_eq!(
        crate::viewer::model::machine_tag("homelab", &names),
        "homelab"
    );
    assert_eq!(
        crate::viewer::model::machine_tag("build.lan", &names),
        "build.lan"
    );
}

#[test]
#[ignore = "prints the screen: cargo test -- --ignored --nocapture print_wide"]
fn print_wide() {
    let now = Instant::now();
    let mut model = fleet(2, now);
    select_title(&mut model, "deploy fix", now);
    println!("{}", draw(&model, 130, 34, now));
}

#[test]
#[ignore = "prints the screen: cargo test -- --ignored --nocapture print_machine"]
fn print_machine() {
    let now = Instant::now();
    println!("{}", draw(&fleet(2, now), 130, 40, now));
}

#[test]
#[ignore = "prints the screen: cargo test -- --ignored --nocapture print_phone"]
fn print_phone() {
    let now = Instant::now();
    println!("{}", draw(&fleet(2, now), 46, 60, now));
}

#[test]
fn phone_width_keeps_names_cpu_and_memory() {
    let now = Instant::now();
    let screen = draw(&fleet(2, now), 46, 30, now);
    assert!(
        screen.contains("host-0") && screen.contains("37%"),
        "{screen}"
    );
}

#[test]
fn sessions_that_need_you_come_first_then_projects_with_plans_at_the_bottom() {
    let now = Instant::now();
    let model = fleet(2, now);
    let screen = draw(&model, 160, 40, now);
    let lines: Vec<&str> = screen.lines().collect();
    let at = |text: &str| lines.iter().position(|line| line.contains(text));
    let claude = at("Claude 5h").expect("Claude's plan");
    let codex = at("Codex 5h").expect("Codex's plan");
    let needs_you = at(" Needs you").expect("a Needs you group");
    let magi = at("▾ magi").expect("the magi project");
    let stampr = at("▾ stampr").expect("stampr, with stampr-ops merged in");
    assert!(needs_you < magi && magi < stampr, "{screen}");
    assert!(stampr < claude && claude < codex, "{screen}");
    assert!(lines[needs_you + 1].contains("✻ deploy fix"), "{screen}");
    assert!(!screen.contains("stampr-ops "), "{screen}");
    assert!(
        !lines[needs_you + 1].contains("opus 5.5"),
        "model and context stay in the detail\n{screen}"
    );
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
        "12m",
        "$51.38",
        "opus 5.5",
        "831K context",
        "reading the conversation",
    ] {
        assert!(screen.contains(text), "missing {text:?}\n{screen}");
    }
    for gone in ["cache write", "at API list prices", "session deploy fix"] {
        assert!(!screen.contains(gone), "still shows {gone:?}\n{screen}");
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
                .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only(), &mut None))
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
            .draw(|frame| render(frame, &model, now, &mut pixels, &mut None))
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
        .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only(), &mut None))
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
            .draw(|frame| render(frame, &model, now, &mut Graphics::cells_only(), &mut None))
            .unwrap();
    }
    println!(
        "terminal bytes at 120x40: first frame {first}, then {} per update",
        (written.get() - first) / frames
    );
}

#[test]
fn storage_and_every_disk_stay_on_screen_when_the_pane_is_short() {
    let now = Instant::now();
    let model = fleet(2, now);
    for (width, height) in [(120, 24), (120, 30), (160, 30)] {
        let screen = draw(&model, width, height, now);
        assert!(screen.contains("Disk"), "{width}x{height}\n{screen}");
        for n in 0..3 {
            assert!(
                screen.contains(&format!("long-name-{n} ")),
                "{width}x{height} lost disk {n}\n{screen}"
            );
        }
    }
}

#[test]
fn truncated_text_never_draws_wider_than_its_room() {
    for text in [
        "❤️❤️❤️❤️x",
        "👨‍👩‍👧 family",
        "日本語のテキスト",
        "plain ascii text",
    ] {
        for width in 1..8 {
            let cut = truncate(text, width);
            assert!(cut.width() <= width, "{text:?} at {width} -> {cut:?}");
        }
    }
    assert_eq!(truncate("👨‍👩‍👧 family", 3), "👨‍👩‍👧…");
}

#[test]
fn only_a_session_using_much_of_its_machine_is_flagged_for_cpu() {
    let now = Instant::now();
    let mut model = fleet(1, now);
    let mut busy = sample(0.5, 12);
    let cores = busy.cores.len() as u32;
    busy.sessions[0].usage = Some(crate::proto::Usage {
        cpu: Some(cores * 500),
        memory: 3 << 30,
    });
    busy.sessions[1].usage = Some(crate::proto::Usage {
        cpu: None,
        memory: 1 << 20,
    });
    model.apply(Endpoint::Local, LinkUpdate::Sample(Box::new(busy)), now);
    select_title(&mut model, "refactor", now);
    let screen = draw(&model, 160, 50, now);
    assert_eq!(screen.matches("high CPU").count(), 1, "{screen}");
    assert!(!screen.contains("cores"), "{screen}");
    select_title(&mut model, "deploy fix", now);
    let screen = draw(&model, 160, 50, now);
    assert!(!screen.contains("high CPU"), "{screen}");
}

#[test]
fn each_machine_is_one_line_and_its_details_stay_in_the_detail() {
    let now = Instant::now();
    let mut model = fleet(3, now);
    model.selected = model.rows(now).len();
    let screen = draw(&model, 160, 40, now);
    let machine_lines = screen
        .lines()
        .filter(|line| line.contains("host-") && line.contains('%'))
        .count();
    assert_eq!(machine_lines, 3, "{screen}");
    assert!(!screen.contains("up 3d 2h"), "{screen}");
}

#[test]
fn a_mac_shows_its_performance_cores_apart_from_its_efficiency_cores() {
    use crate::proto::CoreKind::{Efficiency, Performance};
    let now = Instant::now();
    let mut model = fleet(1, now);
    let screen = draw(&model, 160, 50, now);
    assert!(!screen.contains("Performance"), "{screen}");
    let machine = model
        .links
        .get_mut(&Endpoint::Local)
        .and_then(|link| link.machine.as_mut())
        .unwrap();
    machine.identity.core_kinds = vec![Efficiency, Efficiency, Performance, Performance];
    let screen = draw(&model, 160, 50, now);
    let at = |text: &str| screen.lines().position(|line| line.contains(text)).unwrap();
    assert!(at("Cores") < at("Performance") && at("Performance") < at("Efficiency"));
    assert_eq!(screen.matches("2 cores").count(), 2, "{screen}");
}
