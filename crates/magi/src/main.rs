mod agent;
mod bounded;
mod children;
mod collect;
mod conversation;
mod handoff;
mod log;
mod mirrors;
mod net;
mod opener;
mod proto;
mod prs;
mod service;
mod sessions;
mod show;
mod statusline;
mod tailscale;
mod tools;
mod typing;
mod viewer;

use std::process::ExitCode;
use std::time::Duration;

use lexopt::prelude::*;

const RECENT_ITEMS: usize = 20;
const WAIT_WITHIN: Duration = Duration::from_secs(30 * 60);

const USAGE: &str = "magi: your coding agents across your machines

Moving sessions and sending work, over SSH:
  magi init                                            set up this machine and the machines to send work to
  magi doctor [MACHINE...]                             check that everything a move needs is in place
  magi move SESSION MACHINE                            move a Claude Code or Codex session there, and wait
  magi pull MACHINE [SESSION-ID]                       bring a session back from there and continue it here
  magi open FOLDER [--codex] [--model M]               start a session here, in a Rex tab for its folder
  magi tasks                                           work sent to other machines, and its state
  magi tasks wait|show|cancel ID                       wait for a task, collect it, or stop it
  magi tasks clean                                     collect finished tasks and drop old copies
  magi status                                          recent moves and parked checkouts
  magi forwards [close SESSION-ID]                     ports forwarded to sessions that moved
  magi allow [DIR]                                     let a project's .magi.toml run after moves
  magi unpark [DIR]                                    stop treating a checkout as parked
  magi mcp                                             every action as MCP tools, for Claude Code, Claude Desktop and others

The overview, with Rex and Tailscale on every machine:
  magi [--port PORT]                                   open the viewer (press a for alerts)
  magi agent install [--port PORT] [--interval MS]     run the agent in the background, now and at login
  magi agent uninstall                                 stop it and remove the background service
  magi agent [--port PORT] [--interval MS]             run the agent in this terminal
  magi fleet [--json] [--port PORT]                    print every machine's load and agents once
  magi sessions [--json]                               every session on every machine
  magi send SESSION TEXT                               message a session, queued until it's idle
  magi answer SESSION yes|no                           allow or deny what it asks to run
  magi choose SESSION N                                pick option N of its question
  magi start MACHINE FOLDER|--chat [--codex] [--model M]
                                                       start a session there, in a Rex tab for its folder
  magi events                                          what needs you, as JSON lines, as it happens
  magi subagents [--json]                              every running subagent and the step it's on
  magi recent SESSION [SUBAGENT] [--items N]           its latest messages and steps, one line each
  magi wait SESSION [SUBAGENT] [--for 10m]             until it stops working, or the subagent ends
  magi alerts                                          the alert history as a feed, by day
  magi statusline install|uninstall                    a Claude Code status line with context % and plan limits

  SESSION is the start of its id, its title, or at least four letters of its title.
  move takes any session by name with the overview; without it, give the full session id.
  SUBAGENT is its id or its label, matched the same way.
  magi --version
  magi --help

Options:
  --port PORT     agent port (default 2015)
  --interval MS   sampling interval in milliseconds, 100 to 60000 (default 1000)";

enum Command {
    Viewer(viewer::Config),
    Fleet(viewer::Config, bool),
    Agent(agent::Config),
    Install(agent::Config),
    Uninstall,
    StatusLineInstall,
    StatusLineUninstall,
    StatusLine,
    Page(std::path::PathBuf, viewer::Page, bool),
    Launch(std::path::PathBuf),
    Alerts,
    Follow(std::path::PathBuf, String, String),
    Cli(u16, viewer::Cli),
    Mcp(u16),
    Version,
    Help,
}

const REMOTE_COMMANDS: [&str; 13] = [
    "init",
    "doctor",
    "pull",
    "status",
    "unpark",
    "allow",
    "open",
    "forwards",
    "tasks",
    "arm",
    "hook",
    "forward-run",
    "mcp",
];

fn for_remote(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        Some("move") => {
            args.iter().any(|arg| arg == "--json" || arg == "--stay")
                || args
                    .get(1)
                    .is_some_and(|session| handoff::is_session_id(session))
        }
        Some(name) => REMOTE_COMMANDS.contains(&name),
        None => false,
    }
}

fn run_remote(args: Vec<std::ffi::OsString>) -> ExitCode {
    use std::os::unix::process::CommandExt;
    match handoff::remote_program() {
        Ok(program) => {
            let error = std::process::Command::new(program).args(args).exec();
            eprintln!("magi: couldn't run {}: {error}", handoff::REMOTE);
        }
        Err(error) => eprintln!("magi: {error}"),
    }
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let text: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if for_remote(&text) {
        return run_remote(args);
    }
    let command = match parse(lexopt::Parser::from_env()) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("magi: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Viewer(config) => viewer::run(config),
        Command::Fleet(config, json) => viewer::fleet(config, json),
        Command::Agent(config) => agent::run(&config),
        Command::Install(config) => service::install(&config),
        Command::Uninstall => service::uninstall(),
        Command::StatusLineInstall => statusline::install(),
        Command::StatusLineUninstall => statusline::uninstall(),
        Command::StatusLine => statusline::run(),
        Command::Page(path, page, remove) => viewer::page(&path, page, remove),
        Command::Launch(socket) => viewer::launch(&socket),
        Command::Alerts => viewer::alerts(),
        Command::Follow(transcript, session, block) => {
            viewer::follow(&transcript, &session, &block)
        }
        Command::Cli(port, command) => viewer::cli(port, command),
        Command::Mcp(port) => viewer::mcp(port),
        Command::Version => {
            println!("magi {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("magi: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Viewer,
    Fleet,
    Agent,
    Install,
    Uninstall,
    StatusLineInstall,
    StatusLineUninstall,
    StatusLine,
    Page,
    Launch,
    Alerts,
    Follow,
    Mcp,
    Sessions,
    Send,
    Answer,
    Choose,
    Move,
    Start,
    Events,
    Subagents,
    Recent,
    Wait,
}

const CLI_COMMANDS: [(&str, Mode); 10] = [
    ("sessions", Mode::Sessions),
    ("send", Mode::Send),
    ("answer", Mode::Answer),
    ("choose", Mode::Choose),
    ("move", Mode::Move),
    ("start", Mode::Start),
    ("events", Mode::Events),
    ("subagents", Mode::Subagents),
    ("recent", Mode::Recent),
    ("wait", Mode::Wait),
];

#[derive(Default)]
struct Flags {
    json: bool,
    codex: bool,
    chat: bool,
    model: Option<String>,
    items: Option<usize>,
    within: Option<Duration>,
}

fn is_cli(mode: Mode) -> bool {
    CLI_COMMANDS.iter().any(|(_, known)| *known == mode)
}

fn cli(mode: Mode, rest: Vec<String>, flags: Flags) -> Result<viewer::Cli, lexopt::Error> {
    let mut rest = rest.into_iter();
    let mut next = |what: &str| {
        rest.next()
            .ok_or_else(|| lexopt::Error::from(format!("missing {what}")))
    };
    Ok(match mode {
        Mode::Sessions => viewer::Cli::Sessions { json: flags.json },
        Mode::Subagents => viewer::Cli::Subagents { json: flags.json },
        Mode::Recent => viewer::Cli::Recent {
            session: next("the session")?,
            subagent: next("").ok(),
            items: flags.items.unwrap_or(RECENT_ITEMS),
        },
        Mode::Wait => viewer::Cli::Wait {
            session: next("the session")?,
            subagent: next("").ok(),
            within: flags.within.unwrap_or(WAIT_WITHIN),
        },
        Mode::Events => viewer::Cli::Events,
        Mode::Send => {
            let session = next("the session")?;
            let words: Vec<String> = std::iter::from_fn(|| next("").ok()).collect();
            if words.is_empty() {
                return Err("missing the message".into());
            }
            viewer::Cli::Send {
                session,
                text: words.join(" "),
            }
        }
        Mode::Answer => {
            let session = next("the session")?;
            let allow = match next("yes or no")?.as_str() {
                "yes" | "y" | "allow" => true,
                "no" | "n" | "deny" => false,
                other => return Err(format!("answer yes or no, not {other:?}").into()),
            };
            viewer::Cli::Answer { session, allow }
        }
        Mode::Choose => {
            let session = next("the session")?;
            let option = next("the option number")?
                .parse::<u8>()
                .ok()
                .filter(|option| (1..=9).contains(option))
                .ok_or("the option is a number from 1 to 9")?;
            viewer::Cli::Choose { session, option }
        }
        Mode::Move => {
            let session = next("the session")?;
            viewer::Cli::Move {
                session,
                to: next("the machine")?,
            }
        }
        Mode::Start => {
            let machine = next("the machine")?;
            let place = if flags.chat {
                proto::OpenIn::Chats
            } else {
                proto::OpenIn::Folder(next("the folder")?)
            };
            viewer::Cli::Start {
                machine,
                place,
                codex: flags.codex,
                model: flags.model,
            }
        }
        _ => unreachable!("only the commands for other clients come here"),
    })
}

fn duration(text: &str) -> Result<Duration, lexopt::Error> {
    let (number, unit) = text.split_at(text.trim_end_matches(char::is_alphabetic).len());
    let number: u64 = number
        .parse()
        .map_err(|_| format!("{text:?} isn't a length of time like 90s, 10m or 1h"))?;
    let secs = match unit {
        "" | "s" => number,
        "m" => number * 60,
        "h" => number * 3600,
        _ => return Err(format!("{text:?} isn't a length of time like 90s, 10m or 1h").into()),
    };
    Ok(Duration::from_secs(secs))
}

fn parse(mut parser: lexopt::Parser) -> Result<Command, lexopt::Error> {
    let mut mode = Mode::Viewer;
    let mut port = proto::DEFAULT_PORT;
    let mut interval_ms: u32 = 1000;
    let mut flags = Flags::default();
    let mut page = viewer::Page {
        title: String::new(),
    };
    let mut remove = false;
    let mut file = None;
    let mut rex_session = None;
    let mut rex_block = None;
    let mut rest: Vec<String> = Vec::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Value(value)
                if mode == Mode::Viewer && CLI_COMMANDS.iter().any(|(name, _)| value == *name) =>
            {
                mode = CLI_COMMANDS
                    .iter()
                    .find(|(name, _)| value == *name)
                    .map(|(_, mode)| *mode)
                    .expect("checked just above");
            }
            Long("json") if matches!(mode, Mode::Sessions | Mode::Subagents) => flags.json = true,
            Long("codex") if mode == Mode::Start => flags.codex = true,
            Long("chat") if mode == Mode::Start => flags.chat = true,
            Long("model") if mode == Mode::Start => {
                flags.model = Some(parser.value()?.string()?);
            }
            Long("items") if mode == Mode::Recent => {
                flags.items = Some(
                    parser
                        .value()?
                        .string()?
                        .parse()
                        .map_err(|_| "--items is a number")?,
                );
            }
            Long("for") if mode == Mode::Wait => {
                flags.within = Some(duration(&parser.value()?.string()?)?);
            }
            Value(value) if is_cli(mode) => rest.push(value.string()?),
            Value(value) if value == "agent" && mode == Mode::Viewer => mode = Mode::Agent,
            Value(value) if value == "fleet" && mode == Mode::Viewer => mode = Mode::Fleet,
            Long("json") if mode == Mode::Fleet => flags.json = true,
            Value(value) if value == "page" && mode == Mode::Viewer => mode = Mode::Page,
            Value(value) if value == "launch" && mode == Mode::Viewer => mode = Mode::Launch,
            Value(value) if value == "alerts" && mode == Mode::Viewer => mode = Mode::Alerts,
            Value(value) if value == "follow" && mode == Mode::Viewer => mode = Mode::Follow,
            Long("session") if mode == Mode::Follow => {
                rex_session = Some(parser.value()?.string()?)
            }
            Long("block") if mode == Mode::Follow => rex_block = Some(parser.value()?.string()?),
            Value(value) if value == "mcp-overview" && mode == Mode::Viewer => mode = Mode::Mcp,
            Long("remove") if mode == Mode::Page => remove = true,
            Long("title") if mode == Mode::Page => page.title = parser.value()?.string()?,
            Value(value)
                if matches!(mode, Mode::Page | Mode::Launch | Mode::Follow) && file.is_none() =>
            {
                file = Some(std::path::PathBuf::from(value))
            }
            Value(value) if value == "statusline" && mode == Mode::Viewer => {
                mode = Mode::StatusLine
            }
            Value(value) if value == "install" && mode == Mode::StatusLine => {
                mode = Mode::StatusLineInstall
            }
            Value(value) if value == "uninstall" && mode == Mode::StatusLine => {
                mode = Mode::StatusLineUninstall
            }
            Value(value) if value == "install" && mode == Mode::Agent => mode = Mode::Install,
            Value(value) if value == "uninstall" && mode == Mode::Agent => mode = Mode::Uninstall,
            Long("port")
                if matches!(
                    mode,
                    Mode::Viewer | Mode::Fleet | Mode::Agent | Mode::Install | Mode::Mcp
                ) || is_cli(mode) =>
            {
                port = parser.value()?.parse()?
            }
            Long("interval") if matches!(mode, Mode::Agent | Mode::Install) => {
                interval_ms = parser.value()?.parse()?
            }
            Short('V') | Long("version") => return Ok(Command::Version),
            Short('h') | Long("help") => return Ok(Command::Help),
            _ => return Err(arg.unexpected()),
        }
    }
    if !(100..=60_000).contains(&interval_ms) {
        return Err("--interval must be between 100 and 60000 ms".into());
    }
    let config = agent::Config {
        port,
        interval: Duration::from_millis(u64::from(interval_ms)),
    };
    if is_cli(mode) {
        return cli(mode, rest, flags).map(|command| Command::Cli(port, command));
    }
    Ok(match mode {
        Mode::Viewer => Command::Viewer(viewer::Config { port }),
        Mode::Fleet => Command::Fleet(viewer::Config { port }, flags.json),
        Mode::Agent => Command::Agent(config),
        Mode::Install => Command::Install(config),
        Mode::Uninstall => Command::Uninstall,
        Mode::StatusLine => Command::StatusLine,
        Mode::StatusLineInstall => Command::StatusLineInstall,
        Mode::StatusLineUninstall => Command::StatusLineUninstall,
        Mode::Launch => Command::Launch(file.ok_or("magi launch needs the viewer's socket")?),
        Mode::Alerts => Command::Alerts,
        Mode::Mcp => Command::Mcp(port),
        Mode::Follow => Command::Follow(
            file.ok_or("magi follow needs a subagent's transcript")?,
            rex_session.ok_or("magi follow needs --session, its parent's Rex session")?,
            rex_block.ok_or("magi follow needs --block, its parent's Rex pane")?,
        ),
        Mode::Page => {
            let file = file.ok_or("magi page needs a file")?;
            if page.title.is_empty() {
                page.title = file
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            }
            Command::Page(file, page, remove)
        }
        _ => unreachable!("the commands for other clients returned above"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ssh_commands_go_to_magi_remote_and_the_overview_ones_stay() {
        let remote =
            |args: &[&str]| for_remote(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
        for args in [
            &["init"][..],
            &["tasks", "wait", "1a2b3c4d"],
            &["hook", "stop"],
            &["mcp"],
            &["move", "--json", "--stay", "fix", "homelab"],
            &["move", "0b7a3c1e-1111-4222-8333-944455556666", "homelab"],
        ] {
            assert!(remote(args), "{args:?}");
        }
        for args in [
            &[][..],
            &["move", "fix login", "homelab"],
            &["mcp-overview"],
            &["sessions", "--json"],
            &["agent", "install"],
        ] {
            assert!(!remote(args), "{args:?}");
        }
    }

    #[test]
    fn a_wait_takes_seconds_minutes_or_hours() {
        assert_eq!(duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(duration("90s").unwrap(), Duration::from_secs(90));
        assert_eq!(duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(duration("1h").unwrap(), Duration::from_secs(3600));
        assert!(duration("soon").is_err());
        assert!(duration("5d").is_err());
    }

    fn cli(args: &[&str]) -> Result<viewer::Cli, String> {
        match parse(lexopt::Parser::from_args(args.iter().copied())) {
            Ok(Command::Cli(_, cli)) => Ok(cli),
            Ok(_) => Err("not a command for other clients".into()),
            Err(error) => Err(error.to_string()),
        }
    }

    #[test]
    fn commands_for_other_clients_take_their_arguments_in_order() {
        assert_eq!(
            cli(&["send", "9fb6", "run", "the", "tests"]),
            Ok(viewer::Cli::Send {
                session: "9fb6".into(),
                text: "run the tests".into()
            })
        );
        assert_eq!(
            cli(&["answer", "9fb6", "no"]),
            Ok(viewer::Cli::Answer {
                session: "9fb6".into(),
                allow: false
            })
        );
        assert!(
            cli(&["answer", "9fb6", "maybe"])
                .unwrap_err()
                .contains("yes or no")
        );
        assert!(cli(&["choose", "9fb6", "12"]).is_err());
        assert_eq!(
            cli(&["start", "homelab", "--chat", "--codex"]),
            Ok(viewer::Cli::Start {
                machine: "homelab".into(),
                place: proto::OpenIn::Chats,
                codex: true,
                model: None
            })
        );
        assert!(
            cli(&["send", "9fb6"])
                .unwrap_err()
                .contains("missing the message")
        );
        assert_eq!(
            cli(&["sessions", "--json"]),
            Ok(viewer::Cli::Sessions { json: true })
        );
    }
}
