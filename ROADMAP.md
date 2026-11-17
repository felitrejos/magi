# Roadmap

magi is built around [Rex](https://www.superlogical.com/rex): every session it
drives lives in a Rex tab, and Rex is how magi sees, types into and opens
them.

## Direction

- **An overview, not a workspace.** You work in Claude Code, Codex and the
  Claude app. magi shows every session on every machine, says what needs you,
  and puts new sessions in the right place.
- **Leave it to the tools.** What Claude Code, Codex or Rex already do (modes,
  conversations, links, file views) stays there. magi keeps only what needs
  the whole fleet: answering prompts, diffs, moves, starts, limits and alerts.
- **Recommend, never act alone.** magi suggests a move or a machine with
  room, and never moves or starts anything by itself. The one thing it
  tidies up on its own is a port forward left pointing at a machine its
  session has moved on from.
- **A layer for other tools.** The CLI and `magi mcp` offer the same actions
  to scripts, orchestrators and Claude itself.

## Done

- **One view for Rex's sidebar:** what needs you on top, then a band per
  machine with its load and a line per session. `enter` goes to a session's
  pane (on any machine, through its Rex host), `y`/`n` and `1`–`9` answer it,
  `v` shows its diff, `m` moves it, `c` starts one, and a machine's page has
  its CPU, memory, network, disk and temperature.
- **Placement:** a new session opens on the chosen machine in a Rex session
  named after its folder and a tab for its repo, with Remote Control on.
  Claude's folder-trust prompt shows as waiting, and `y` trusts the folder.
- **Subagent tabs:** each running subagent gets a read-only `↳` tab beside
  its parent, drawn the way Claude Code draws its work, that closes when Claude
  stops reporting the subagent to Rex. The agent opens them whether or not a
  viewer is watching.
- **Messages while it works** wait in the agent and go in once the session
  is idle.
- **Pull requests:** a session's branch shows its PR state, review and checks
  (through `gh`), and failing checks raise an alert.
- **Limits and alerts:** Claude and Codex plan windows, and alerts when a
  session needs you, a machine stops answering or stays critical, a PR's
  checks fail, or a plan is above 90%. `a` opens the history, and `c` there
  clears what's open.
- **Moves as suggestions:** a move is suggested when a machine is under
  sustained pressure, the session is one of the causes and another machine
  has room.
- **Rex events:** each agent follows `rex events` for every pane's status
  report and layout changes, instead of polling.
- **CLI and MCP:** `magi sessions | send | answer | choose | move | start |
  events`, and the same as MCP tools, with read tools for recent work,
  diffs, alerts and plans.
- **cielago** installs and works on its own: moves and pulls of Claude Code
  and Codex sessions, delegated tasks, port forwards, Rex or tmux. magi
  offers and suggests moves for both.
- **Work goes where it fits:** each agent reports its machine's system,
  architecture and tools (Docker or Podman only when they answer), and
  cielago's `machines` ranks the places for a job by what it needs: native
  first, then a container on the machine with room, lightest first.
- **Work sent elsewhere points home:** a delegated session opens on the
  other machine in a Rex session named like its parent's, in a tab titled
  with the sending machine's emoji and the parent's name (`💻 ← fix login`),
  and magi links the two in the overview.
- **A dispatcher for the Claude app:** a `claude rc` server in `~/dispatch`
  on homelab, with magi's MCP tools, kept running by a systemd unit tied to
  Rex, so a phone can start sessions anywhere.
- **Stale forwards close themselves:** once a session has run on another
  machine for a minute, the viewer has the old forward closed. A forward
  for a session that simply ended stays, with the command to close it.
- **Watching cheaply:** `magi subagents`, `magi recent` and `magi wait` (and
  the same as MCP tools) let an agent follow other work in a few lines, or
  with one blocking call, instead of reading transcripts or asking again and
  again.

Decided against, for now:
- **Live screens over Rex's `data.connect`:** it would need a persistent
  control connection and a terminal emulator in magi, for something the
  agents' own panes already show.
- **Attaching images from magi:** dropping an image on the session's own Rex
  pane already uploads it to that machine.

## Open

Nothing right now.

Rex is in preview, so magi and cielago follow its latest release only, with no
fallbacks for older servers: bump `REX_API` and `rexAPI` with each Rex
release.

## Before 0.6.0

- A dry run of the release workflow, which builds magi and cielago into one
  archive, and of `install.sh` installing both.
- Release notes, and magi and cielago versioned together.
- Re-record the README's GIFs, which show the old interface.
