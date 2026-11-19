# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code or
  Codex session to another machine and back, and sending work to other
  machines that comes back by itself.
- **One program.** `magi` is a single Go program. It needs SSH, tmux on
  the machine that receives work, and the Claude Code or Codex CLI.
- **Reliable before new.** A handoff should be boring. Work on making moves
  safe and measured comes before anything else.
- **Leave it to the tools.** What Claude Code, Codex, SSH or tmux already
  do stays there.
- **Suggest, don't act.** magi suggests machines but never moves or starts
  anything by itself. The one exception is `magi watch`, which takes back a
  session you asked to hand back. Beyond that it only cleans up after
  itself. `magi status` and the next move of a session settle a move that
  was cut off.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself magi's actions. No tool types into
  another live session. The session's own app (Desktop, the Claude app
  through Remote Control, or its terminal) is where you answer it.

## 1.0

1.0 is for a developer with two or more machines who wants one live coding
session, Claude Code or Codex, to follow them from one machine to the next.
The session arrives with its uncommitted work and its context, and there is
exactly one live copy at any time. A read-only menu bar app on the Mac
shows every machine and every session at a glance. magi is not a cloud
workspace.

- **Keep up with Claude Code and Codex.** The core leans on their
  undocumented session files and hooks, and that is the biggest risk to the
  project. What exists now is a list of tested versions,
  `remote/tested.json`, with the oldest supported version, the newest one
  the contract tests passed with and a list of versions that fail. A
  nightly job on a Linux server tests the newest releases and raises the
  newest tested version when they pass. A hand off, a pull and
  `magi doctor` refuse a version outside the list, and pick up a new list
  within an hour without a magi update. Still open are these.
  - Nothing runs the contract tests on macOS. The list comes from the
    Linux nightly only, so a version can be marked tested without a pass
    on a Mac.
  - A failed night shows only in that server's journal and logs. Users see
    it as a refusal in `magi doctor`, and nobody is told.
  - The list of failing versions is edited by hand.
  - The job depends on one server and on the accounts logged in there.
- **Recovery anyone can follow.** When a move is cut off because SSH
  drops, the machine sleeps or the hook is killed, `magi status` says which
  copy is the live one, cleans up the other and prints the exact command to
  continue. What exists now is a journal per move in `~/.magi/moves`,
  written before the move changes anything and removed when it has fully
  finished. `magi status` and the next move of that session read a journal
  left behind and finish or undo the move. They never delete a folder, and
  they stop nothing when they can't tell which copy is live. Tests kill the hook
  partway and take the network down, and they run in the test suite.
  Still open are these.
  - The results of those tests aren't published alongside the round-trip
    numbers.
  - No test covers recovery of a Codex session.
- **Show what gets copied.** Before a move, magi prints a short list of what
  goes across, with files outside git, the secrets files the project listed,
  ports and the services in the project's `.magi.toml`. Secrets show by
  name, never by value. Defaults are conservative and SSH host key checks
  stay on.
- **A support matrix.** Mac to Linux and Linux to Linux are tested on every
  release. WSL is either tested end to end or marked unsupported, in the
  docs and in `magi init`. A table shows what works for Claude Code and for
  Codex, since Codex has no end-of-turn hook and moves differently.
- **Docs a stranger can use.** A five-minute tutorial that goes from one
  machine to a second one, a troubleshooting page, a short versioning policy
  that says what 1.x promises not to break, and a changelog.
- **Session state from hooks.** The plugin's hooks record each session's
  state on its machine, working, waiting on you or idle, and `sessions`
  reports it. Asking "does anything need me?" is then one `sessions` call.
- **Two more MCP tools.** `wait` waits until another session finishes,
  such as one handed off to another machine. `move_session` moves a session
  other than the one you're in, using `magi move`.
- **`magi watch` streams changes.** Today the watcher only takes sessions
  back. Over the same SSH connection per machine it will also stream what
  changes there. That is machine stats, sessions and their state, and the
  CPU and memory each session uses, counting what it started. It reacts to
  hooks and file changes rather than polling. Idle cost is measured on the
  Mac and on Linux against a budget of 0.5% CPU and 30 MB.
- **The menu bar app.** SwiftUI, Mac only, open source. It shows each
  machine's CPU, memory, load, disk and whether it answers. For each session
  it shows the project, the state, how long it's been in it, and its CPU and
  memory. It also shows where sessions moved and background tasks. A badge
  shows when a session is waiting on you. The only click opens a session
  where it lives, in Desktop or its tmux window. It doesn't answer prompts
  or type into sessions. It reconnects quietly after sleep or a network
  change and shows a machine that doesn't answer instead of hiding it.
- **Shipping the menu bar app.** Not notarized. Installed from a Homebrew tap of the
  project's own, with the README saying how to open it the first time
  (right-click and Open, or `xattr -dr com.apple.quarantine`). Updates
  through that tap.

### Not in 1.0

- Moving cloud sessions.
- GitHub or pull request workflows.
- Moving or starting sessions by itself.
- Native Windows.
- New ways to delegate work.

## Decided against

- **Actions in the menu bar.** Answering prompts or typing into a session
  from the menu bar would duplicate the session's own app and need hooks
  that wait for an answer. Opening the session is enough.
- **Tools that type into other sessions.** Remote Control and Desktop
  already let you answer a session yourself.
- **Claude Desktop handling of its own.** magi used to jump Desktop to the
  moved session, reopen a hand back in Desktop and archive the copy left
  behind. All three relied on undocumented Desktop behaviour, so they were
  dropped. A Desktop session now moves like any other and shows in Desktop
  through Remote Control. This is worth revisiting only if Desktop
  documents a way to open a session by id with a ready signal,
  `claude --desktop --fork-session`, or archiving from the CLI.
