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

The core is built, and its open points are under Known limits in
[docs/support.md](docs/support.md). What is left for 1.0 is a read-only
Mac menu bar app. It shows every machine and session, says what is waiting
on you, and opens a session through its Remote Control link. It never
moves or starts anything and never types into a session.

- A native AppKit status item with a SwiftUI panel, on macOS 26 and later.
- It reads `~/.magi/live` when a file there changes. It never polls and
  never runs SSH or magi.
- One icon with a state for idle, busy, a session waiting and no machine
  answering.
- A notification when a session starts waiting on you.
- A first run that checks magi is set up and offers notifications and
  launch at login. You can skip every step.
- Idle CPU near 0%, measured and published.
- magi installs it and keeps it at the plugin's version. It ships signed
  with magi's own certificate, without notarization.

### Not in 1.0

Moving cloud sessions, GitHub or pull request workflows, moving or
starting sessions by itself, native Windows and WSL, and new ways to
delegate work.

## Decided against

- **Actions in the menu bar.** Opening the session is enough.
- **Typing into other sessions.** Remote Control already does that.
- **Polling SSH from the app.** The watcher already holds a connection.
- **Private APIs.** The app uses public APIs only.
- **Notarizing for 1.0.** magi installs the app, so Gatekeeper skips it.
