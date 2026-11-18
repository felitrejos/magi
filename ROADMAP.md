# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code or
  Codex session to another machine and back, and sending work to other
  machines that comes back by itself. That core needs only SSH.
- **One program.** `magi` is a single Go program with SSH as its only
  requirement. It never depends on the overview.
- **The overview is a separate install.** The live view in a Rex pane, the
  agents on every machine and Tailscale live in `magi-overview`, an optional
  install for Rex users. It keeps working but gets no new features for now.
- **Reliable before new.** A handoff should be boring. Work on making moves
  safe and measured comes before anything else.
- **tmux as good as Rex.** Everything the core does in a Rex tab it does in
  a tmux window too.
- **Leave it to the tools.** What Claude Code, Codex, Rex or tmux already do
  stays there.
- **Suggest, don't act.** magi suggests moves and machines but never moves
  or starts anything by itself. It only cleans up after itself. It closes a
  port forward left pointing at a machine its session has moved on from,
  and `magi status` and the next handoff stop a copy that a failed handoff
  left running.
- **A layer for other tools.** The CLI and `magi mcp` give scripts, other
  orchestrators and Claude itself magi's actions. Moving sessions, sending
  work, and reading machines, sessions and diffs need only SSH. The
  live-control actions (messaging a session, answering its prompts,
  following its work) need `magi-overview`.
- **Follow Rex.** While Rex is in preview, magi supports only its latest
  release. Bump `REX_API` and `rexAPI` with each one.

## 0.9.0

- **Split the install.** `magi` ships as one Go program that needs only
  SSH. The overview ships as `magi-overview`.
- **Never two live copies.** A move that fails leaves the session live on
  exactly one machine, and a handoff is refused when the destination
  already runs the session.
- **Budgets that fit the hook.** Every step of a handoff gets a time budget
  that fits inside the hook's time limit of 200 seconds.
- **Remember unfinished moves.** A copy that a failed handoff couldn't stop
  is recorded. `magi status` shows it and stops it in the same run, and so
  does the next handoff of that session.
- **Block typing after a move.** A session that has already moved refuses
  new input on the machine it left.
- **Safer pulls.** `pull` closes the old tab only after it succeeds, and
  treats a session waiting on a prompt as busy.
- **A limit for desktop sessions.** A desktop session that never closes
  itself gets a time limit.
- **tmux handoffs.** A handoff closes the tmux original and switches
  attached tmux clients to the new window. Tab records get safer, and
  `pull` opens a tmux window when there is no Rex.
- **The MCP without the overview.** Machine load is read over SSH, so
  choosing a machine works with no agents. `sessions` and `diff` are served
  over SSH too. The live-control tools stay with the overview.
- **Measure it.** Scripted round trips with Rex and with tmux, plus injected
  failures, with the success rate published.

## Decided against

- **Live screens through Rex's `data.connect`.** It would need a persistent
  connection and a terminal emulator inside magi, for what the agents' own
  panes already show.
- **Attaching images from magi.** Dropping an image on the session's own Rex
  pane already uploads it to that machine.
