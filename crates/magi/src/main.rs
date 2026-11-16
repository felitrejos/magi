mod agent;
mod bounded;
mod children;
mod cmd;
mod collect;
mod conversation;
mod handoff;
mod log;
mod mirrors;
mod net;
mod opener;
mod proto;
mod screen;
mod service;
mod sessions;
mod show;
mod statusline;
mod tailscale;
mod typing;
mod viewer;

use std::process::ExitCode;
use std::time::Duration;

use lexopt::prelude::*;

const USAGE: &str = "magi: live monitoring for your machines and coding agents, in one Rex pane

Usage:
  magi [--port PORT]                                   open the viewer (press a for alerts)
  magi fleet [--json] [--port PORT]                    print every machine's load and agents once
  magi agent install [--port PORT] [--interval MS]     run the agent in the background, now and at login
  magi agent uninstall                                 stop it and remove the background service
  magi agent [--port PORT] [--interval MS]             run the agent in this terminal
  magi statusline install                              add a Claude Code status line that reports context % and plan limits
  magi statusline uninstall                            remove it from ~/.claude/settings.json
  magi page [--diff] [--title TITLE] FILE              show a Markdown file or a diff in a pager
  magi launch SOCKET                                   the new-session launcher the viewer opens in a Rex float
  magi md [--width N] FILE                             print a Markdown file in magi's colours
  magi alerts                                          the alert history as a feed, by day
  magi follow TRANSCRIPT                               a subagent's work, live, for its mirror tab
  magi sessions [--json]                               every session on every machine
  magi send SESSION TEXT                               message a session, queued until it's idle
  magi answer SESSION yes|no                           allow or deny what it asks to run
  magi choose SESSION N                                pick option N of its question
  magi mode SESSION [manual|accept-edits|plan|auto]    switch its permission mode, or cycle it
  magi move SESSION MACHINE                            move it to another machine, and wait
  magi start MACHINE FOLDER|--chat [--project NAME] [--codex] [--model M]
                                                       start a session there, in its project's Rex tab
  magi events                                          what needs you, as JSON lines, as it happens
  magi mcp                                             all of the above as an MCP server, for Claude Desktop and others

  SESSION is the start of its id or a unique part of its title.
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
    Document(std::path::PathBuf, usize),
    Alerts,
    Follow(std::path::PathBuf),
    Cli(u16, viewer::Cli),
    Mcp(u16),
    Version,
    Help,
}

fn main() -> ExitCode {
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
        Command::Agent(config) => agent::run(config),
        Command::Install(config) => service::install(&config),
        Command::Uninstall => service::uninstall(),
        Command::StatusLineInstall => statusline::install(),
        Command::StatusLineUninstall => statusline::uninstall(),
        Command::StatusLine => statusline::run(),
        Command::Page(path, page, remove) => viewer::page(&path, page, remove),
        Command::Launch(socket) => viewer::launch(&socket),
        Command::Document(path, width) => viewer::print_document(&path, width),
        Command::Alerts => viewer::alerts(),
        Command::Follow(transcript) => viewer::follow(&transcript),
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
    Document,
    Alerts,
    Follow,
    Mcp,
    Sessions,
    Send,
    Answer,
    Choose,
    Permissions,
    Move,
    Start,
    Events,
}

const CLI_COMMANDS: [(&str, Mode); 8] = [
    ("sessions", Mode::Sessions),
    ("send", Mode::Send),
    ("answer", Mode::Answer),
    ("choose", Mode::Choose),
    ("mode", Mode::Permissions),
    ("move", Mode::Move),
    ("start", Mode::Start),
    ("events", Mode::Events),
];

fn is_cli(mode: Mode) -> bool {
    CLI_COMMANDS.iter().any(|(_, known)| *known == mode)
}

fn cli(
    mode: Mode,
    rest: Vec<String>,
    json: bool,
    codex: bool,
    chat: bool,
    model: Option<String>,
    project: Option<String>,
) -> Result<viewer::Cli, lexopt::Error> {
    let mut rest = rest.into_iter();
    let mut next = |what: &str| {
        rest.next()
            .ok_or_else(|| lexopt::Error::from(format!("missing {what}")))
    };
    Ok(match mode {
        Mode::Sessions => viewer::Cli::Sessions { json },
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
        Mode::Permissions => {
            let session = next("the session")?;
            let to = match next("").ok().as_deref() {
                None => None,
                Some("manual" | "default") => Some(proto::PermissionMode::Manual),
                Some("accept-edits" | "edits") => Some(proto::PermissionMode::AcceptEdits),
                Some("plan") => Some(proto::PermissionMode::Plan),
                Some("auto") => Some(proto::PermissionMode::Auto),
                Some(other) => {
                    return Err(format!(
                        "{other:?} isn't a mode: manual, accept-edits, plan or auto"
                    )
                    .into());
                }
            };
            viewer::Cli::Mode { session, to }
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
            let place = if chat {
                proto::OpenIn::Chats
            } else {
                proto::OpenIn::Folder(next("the folder")?)
            };
            viewer::Cli::Start {
                machine,
                place,
                project,
                codex,
                model,
            }
        }
        _ => unreachable!("only the commands for other clients come here"),
    })
}

fn parse(mut parser: lexopt::Parser) -> Result<Command, lexopt::Error> {
    let mut mode = Mode::Viewer;
    let mut port = proto::DEFAULT_PORT;
    let mut interval_ms: u32 = 1000;
    let mut json = false;
    let mut page = viewer::Page {
        title: String::new(),
        diff: false,
    };
    let mut remove = false;
    let mut width: usize = 80;
    let mut file = None;
    let mut rest: Vec<String> = Vec::new();
    let mut codex = false;
    let mut chat = false;
    let mut model = None;
    let mut project = None;
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
            Long("json") if mode == Mode::Sessions => json = true,
            Long("codex") if mode == Mode::Start => codex = true,
            Long("chat") if mode == Mode::Start => chat = true,
            Long("model") if mode == Mode::Start => model = Some(parser.value()?.string()?),
            Long("project") if mode == Mode::Start => project = Some(parser.value()?.string()?),
            Value(value) if is_cli(mode) => rest.push(value.string()?),
            Value(value) if value == "agent" && mode == Mode::Viewer => mode = Mode::Agent,
            Value(value) if value == "fleet" && mode == Mode::Viewer => mode = Mode::Fleet,
            Long("json") if mode == Mode::Fleet => json = true,
            Value(value) if value == "page" && mode == Mode::Viewer => mode = Mode::Page,
            Value(value) if value == "launch" && mode == Mode::Viewer => mode = Mode::Launch,
            Value(value) if value == "md" && mode == Mode::Viewer => mode = Mode::Document,
            Value(value) if value == "alerts" && mode == Mode::Viewer => mode = Mode::Alerts,
            Value(value) if value == "follow" && mode == Mode::Viewer => mode = Mode::Follow,
            Value(value) if value == "mcp" && mode == Mode::Viewer => mode = Mode::Mcp,
            Long("width") if mode == Mode::Document => width = parser.value()?.parse()?,
            Long("diff") if mode == Mode::Page => page.diff = true,
            Long("remove") if mode == Mode::Page => remove = true,
            Long("title") if mode == Mode::Page => page.title = parser.value()?.string()?,
            Value(value)
                if matches!(
                    mode,
                    Mode::Page | Mode::Launch | Mode::Document | Mode::Follow
                ) && file.is_none() =>
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
        return cli(mode, rest, json, codex, chat, model, project)
            .map(|command| Command::Cli(port, command));
    }
    Ok(match mode {
        Mode::Viewer => Command::Viewer(viewer::Config { port }),
        Mode::Fleet => Command::Fleet(viewer::Config { port }, json),
        Mode::Agent => Command::Agent(config),
        Mode::Install => Command::Install(config),
        Mode::Uninstall => Command::Uninstall,
        Mode::StatusLine => Command::StatusLine,
        Mode::StatusLineInstall => Command::StatusLineInstall,
        Mode::StatusLineUninstall => Command::StatusLineUninstall,
        Mode::Launch => Command::Launch(file.ok_or("magi launch needs the viewer's socket")?),
        Mode::Alerts => Command::Alerts,
        Mode::Mcp => Command::Mcp(port),
        Mode::Follow => Command::Follow(file.ok_or("magi follow needs a subagent's transcript")?),
        Mode::Document => Command::Document(file.ok_or("magi md needs a file")?, width),
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
        assert_eq!(
            cli(&["mode", "9fb6", "plan"]),
            Ok(viewer::Cli::Mode {
                session: "9fb6".into(),
                to: Some(proto::PermissionMode::Plan)
            })
        );
        assert!(cli(&["choose", "9fb6", "12"]).is_err());
        assert_eq!(
            cli(&["start", "homelab", "--chat", "--codex"]),
            Ok(viewer::Cli::Start {
                machine: "homelab".into(),
                place: proto::OpenIn::Chats,
                project: None,
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
