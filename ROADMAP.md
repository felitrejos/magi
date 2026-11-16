# Roadmap

magi is built around [Rex](https://www.superlogical.com/rex): every session it
drives lives in a Rex tab, and Rex is how magi sees, types into and opens
them. This is what's left, in the order it's planned.

## Direction

magi is the control room for the Claude Code and Codex sessions you already
run, across your own machines. It shows what needs you, lets you answer,
read and review from one place, and recommends where work should run.

- **A multiplexer, not an orchestrator.** Splitting work across agents is the
  agents' job. magi's job is to make whatever they do clear: subagents,
  background tasks, worktrees, teammates and sessions one agent started for
  another, each shown under the session it belongs to.
- **Projects first.** Sessions are grouped by project, as in the Claude
  desktop app; the machine is a badge on each session, not a group.
- **Recommend, never act alone.** magi warns and suggests (move this session,
  start that one elsewhere, a plan is nearly spent) but never moves or starts
  anything on its own.
- **All of Rex, not one pane.** The magi pane is home. Diffs, plans, Markdown,
  files, images and machine detail open as Rex floating panes over it, like
  modals, and close back to the conversation.
- **Built for the Mac client, working everywhere.** Mac viewers get SF
  Symbols and the Mac's own sensors; every viewer draws the same conversation
  and pages in the terminal's own text.

## Phase 3: every session in Rex

Done:
- `c` starts a Claude Code or Codex session in any project on any machine, in
  its project's Rex tab, and magi selects it.
- `x` stops a running turn.
- `r` continues a session from the Claude desktop app or another terminal in a
  Rex tab, as a copy, so magi can message, approve and move it. The original
  is left alone; archive it in the app when you like.
- Multi-line messages and pastes (`⇧↵` for a new line), sent with Rex's
  paste event.

## Phase 4: the redesign

Done:
- **Projects:** related repos grouped across machines, foldable rows, `p`
  and `P` to move and rename (your edits always win), and Haiku naming the
  projects of new repos.
- **Layout, after Dia:** a narrow borderless sidebar with machines (and
  their Rex icons), sessions and plan usage in three zones; the
  conversation on an inset card; the selected row as a pill.
- **Machine view:** CPU, memory, network and disk as titled tiles, each
  with its own scaled chart and colour-coded legend; each core's own chart beside what's
  using the machine (with the session each process belongs to); CPU
  temperature on Linux and on the Mac (through its HID sensors).
- **The launcher:** `c` opens a Rex floating pane like a modal: pick a
  project (active and recent ones first, or No project for a chat in
  `~/Chats` in its own `chats` Rex session), the machine (magi suggests the
  least busy one with room, with its reason), Claude or Codex and the
  model (Claude's aliases, or the models that machine's Codex lists); it starts
  the session, closes itself and magi selects the new session.
- **Conversation, after the desktop app:** replies in magi's own quieter
  colours, your messages in a raised block, each run of
  tool calls folded into one line (`t` shows them), the call still running
  with its time, and the session's plan as a card at the end.
- **Session family tree:** subagents, background shells, worktrees and
  sessions started inside another, nested under their session with their
  state. Selecting one shows a subagent's own conversation, live, or a
  shell's output.

Next:
1. **Rex popups.** `d` lists the session's changed files with each diff
   beside it, and `v` its outputs (its plan and the documents it wrote), in
   floating panes. Next: send a selection and a question back to the session
   from the diff (the idea from leaf's `@`), and images.

## Phase 5: the rest of the redesign

Done:
- **Zen view:** `z` switches to a layout of its own for watching rather
  than talking: a summary of the fleet and plan usage on top, then a column
  per machine with its load, memory and temperature, and its sessions as
  quiet two-line cards (needs you first, with what each is doing or waiting
  for). Arrows move between cards, `↵` opens the session's Rex tab, and `m`
  moves it, ←→ picking the machine. No conversation is read in Zen.
- **Alerts feed:** `a` opens the alert history in a Rex floating pane, by
  day, each alert with how long it took to resolve, live as it changes.
- **Shared project edits:** project moves, renames, machine labels and the
  namer's suggestions travel through the agents, newest wins, so every
  viewer shows the same projects; folds and the view stay per viewer.
- **Search:** `/` in the conversation highlights matches; `n` and `N` step
  through them and scroll each into view.
- **Links:** links in the conversation and the Outputs preview are real
  terminal hyperlinks, so Rex opens them on a click.

## Phase 6: Rex only, and two apps (done)

- magi runs only in a Rex pane and says so elsewhere (`magi fleet --json`
  still works anywhere). The in-pane launcher and alerts view are gone, as
  is the Ghostty and kitty detection: the launcher, alerts, diff and outputs
  always open as Rex floating panes.
- cielago installs on its own (`install.sh | sh -s cielago`, its own release
  archive) and keeps tmux and SSH tunnels for machines without Rex, since it
  is useful without magi.
- The README presents the two apps, short.

## Phase 7: placement, as recommendations (done)

- A move is suggested when a machine is under sustained pressure (CPU
  averaging 80% over a minute, a 5-minute load at or above its core count,
  or memory 90% full), the session is one of the causes (a core of CPU, or a
  gigabyte of memory when memory is short), and another machine has room.
  The suggestion says why, and the machine with the most room is the target
  `m` proposes.
- A plan warns before it runs out: from half used, if its pace would use it
  up before it resets, with when, and the next session suggested in the other
  agent when that plan has room.
- magi never moves or starts a session by itself.
- Codex sessions aren't suggested for a move, since cielago only moves Claude
  Code sessions. Moving Codex sessions is in Phase 9.

## Phase 8: Rex between machines (done)

Each machine runs its own Rex server, and a Rex client reaches the others by
host label (`rex -S homelab`) over Tailscale HTTPS or SSH.

- **Events instead of polling.** Each agent follows `rex events --all --json`
  for every pane's own status report (Claude Code reports working, idle or
  blocked, with a line on what it's doing), and for layout changes: a pane's
  window is listed again when its session's layout changes, otherwise once a
  minute.
- **Opening a session on another machine.** `↵` goes through that machine's
  Rex host, found by label or endpoint, and the Rex app's client there, and
  focuses the agent's own pane, not just its tab.
- **Port forwards** go through `rex forward` as tunnel blocks. A forward whose
  session has ended or moved on shows on the machine running it, with the
  command that closes it.

Decided against, for now:
- **Live screens over `data.connect`.** It needs a persistent control
  connection, a binary WebSocket protocol and a terminal emulator in magi, to
  replace one `rex capture` a second that runs only while a session is
  waiting on you.
- **Finding panes and folders through Rex.** The magi agent on each machine
  already reads process environments, folders and files itself; `block call
  process`, `list_dir` and `download_begin` would add a call per pane.
- **Attaching an image to a message.** Dropping it on the session's own Rex
  pane already uploads it to that machine. From magi it would need Rex's
  upload stream, which has no CLI, or a larger request limit in magi.

Rex is in preview, so magi and cielago follow its latest release only
(today 0.1.0-preview.19, API revision 14), with no fallbacks for older
servers: bump `REX_API` and `rexAPI` with each Rex release.

## Phase 9: sessions

Done:
- **Messages while it works.** A message for a busy session waits in its
  machine's agent and goes in once the session is idle, so it can't land in a
  prompt that opens meanwhile. The session shows what's queued; `↵` edits it,
  and sending it empty drops it.
- **Permission mode.** `⇧tab` cycles a Claude Code session's mode (manual,
  accept edits, plan, auto) the way Claude Code does, and the session header
  shows the mode it's in, read from its screen after a switch and from its
  transcript otherwise.
- **Codex approval prompts** show as waiting on you: Codex reports them to Rex.

Open:
- **Starting in a new folder.** Claude asks whether to trust a folder it has
  never opened, and writes no session file until it's answered, so magi
  doesn't see that session yet. Look for it by the pane cielago just opened,
  show it as waiting, and let `y` trust it.
- PR status for a session's branch.
- Moving Codex sessions with cielago, and suggesting them for a move.
- Forwards: clean up one that points at a machine the session moved on from,
  and handle dev servers listening on IPv6 localhost.

## Phase 10: Rex is the workspace, magi the overview (done)

You work in the real tools, Claude Code and Codex in Rex or the Claude app,
and magi puts every session in the right place and keeps watch.

- **Placement.** Starting a session opens it in the project's Rex session
  (one per project, as magi groups them) on the chosen machine, in the
  repo's tab (one per repo, working directory checked), with Remote
  Control on, and jumps there.
- **Mirror tabs for subagents.** Agents keep their own subagents. When one
  starts, the magi agent on that machine opens a tab beside its parent,
  `↳ <subagent>`, running `magi follow`, which draws the subagent's work
  live and read-only, shows its result briefly when it ends, then closes.
  Delegated tasks are real sessions with their own tabs.
- **The viewer becomes an overview.** The conversation view, message box,
  search and links go. A session shows its state, what it's doing or asking,
  context, cost, model, subagents, tasks and ports; `↵` goes to its pane,
  `y`/`n` answer a permission prompt, `⇧tab` switches mode, `m` moves it,
  `d`/`v` show its diff and outputs, `c` starts one. Zen stays.

## Phase 11: fleet client and CLI (done)

One client, shared with `magi fleet`, and commands: `magi sessions
[--json] | send | answer | choose | mode | move | start | events`. A session
is named by the start of its id, its title, or a part of its title at least
four letters long. `send` queues for a busy session, `move` waits for the
move to finish, `events` prints what needs you as JSON lines (raised,
updated, resolved).

## Phase 12: `magi mcp`

The same commands as MCP tools for Claude Desktop, Claude Code and other
orchestrators: read tools (machines, sessions, a session's recent work,
diff, alerts, plans) and action tools (message, answer, mode, move, start).
cielago's MCP stays for work sent from inside a session.

## Phase 13: desktop first

Sessions magi starts run as named Remote Control sessions in their Rex tabs,
so the Claude app on desktop or phone can drive them; `start` returns the
link. Check first that they appear in the desktop app. Sessions the desktop
app runs locally stay visible, with `r` to continue them in Rex.

## Before 0.6.0

- A dry run of the release workflow, which now builds magi and cielago into
  one archive, and of `install.sh` installing both.
- Release notes, and magi and cielago versioned together.
- Re-record the README's GIFs, which show the old interface.
