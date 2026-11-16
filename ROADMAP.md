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

## Phase 8: Rex between machines

Each machine runs its own Rex server, and a Rex client reaches the others by
host label (`rex -S homelab`) over Tailscale HTTPS or SSH, with the list of
hosts kept by your home server. What that can replace:

1. **Events instead of polling.** Done for state: each agent follows
   `rex events --all --json` on its machine and keeps every pane's own
   status report (Claude Code reports working, idle or blocked, with a line
   on what it's doing). A report, bell, notification or exit updates magi at
   once, and the report's line shows as the session's activity. Next: drop
   the `block ls` refresh for `process_changed`.
2. **Live screens.** `data.connect` streams a block's content as it changes,
   for remote sessions too, instead of capturing it.
3. **Find panes through Rex.** `rex block ls -A --json` with
   `rex block call process` (foreground pid, argv, cwd, last exit) instead of
   reading process environments, which works on every host.
4. **Port forwards.** Done: a moved session's dev ports go through
   `rex forward` as tunnel blocks in its Rex session on the new machine,
   reconnecting on their own (SSH stays only for tmux hosts). Next: show each
   forward's state (active, away, orphaned) on its session.
5. **Folders and files.** `list_dir` to browse any machine's folders when
   starting a session; `download_begin` and `upload_begin` to fetch a file for
   a popup or attach an image to a message.
6. **Land on the agent.** `rex zoom --on` the agent's pane when opening its
   tab, and label agent panes with `session.set_block_label`.

Popups for a session on another machine get their content (a diff, a plan, a
file) through the magi agent there, which already talks to the viewer over
Tailscale, and draw it locally. SSH stays only for what Rex has no call for:
copying a work tree when a session moves, and running git on the machine it
moves to.

Rex is in preview, so magi and cielago follow its latest release only
(today 0.1.0-preview.19, API revision 14), with no fallbacks for older
servers: bump `REX_API` and `rexAPI` with each Rex release.

## Phase 9: sessions

- **Messages while it works.** Queue them the way Claude Code does, safely,
  so they can't land in a prompt that opens meanwhile.
- **Permission mode.** Switch a session between ask, accept edits, plan and
  auto from magi.
- **Starting in a new folder.** Claude asks whether to trust a folder it has
  never opened; decide whether starting it from magi should answer that.
- PR status and Codex approval prompts.
- Moving Codex sessions with cielago, and suggesting them for a move.
- Forwards: clean up one that points at a machine the session moved on from,
  and handle dev servers listening on IPv6 localhost.

## Before 0.6.0

- A dry run of the release workflow, which now builds magi and cielago into
  one archive, and of `install.sh` installing both.
- Release notes, and magi and cielago versioned together.
- Re-record the README's GIFs, which show the old interface.
