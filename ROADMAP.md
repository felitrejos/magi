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
- **Show what gets copied.** Before a move, magi prints a short list of
  what goes across. What exists now is that list for `magi arm`,
  `magi move`, `magi pull` and `magi watch`. It has the branch and its
  changed files, files git doesn't track yet, the secret files by name and
  never by value, and the setup, services and ports in the project's
  `.magi.toml`. magi never turns SSH host key checks off. Still open are
  these.
  - A busy session moved with `magi move` hands off from the Stop hook and
    prints no list.
  - The defaults for `exclude` and `secrets` are only suggestions in
    `magi init`. A config written by hand gets none, and nothing applies
    them when a move runs.
  - Files git doesn't track and doesn't ignore travel even when their
    names are in `exclude`. The list shows them, and nothing leaves them
    out.
- **A support matrix.** Mac to Linux and Linux to Linux are tested on every
  release. WSL is either tested end to end or marked unsupported, in the
  docs and in `magi init`. A table shows what works for Claude Code and for
  Codex, since Codex has no end-of-turn hook and moves differently.
- **Docs a stranger can use.** A five-minute tutorial that goes from one
  machine to a second one, a troubleshooting page, a short versioning policy
  that says what 1.x promises not to break, and a changelog.
- **Session state.** `sessions` says what each session is doing, and
  asking "does anything need me?" is one call. What exists now is a first
  line in `sessions` that names every Claude Code session waiting on you
  across your machines, and any machine it couldn't check. Claude Code's
  state, busy, waiting or idle, comes from Claude Code itself through
  `claude agents --json`, so magi adds no hooks for it. Codex shows busy
  or idle, read from the last turn event in the session's rollout file.
  Still open are these.
  - A Codex session waiting on an approval doesn't show as waiting. The
    Codex window keeps its sessions in its own background server, and the
    `codex app-server` magi starts to ask can't see into it.
  - A Codex turn that was killed midway can still read as busy.
- **Wait for a session, move another one.** What exists now is
  `magi wait`, which returns once a session on any machine isn't busy and
  says whether it's idle, waiting on you or ended. It follows a session
  that is being handed off to another machine. The `move_session` MCP tool
  moves a session on this machine, other than the one you're in, as
  `magi move` does. Claude proposes the move and waits for your yes. It
  doesn't move a session that is busy or waiting on you. `wait` is a
  command and not a tool, like `magi tasks wait`, because a tool call that
  blocks for minutes runs into MCP time limits.
- **`magi watch` streams changes.** What exists now is a watcher that,
  over the one SSH connection per machine it already keeps for hand backs,
  also streams what changes there. It runs the same loop for this machine.
  Every 2 seconds the loop reads Claude Code's session registry files and
  Codex's open-thread lock files, and it sends a snapshot only when
  something changed. It runs `claude agents --json` only when a session is
  waiting, to learn on what. About every 30 seconds it sends the machine's
  cores, load, memory and home disk, and each session's CPU and memory
  summed over its process tree, from one `ps`. magi writes
  `~/.magi/live/<machine>.json` with whether the machine answers, when it
  was last heard, those stats and each session's id, title, folder, state,
  since when, CPU and memory. Hand backs and state changes show within
  about 2 seconds, and Codex busy or idle within about 6. Idle cost was
  measured on a Mac over 10 minutes against a budget of 0.5% of one core
  and 30 MB. The loop on the watched machine used 0.30% to 0.45% and
  2.8 MB at its peak. magi itself used 0.013% and 13.5 MB. Still open are
  these.
  - Idle cost isn't measured on Linux yet.
  - CPU counts only the processes running at each sample. A child that
    started and exited between two samples isn't counted.
  - Whether the watcher notices an old Codex thread being reopened is
    unverified.
  - A reader of the live file must treat a `heard` time older than about
    40 seconds as the machine not answering. The file keeps its last
    content when `magi watch` itself stops. The menu bar app has to apply
    that rule.
- **The menu bar app.** SwiftUI, Mac only, open source. It reads the files
  `magi watch` writes in `~/.magi/live`. It shows each machine's load
  against its cores, memory, disk and whether it answers, which includes a
  machine not heard from for about 40 seconds. For each session it shows
  the project, the state, how long it's been in it, and its CPU and
  memory. For a Codex session that time counts from when the watcher first
  saw the state. It also shows where sessions moved and background tasks,
  which the live files don't hold. A badge
  shows when a Claude Code session is waiting on you. A Codex approval
  wait shows only once `sessions` can see it. The only click opens a session
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
