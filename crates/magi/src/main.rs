mod agent;
mod bounded;
mod children;
mod cmd;
mod collect;
mod conversation;
mod handoff;
mod log;
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

#[derive(PartialEq)]
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
    let mut file = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Value(value) if value == "agent" && mode == Mode::Viewer => mode = Mode::Agent,
            Value(value) if value == "fleet" && mode == Mode::Viewer => mode = Mode::Fleet,
            Long("json") if mode == Mode::Fleet => json = true,
            Value(value) if value == "page" && mode == Mode::Viewer => mode = Mode::Page,
            Value(value) if value == "launch" && mode == Mode::Viewer => mode = Mode::Launch,
            Long("diff") if mode == Mode::Page => page.diff = true,
            Long("remove") if mode == Mode::Page => remove = true,
            Long("title") if mode == Mode::Page => page.title = parser.value()?.string()?,
            Value(value) if matches!(mode, Mode::Page | Mode::Launch) && file.is_none() => {
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
                    Mode::Viewer | Mode::Fleet | Mode::Agent | Mode::Install
                ) =>
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
        Mode::Page => {
            let file = file.ok_or("magi page needs a file")?;
            if page.title.is_empty() {
                page.title = file
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            }
            Command::Page(file, page, remove)
        }
    })
}
