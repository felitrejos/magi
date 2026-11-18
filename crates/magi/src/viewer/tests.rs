use std::net::Ipv4Addr;

use super::*;
use crate::proto::{AgentSession, Identity, RexPane, Sample, SessionState, Welcome};

fn key(key: Key) -> Event {
    Event::Input(Input::Key(key))
}

fn agent(model: &mut Model, endpoint: Endpoint, name: &str) {
    agent_reaching(model, endpoint, name, &[]);
}

fn agent_reaching(model: &mut Model, endpoint: Endpoint, name: &str, reaches: &[&str]) {
    model.links.insert(endpoint, Link::connecting());
    let identity = Identity {
        hostname: name.into(),
        tailscale_node: Some(name.into()),
        os: String::new(),
        arch: String::new(),
        tools: Vec::new(),
        cpu_model: String::new(),
        core_kinds: Vec::new(),
        reaches: reaches.iter().map(|host| host.to_string()).collect(),
    };
    model.apply(
        endpoint,
        LinkUpdate::Welcome(Box::new(Welcome {
            interval_ms: 1000,
            identity,
        })),
        Instant::now(),
    );
}

fn fleet() -> (Model, Links) {
    let (sender, _) = mpsc::sync_channel(8);
    let links = Links {
        handles: HashMap::new(),
        next_generation: 0,
        port: 2015,
        socket: PathBuf::new(),
        sender,
        rex: false,
        launch: None,
    };
    let now = Instant::now();
    let mut model = Model::new(2015, now, alerts::Alerts::in_memory(now));
    agent_reaching(&mut model, Endpoint::Local, "mac", &["homelab", "nas"]);
    agent(
        &mut model,
        Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2)),
        "homelab",
    );
    agent(
        &mut model,
        Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3)),
        "nas",
    );
    (model, links)
}

fn selected(model: &Model) -> String {
    match model.selection(Instant::now()) {
        Some(model::Focus::Machine(name)) => name,
        other => panic!("expected a machine, got {other:?}"),
    }
}

#[test]
fn arrow_keys_move_the_selection() {
    let (mut model, mut links) = fleet();
    handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(selected(&model), "homelab");
    handle_batch(
        [key(Key::Down), key(Key::Down), key(Key::Up)].into_iter(),
        &mut model,
        &mut links,
    )
    .unwrap();
    assert_eq!(selected(&model), "homelab");
    handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(
        model.selection(Instant::now()),
        Some(model::Focus::Machine("nas".into()))
    );
}

#[test]
fn the_selection_follows_its_machine_when_rows_reorder() {
    let (mut model, mut links) = fleet();
    handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
    agent(
        &mut model,
        Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 4)),
        "laptop",
    );
    let sample = Event::Link(Endpoint::Local, 0, LinkUpdate::Sample(Box::default()));
    handle_batch([sample].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(selected(&model), "nas");
}

#[test]
fn q_quits() {
    let (mut model, mut links) = fleet();
    let flow = handle_batch([key(Key::Char('q'))].into_iter(), &mut model, &mut links).unwrap();
    assert!(flow == Flow::Quit);
}

fn with_session(model: &mut Model, pane: Option<RexPane>) {
    let session = AgentSession {
        agent: crate::proto::Agent::ClaudeCode,
        id: "s1".into(),
        title: "fix login".into(),
        project: "web".into(),
        state: SessionState::Idle,
        state_secs: 0,
        model: String::new(),
        context_tokens: 0,
        context_used: None,
        cost_micros: 0,
        cost_complete: true,
        pane,
        usage: None,
        children: Vec::new(),
        branch: None,
        parent: None,
        activity: None,
        mode: None,
        queued: None,
        remote: None,
        pr: None,
    };
    let sample = Sample {
        sessions: vec![session],
        ..Sample::default()
    };
    model.apply(
        Endpoint::Local,
        LinkUpdate::Sample(Box::new(sample)),
        Instant::now(),
    );
    select_first_session(model);
}

fn asked_and_failed(model: &Model, reason: &str) -> bool {
    model
        .folders
        .values()
        .any(|folders| matches!(folders, model::Folders::Failed(said) if said.contains(reason)))
}

fn screen(model: &Model) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| view::render(frame, model, Instant::now(), &mut Graphics::cells_only()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..30)
        .map(|y| {
            (0..120)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn footer(model: &Model) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| view::render(frame, model, Instant::now(), &mut Graphics::cells_only()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..120).map(|x| buffer[(x, 29)].symbol()).collect()
}

#[test]
fn enter_explains_when_a_session_is_not_in_rex() {
    let (mut model, mut links) = fleet();
    with_session(&mut model, None);
    assert!(!footer(&model).contains("open"));
    handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(
        model.flashed(Instant::now()),
        Some(("fix login isn't running in a Rex pane", false))
    );
    assert!(footer(&model).contains("fix login isn't running in a Rex pane"));
}

#[test]
fn u_and_d_move_the_selection_and_v_opens_the_diff() {
    assert!(matches!(action(&Key::Char('u')), Some(Action::Up)));
    assert!(matches!(action(&Key::Char('d')), Some(Action::Down)));
    assert!(matches!(action(&Key::Char('v')), Some(Action::Diff)));
    assert!(action(&Key::Char('j')).is_none());
    assert!(action(&Key::Char('k')).is_none());
}

#[test]
fn a_session_shows_what_is_queued_but_not_its_mode_or_activity() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let mut session = in_rex(claude("s1", 0));
    session.state = SessionState::Working { tool: None };
    session.mode = Some(crate::proto::PermissionMode::Plan);
    session.activity = Some("reading the parser".into());
    session.queued = Some("then commit".into());
    model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
    select_first_session(&mut model);
    let shown = screen(&model);
    assert!(shown.contains("queued"), "{shown}");
    assert!(!shown.contains("plan ·"), "{shown}");
    assert!(!shown.contains("reading the parser"), "{shown}");
    assert!(!footer(&model).contains("mode"), "{}", footer(&model));
    handle_batch([key(Key::BackTab)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.flashed(Instant::now()), None);
}

#[test]
fn a_failing_pull_request_shows_on_its_session_and_raises_an_alert() {
    let (mut model, _) = fleet();
    let now = Instant::now();
    let mut session = in_rex(claude("s1", 0));
    session.pr = Some(crate::proto::PullRequest {
        number: 42,
        state: crate::proto::PrState::Open,
        checks: crate::proto::Checks::Failing,
        review: Some(crate::proto::Review::ChangesRequested),
        url: "https://github.com/o/r/pull/42".into(),
    });
    model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
    let shown = screen(&model);
    assert!(
        shown.contains("PR #42 checks failing · changes requested"),
        "{shown}"
    );
    let raised: Vec<String> = alerts::conditions(&model, now, 0)
        .into_iter()
        .map(|condition| condition.text)
        .collect();
    assert!(
        raised
            .iter()
            .any(|text| text == "run the tests: PR #42 checks failing"),
        "{raised:?}"
    );
}

#[test]
fn a_sessions_footer_offers_its_rex_pane_and_none_of_the_removed_keys() {
    let (mut model, _) = fleet();
    with_session(
        &mut model,
        Some(RexPane {
            session: "session:1".into(),
            window: "window:2".into(),
            block: "block:3".into(),
        }),
    );
    let keys = footer(&model);
    assert!(keys.contains("enter open in Rex"), "{keys}");
    for gone in ["project", "rename", "mode", "outputs", "open in Claude"] {
        assert!(!keys.contains(gone), "still offers {gone:?}: {keys}");
    }
    model.selected = 0;
    assert!(!footer(&model).contains("open in Rex"));
}

fn live_sample(cpu: u16, session: Option<AgentSession>) -> LinkUpdate {
    LinkUpdate::Sample(Box::new(Sample {
        cpu,
        cores: vec![0; 8],
        sessions: session.into_iter().collect(),
        ..Sample::default()
    }))
}

fn claude(id: &str, millicores: u32) -> AgentSession {
    AgentSession {
        agent: crate::proto::Agent::ClaudeCode,
        id: id.into(),
        title: "run the tests".into(),
        project: "api".into(),
        state: SessionState::Working { tool: None },
        state_secs: 0,
        model: String::new(),
        context_tokens: 0,
        context_used: None,
        cost_micros: 0,
        cost_complete: true,
        pane: None,
        usage: Some(crate::proto::Usage {
            cpu: Some(millicores),
            memory: 0,
        }),
        children: Vec::new(),
        branch: None,
        parent: None,
        activity: None,
        mode: None,
        queued: None,
        remote: None,
        pr: None,
    }
}

#[test]
fn a_move_is_pushed_by_a_machine_that_reaches_the_target_or_else_pulled_by_the_target() {
    let (model, _links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    let (sent_to, request) = model
        .move_request("s1", "run the tests", Endpoint::Local, "homelab", now)
        .unwrap();
    assert_eq!(sent_to, Endpoint::Local);
    assert!(matches!(request, proto::Request::Move { to, .. } if to == "homelab"));
    let (sent_to, request) = model
        .move_request("s1", "run the tests", homelab, "mac", now)
        .unwrap();
    assert_eq!(sent_to, Endpoint::Local);
    assert!(matches!(
        request,
        proto::Request::Pull { from, to, title, .. } if from == "homelab" && to == "mac" && title == "run the tests"
    ));
    assert_eq!(
        model.move_request("s1", "run the tests", homelab, "nas", now),
        Err(
            "neither homelab nor nas is set up to reach the other; run magi init on one of them"
                .into()
        )
    );
}

#[test]
fn m_proposes_the_least_busy_other_machine_and_only_y_sends_it() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    model.apply(
        Endpoint::Local,
        live_sample(9_500, Some(claude("s1", 3_000))),
        now,
    );
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    let nas = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3));
    model.apply(homelab, live_sample(4_000, None), now);
    model.apply(nas, live_sample(300, None), now);
    select_first_session(&mut model);
    let agent = model.selected_agent(now).unwrap();
    assert_eq!(
        model
            .move_suggestion(&agent, now)
            .map(|suggestion| suggestion.target.machine),
        Some("nas".into())
    );
    handle_batch([key(Key::Char('m'))].into_iter(), &mut model, &mut links).unwrap();
    let pending = model.pending_move.clone().unwrap();
    assert_eq!((pending.from.as_str(), pending.to.as_str()), ("mac", "nas"));
    assert!(footer(&model).contains("Move run the tests from mac to nas?"));
    handle_batch([key(Key::Char('n'))].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.pending_move, None);
    assert_eq!(model.flashed(now), None);
    handle_batch(
        [key(Key::Char('m')), key(Key::Char('y'))].into_iter(),
        &mut model,
        &mut links,
    )
    .unwrap();
    assert_eq!(model.pending_move, None);
    assert_eq!(
        model.flashed(Instant::now()),
        Some((
            "couldn't ask for the move: that machine isn't connected",
            false
        ))
    );
}

#[test]
fn a_move_is_suggested_when_a_session_weighs_on_a_machine_under_sustained_pressure() {
    struct Machine {
        cpu: u16,
        memory: f32,
        load: f32,
    }
    let quiet = Machine {
        cpu: 1_000,
        memory: 0.3,
        load: 1.0,
    };
    let sample = |machine: &Machine, session: Option<AgentSession>| {
        LinkUpdate::Sample(Box::new(Sample {
            cpu: machine.cpu,
            cores: vec![0; 8],
            load: [machine.load; 3],
            memory: crate::proto::Memory {
                total: 16 << 30,
                used: ((16u64 << 30) as f32 * machine.memory) as u64,
                ..Default::default()
            },
            sessions: session.into_iter().collect(),
            ..Sample::default()
        }))
    };
    let suggestion = |here: Machine, millicores: u32, gigabytes: u64, there: &Machine| {
        let (mut model, _) = fleet();
        let now = Instant::now();
        let mut session = claude("s1", millicores);
        session.usage = Some(crate::proto::Usage {
            cpu: Some(millicores),
            memory: gigabytes << 30,
        });
        model.apply(Endpoint::Local, sample(&here, Some(session)), now);
        let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
        model.apply(homelab, sample(there, None), now);
        select_first_session(&mut model);
        let agent = model.selected_agent(now).unwrap();
        model
            .move_suggestion(&agent, now)
            .map(|suggestion| suggestion.pressure.describe("mac"))
    };
    let busy_cpu = || Machine {
        cpu: 9_000,
        ..quiet
    };
    assert_eq!(
        suggestion(busy_cpu(), 2_000, 0, &quiet).as_deref(),
        Some("mac's CPU has averaged 90% for a minute")
    );
    assert_eq!(suggestion(busy_cpu(), 500, 4, &quiet), None);
    let crowded = Machine {
        cpu: 6_000,
        ..quiet
    };
    assert_eq!(suggestion(busy_cpu(), 2_000, 0, &crowded), None);
    let full = || Machine {
        memory: 0.95,
        ..quiet
    };
    assert_eq!(
        suggestion(full(), 0, 4, &quiet).as_deref(),
        Some("mac's memory is 95% full")
    );
    assert_eq!(suggestion(full(), 3_000, 0, &quiet), None);
    let loaded = Machine {
        load: 12.0,
        ..quiet
    };
    assert_eq!(
        suggestion(loaded, 1_500, 0, &quiet).as_deref(),
        Some("mac has a load of 12 on 8 cores")
    );
    assert_eq!(suggestion(Machine { ..quiet }, 4_000, 8, &quiet), None);
}

#[test]
fn replies_from_an_agent_show_in_the_footer_in_their_own_colour() {
    let (mut model, _) = fleet();
    let now = Instant::now();
    model.apply(
        Endpoint::Local,
        LinkUpdate::Reply(crate::proto::Reply {
            ok: true,
            text: "moving run the tests to nas…".into(),
        }),
        now,
    );
    assert_eq!(
        model.flashed(now),
        Some(("moving run the tests to nas…", true))
    );
    assert!(footer(&model).contains("moving run the tests to nas…"));
}

#[test]
fn when_the_selected_session_ends_the_cursor_settles_and_then_stays_put() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let sample = |sessions: Vec<AgentSession>| {
        LinkUpdate::Sample(Box::new(Sample {
            cores: vec![0; 8],
            sessions,
            ..Sample::default()
        }))
    };
    let named = |id: &str, title: &str, state: SessionState| AgentSession {
        title: title.into(),
        state,
        ..claude(id, 0)
    };
    model.apply(
        Endpoint::Local,
        sample(vec![
            named("a", "alpha", SessionState::Idle),
            named("b", "bravo", SessionState::Idle),
        ]),
        now,
    );
    select(&mut model, "b");
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert_eq!(model.selected_agent(now).unwrap().session.id, "b");
    model.apply(
        Endpoint::Local,
        sample(vec![named("a", "alpha", SessionState::Idle)]),
        now,
    );
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    let settled = model
        .selected_agent(now)
        .map(|agent| agent.session.id.clone());
    assert_eq!(settled.as_deref(), Some("a"));
    model.apply(
        Endpoint::Local,
        sample(vec![
            named("a", "alpha", SessionState::Idle),
            named(
                "c",
                "charlie",
                SessionState::Waiting(crate::proto::Waiting::Permission(None)),
            ),
        ]),
        now,
    );
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert_eq!(model.selected_agent(now).unwrap().session.id, "a");
}

fn in_rex(mut session: AgentSession) -> AgentSession {
    session.state = SessionState::Idle;
    session.pane = Some(RexPane {
        session: "session:1".into(),
        window: "window:2".into(),
        block: "block:3".into(),
    });
    session
}

fn select_first_session(model: &mut Model) {
    let index = model
        .board(Instant::now())
        .iter()
        .position(|item| matches!(item, model::Focus::Agent(_)))
        .unwrap();
    model.selected = index;
}

fn select(model: &mut Model, id: &str) {
    let index = model
        .board(Instant::now())
        .iter()
        .position(|item| *item == model::Focus::Agent(id.into()))
        .unwrap();
    model.selected = index;
}

#[test]
fn a_session_leaves_its_running_subagents_to_their_own_tabs() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let mut parent = claude("s1", 0);
    parent.children = vec![crate::proto::Child {
        id: "toolu_a".into(),
        kind: crate::proto::ChildKind::Subagent,
        label: "Find the parser".into(),
        detail: "Explore".into(),
        state: crate::proto::ChildState::Running,
        secs: 12,
        task: None,
        doing: None,
    }];
    model.apply(Endpoint::Local, live_sample(1_000, Some(parent)), now);
    select_first_session(&mut model);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    let shown = screen(&model);
    assert!(!shown.contains("subagent"), "{shown}");
}

#[test]
fn a_session_outside_rex_offers_to_continue_in_rex() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(claude("s1", 0))),
        now,
    );
    select_first_session(&mut model);
    assert!(screen(&model).contains("not in Rex"), "{}", screen(&model));
    assert!(
        footer(&model).contains("r continue in Rex"),
        "{}",
        footer(&model)
    );
    handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(
        model.flashed(Instant::now()),
        Some(("run the tests isn't running in a Rex pane", false))
    );
}

#[test]
fn y_and_n_answer_only_a_session_asking_for_permission() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(in_rex(claude("s1", 0)))),
        now,
    );
    select_first_session(&mut model);
    assert!(!screen(&model).contains("Needs you"));
    handle_batch([key(Key::Char('y'))].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.flashed(Instant::now()), None);
    let mut asking = in_rex(claude("s1", 0));
    asking.state = SessionState::Waiting(crate::proto::Waiting::Permission(Some("Bash".into())));
    model.apply(Endpoint::Local, live_sample(1_000, Some(asking)), now);
    model.reselect(now);
    let shown = screen(&model);
    assert!(
        shown.contains("Needs you") && shown.contains("needs you · Bash"),
        "{shown}"
    );
    assert!(
        footer(&model).contains("y allow   n deny"),
        "{}",
        footer(&model)
    );
    handle_batch([key(Key::Char('n'))].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(
        model.flashed(Instant::now()),
        Some(("couldn't answer it: that machine isn't connected", false))
    );
}

#[test]
fn escape_never_quits() {
    let (mut model, mut links) = fleet();
    let flow = handle_batch(
        [key(Key::Escape), key(Key::Escape)].into_iter(),
        &mut model,
        &mut links,
    )
    .unwrap();
    assert!(flow == Flow::Continue);
}

#[test]
fn after_a_move_the_selection_follows_the_session_to_its_new_machine() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(claude("s1", 0))),
        now,
    );
    model.apply(homelab, live_sample(1_000, Some(claude("other", 0))), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    model.follow("s1".into(), Endpoint::Local, "homelab".into(), now);
    model.apply(Endpoint::Local, live_sample(1_000, None), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert_eq!(model.selected_agent(now).map(|_| ()), None);
    assert_eq!(model.rows(now)[model.selected].name, "homelab");
    let mut arrived = claude("s1", 0);
    arrived.title = "moved".into();
    model.apply(homelab, live_sample(1_000, Some(arrived)), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    let selected = model.selected_agent(now).unwrap();
    assert_eq!(
        (selected.session.id.as_str(), selected.endpoint),
        ("s1", homelab)
    );
}

#[test]
fn a_moved_session_shows_the_ports_its_old_machine_forwards_to_it() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    let LinkUpdate::Sample(mut here) = live_sample(1_000, None) else {
        unreachable!()
    };
    here.forwards = vec![
        crate::proto::Forward {
            session: "s1".into(),
            host: "homelab".into(),
            ssh: "homelab".into(),
            ports: vec![3000],
            state: crate::proto::ForwardState::Waiting,
        },
        crate::proto::Forward {
            session: "s1".into(),
            host: "nas".into(),
            ssh: "nas".into(),
            ports: vec![4000],
            state: crate::proto::ForwardState::Connected,
        },
        crate::proto::Forward {
            session: "gone".into(),
            host: "homelab".into(),
            ssh: "homelab".into(),
            ports: vec![5000],
            state: crate::proto::ForwardState::Connected,
        },
    ];
    model.apply(Endpoint::Local, LinkUpdate::Sample(here), now);
    model.apply(homelab, live_sample(1_000, Some(claude("s1", 0))), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    select_first_session(&mut model);
    assert_eq!(model.selected_agent(now).unwrap().session.id, "s1");
    let shown = screen(&model);
    assert!(shown.contains("localhost:3000 waiting"), "{shown}");
    assert!(!shown.contains("localhost:4000"), "{shown}");
    assert!(!shown.contains("localhost:5000"), "{shown}");
    model.machine_page = Some(model.rows(now)[0].name.clone());
    let shown = screen(&model);
    assert!(
        shown.contains("localhost:5000 → homelab for a session that left"),
        "{shown}"
    );
    assert!(shown.contains("magi forwards close gone"), "{shown}");
    assert!(!shown.contains("localhost:4000 →"), "{shown}");
    assert_eq!(
        model.moved_forwards(now),
        [(Endpoint::Local, "s1".to_string())],
        "only the forward to a machine s1 has left is closed; one for an ended session stays"
    );
    let LinkUpdate::Sample(mut unknown) = live_sample(1_000, None) else {
        unreachable!()
    };
    unknown.forwards = vec![crate::proto::Forward {
        session: "s1".into(),
        host: "server".into(),
        ssh: "me@homelab".into(),
        ports: vec![3000],
        state: crate::proto::ForwardState::Connected,
    }];
    model.apply(Endpoint::Local, LinkUpdate::Sample(unknown), now);
    assert_eq!(
        model.moved_forwards(now),
        [],
        "a host named only by its ssh login is still the machine s1 runs on"
    );
}

#[test]
fn the_launcher_groups_a_project_across_machines_lists_active_ones_first_and_starts_the_choice() {
    let (mut model, links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(in_project("s1", "web"))),
        now,
    );
    model.apply(homelab, live_sample(1_000, None), now);
    model
        .folders
        .insert(Endpoint::Local, model::Folders::Asking);
    model.folders.insert(homelab, model::Folders::Asking);
    model.apply(
        Endpoint::Local,
        LinkUpdate::Folders(vec![
            "/home/me/api".into(),
            "/home/me/clones/web".into(),
            "/home/me/web".into(),
        ]),
        now,
    );
    let menu = model.menu(now);
    assert_eq!(menu.machines[1].listing, launcher::Listing::Asking);
    model.apply(
        homelab,
        LinkUpdate::Folders(vec!["/home/felipe/dev/web".into()]),
        now,
    );
    let menu = model.menu(now);
    let projects: Vec<(&str, bool, usize)> = menu
        .projects
        .iter()
        .map(|project| (project.name.as_str(), project.active, project.folders.len()))
        .collect();
    assert_eq!(
        projects,
        [
            ("No project", false, 2),
            ("web", true, 2),
            ("api", false, 1)
        ]
    );
    assert!(menu.projects[0].chat);
    assert_eq!(menu.projects[1].folders[0].dir, "/home/me/web");
    let mut launcher = launcher::Launcher::new(menu);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| view::launcher::float(frame, Some(&launcher)))
        .unwrap();
    let shown = format!("{:?}", terminal.backend().buffer());
    for text in [
        "New session",
        "No project",
        "~/Chats",
        "web",
        "active",
        "suggested",
        "Codex",
    ] {
        assert!(shown.contains(text), "missing {text:?}\n{shown}");
    }
    launcher.key(Key::Tab);
    let launcher::Outcome::Start(choice) = launcher.key(Key::Enter) else {
        panic!("enter should start the chosen session");
    };
    start_chosen(&mut model, &links, choice, now);
    assert!(model.folders.is_empty());
    assert_eq!(
        model.flashed(Instant::now()),
        Some(("couldn't open it: that machine isn't connected", false))
    );
}

#[test]
fn r_asks_the_agent_to_continue_a_session_outside_rex_and_skips_one_inside() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(claude("s1", 0))),
        now,
    );
    select_first_session(&mut model);
    handle_batch([key(Key::Char('r'))].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(
        model.flashed(Instant::now()),
        Some((
            "couldn't continue it in Rex: that machine isn't connected",
            false
        ))
    );
    let (mut model, mut links) = fleet();
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(in_rex(claude("s1", 0)))),
        now,
    );
    select_first_session(&mut model);
    handle_batch([key(Key::Char('r'))].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.flashed(Instant::now()), None);
}

fn in_project(id: &str, project: &str) -> AgentSession {
    let mut session = claude(id, 0);
    session.project = project.into();
    session.title = id.into();
    session
}

#[test]
fn a_session_magi_opened_is_selected_when_it_appears_and_a_failed_open_selects_nothing() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    model.apply(
        homelab,
        live_sample(1_000, Some(in_project("old", "api"))),
        now,
    );
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    model.await_session(homelab, "api", now);
    let LinkUpdate::Sample(mut sample) = live_sample(1_000, Some(in_project("old", "api"))) else {
        unreachable!()
    };
    sample.sessions.push(in_project("new", "api"));
    model.apply(homelab, LinkUpdate::Sample(sample.clone()), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert_eq!(model.selected_agent(now).unwrap().session.id, "new");

    let (mut model, mut links) = fleet();
    model.apply(
        homelab,
        live_sample(1_000, Some(in_project("old", "api"))),
        now,
    );
    model.await_session(homelab, "api", now);
    model.apply(
        homelab,
        LinkUpdate::Reply(crate::proto::Reply {
            ok: false,
            text: "couldn't open a session: no".into(),
        }),
        now,
    );
    model.apply(homelab, LinkUpdate::Sample(sample), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert_ne!(
        model
            .selected_agent(now)
            .map(|agent| agent.session.id.clone()),
        Some("new".into())
    );
}

#[test]
fn arrows_walk_machines_and_their_sessions_in_order_and_pick_where_a_move_goes() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    let nas = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 3));
    model.apply(
        Endpoint::Local,
        live_sample(1_000, Some(in_project("a", "api"))),
        now,
    );
    model.apply(
        homelab,
        live_sample(1_000, Some(in_project("b", "web"))),
        now,
    );
    model.apply(nas, live_sample(1_000, None), now);
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    let selected = |model: &Model| {
        model
            .selected_agent(now)
            .map(|agent| agent.session.id.clone())
    };
    handle_batch([key(Key::Down)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(selected(&model).as_deref(), Some("a"));
    handle_batch(
        [key(Key::Down), key(Key::Down)].into_iter(),
        &mut model,
        &mut links,
    )
    .unwrap();
    assert_eq!(selected(&model).as_deref(), Some("b"));
    handle_batch([key(Key::Last)].into_iter(), &mut model, &mut links).unwrap();
    let Some(model::Focus::Machine(name)) = model.selection(Instant::now()) else {
        panic!("the last row should be a machine");
    };
    assert!(name.contains("nas"), "{name}");
    handle_batch([key(Key::Enter)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.machine_page.as_deref(), Some(name.as_str()));
    handle_batch([key(Key::Escape)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.machine_page, None);
    handle_batch(
        [key(Key::First), key(Key::Down)].into_iter(),
        &mut model,
        &mut links,
    )
    .unwrap();
    assert_eq!(selected(&model).as_deref(), Some("a"));
    handle_batch([key(Key::Char('m'))].into_iter(), &mut model, &mut links).unwrap();
    let first = model.pending_move.clone().unwrap().to;
    handle_batch([key(Key::Right)].into_iter(), &mut model, &mut links).unwrap();
    let second = model.pending_move.clone().unwrap().to;
    assert_ne!(first, second);
    assert!(["homelab", "nas"].contains(&second.as_str()), "{second}");
    handle_batch([key(Key::Right)].into_iter(), &mut model, &mut links).unwrap();
    assert_eq!(model.pending_move.clone().unwrap().to, first);
}

#[test]
fn the_launcher_says_why_a_machine_listed_no_projects() {
    let (mut model, mut links) = fleet();
    let now = Instant::now();
    let homelab = Endpoint::Tailnet(Ipv4Addr::new(100, 64, 0, 2));
    model.apply(homelab, live_sample(1_000, None), now);
    handle_batch([key(Key::Char('c'))].into_iter(), &mut model, &mut links).unwrap();
    assert!(asked_and_failed(
        &model,
        "couldn't ask for its projects: that machine isn't connected"
    ));
    model.folders.insert(homelab, model::Folders::Asking);
    model.apply(
        homelab,
        LinkUpdate::Reply(crate::proto::Reply {
            ok: false,
            text: "magi is missing".into(),
        }),
        now,
    );
    handle_batch(std::iter::empty(), &mut model, &mut links).unwrap();
    assert!(asked_and_failed(&model, "magi is missing"));
}

#[test]
fn a_codex_session_can_be_moved_like_a_claude_one() {
    let (mut model, _) = fleet();
    let now = Instant::now();
    let mut session = in_rex(claude("s1", 0));
    session.agent = crate::proto::Agent::Codex;
    model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
    select_first_session(&mut model);
    let keys = footer(&model);
    assert!(keys.contains("m move"), "{keys}");
}

#[test]
fn a_session_asking_to_trust_its_folder_can_be_answered_from_the_overview() {
    let (mut model, _) = fleet();
    let now = Instant::now();
    let mut session = in_rex(claude("s1", 0));
    session.state = crate::proto::SessionState::Waiting(crate::proto::Waiting::Trust);
    model.apply(Endpoint::Local, live_sample(1_000, Some(session)), now);
    select_first_session(&mut model);
    let keys = footer(&model);
    assert!(
        keys.contains("y trust") && keys.contains("n leave"),
        "{keys}"
    );
}
