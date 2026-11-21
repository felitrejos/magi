# Roadmap

## Direction

- **Move a session, send work.** magi does those two things for Claude
  Code and nothing else.
- **Ask, don't type.** After you install the plugin, you ask Claude.
- **Reliable before new.** A handoff should be boring.
- **Suggest, don't act.** magi never moves or starts anything by itself,
  except the watcher taking back a session you asked to hand back.
- **Leave it to the tools.** What Claude Code, SSH and tmux already do
  stays there.

## 1.0

Done. magi moves sessions, sends work, and has a Mac menu bar app that
shows every machine and session, says what is waiting on you and opens a
session where it runs. [docs/support.md](docs/support.md) lists what a
1.x release keeps the same.

## Still open

Nothing new is planned. These are checks 1.0 shipped without, and
[Known limits](docs/support.md#known-limits) has the rest.

- The menu bar app was built and used on macOS 27 only. It hasn't run
  on macOS 26.
- Its install hasn't run on a new Mac account or a standard account, and
  no update has come through a published release yet.
- No CI job moves a session between two real machines.
- The app isn't notarized, so a copy downloaded with a browser is
  blocked. Notarizing stays possible without changing the app.

## Not planned

Moving cloud sessions, GitHub or pull request workflows, moving or
starting sessions by itself, native Windows and WSL, and new ways to
delegate work.

## Decided against

- **Actions in the menu bar.** Opening the session is enough.
- **Typing into other sessions.** Remote Control already does that.
- **Polling SSH from the app.** The watcher already holds a connection.
- **Private APIs.** The app uses public APIs only.
