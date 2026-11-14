# Changelog

## Unreleased

- `cielago move <session-id> <host>` moves a Claude Code session running on
  this machine from outside it, for tools such as magi. An idle session is
  stopped and handed off at once; a session in the middle of a turn moves
  when the turn ends, through its own Stop hook. Each step and the outcome
  come out as JSON lines with `--json`. Codex sessions are refused.
- Hosts can have a budget for tasks: `max_agents`, `min_free_memory` (such as
  `"4G"`) and `quiet_hours` (such as `"22:00-07:00"`).
- Tasks go to the first host in the config, in order, that has room, judged
  by its budget and magi's live numbers, instead of the least loaded one. The
  host checks its budget again as the task starts, so two tasks can't take
  its last slot, and a full host is skipped for the next with the reason.
- `machines` and `cielago doctor` show each host's budget and why a host has
  no room; `cielago init` keeps the order of the hosts in the config.
- `task_reply` sends a follow-up to a delegated Claude that has finished a
  turn. It resumes the same headless session in the same copy, so it keeps
  its context, and its changes move the branch `cielago/task-<id>` forward.
  A delegate's copy now stays after `task_result` for follow-ups;
  `cancel_task` removes it, and `cielago tasks clean` removes copies idle for
  a day, with the delegate's transcript there.
- When magi reports the Claude plan window 90% or more used, `delegate`,
  `start_session` and `task_reply` don't start, and say when it resets, unless
  the user says to go ahead anyway; from 75% they warn. `machines` shows the
  plan's windows and what each machine's agent sessions use.

## 0.4.0

- Send work to another machine without moving the session. The plugin now
  ships a tool server with `machines`, `run_elsewhere`, `delegate`,
  `start_session`, `task_status`, `task_result` and `cancel_task`, available
  in every Claude Code session.
- Each task runs in a copy of your exact working tree, in a checkout of its
  own on the other machine, in a Rex or tmux tab you can watch. Your checkout
  there and the session that asked are never touched.
- Tasks start at once and run in the background: `run_elsewhere` returns the
  exit code, the end of the output and the full log; `delegate` returns the
  remote Claude's final message and its changes as the branch
  `cielago/task-<id>`.
- A delegated Claude edits only its own copy and runs anything else only
  through rules you approve; it never runs with permission checks bypassed.
- cielago picks the least loaded machine from `magi fleet`, or the first that
  answers when magi isn't there.
- `cielago tasks` lists tasks; `show`, `cancel` and `clean` collect or stop
  them. `cielago doctor` says whether magi reports machine load.
- A project's `.cielago.toml` says what to run when it arrives: `setup` runs
  in a pane below Claude, then each of `services` in its own pane. Claude is
  told they're running. `cielago pull` runs the setup before Claude continues.
- `cielago allow` shows a project's `.cielago.toml` and approves it for that
  project; changing the file needs a new approval.
- When a session leaves a machine, by `cielago pull` or another handoff, the
  tab cielago opened for it there closes, panes included.

## 0.3.0

- Committed Git LFS files arrive with their content even when their objects
  were never uploaded: cielago copies the LFS objects the other machine lacks.
- Secret files inside ignored folders come along; folders listed in `exclude`
  are skipped.
- A remote written with an SSH host alias (`git@github-work:acme/api`) matches
  the same repository written with the real host.
- Works when the other machine's login shell is fish, csh or tcsh, and on
  BusyBox systems such as Alpine.
- All shell code lives in scripts that shellcheck checks, and every path and
  name reaches them as an argument, never as part of the script.
- Secret files that are symlinks come along as symlinks, and folders cielago
  can't read no longer stop a handoff.
- A Claude session that doesn't exit within a minute of the handoff is killed,
  so its transcript is always retired.
- `cielago unpark` works during a merge or rebase.
- A new folder on the other machine is filled from the repository's `origin`
  first, so shallow clones and large histories hand off.
- Trying again after a failed handoff picks up from the copy it left, even if
  you kept working meanwhile.
- When the new session waits for you to trust its folder, the error says so.
- A session started inside a submodule hands off the whole project.
- The new session's start is timed by the other machine's clock, so clock
  drift between machines doesn't fail a handoff.
- tmux sessions cielago starts no longer inherit `LC_ALL=C`.
- `cielago pull <host>` says when the last session there was already brought
  back; `cielago unpark` also clears submodules.
- `cielago doctor` checks for git 2.31 or newer and gives each host its own
  time limit; `cielago init` keeps an existing tab command.
- Error messages show what went wrong without shell noise.
- Builds use the latest Go 1.27 patch release; CI runs staticcheck and
  govulncheck.

## 0.2.0

- Repositories without any commits can be handed off.
- Folders that aren't git repositories are mirrored, deletions included, and a
  copy changed since a session left it is never overwritten. A session can
  come back to the folder it started in.
- Uncommitted Git LFS files arrive with their real content.
- Uncommitted changes inside git submodules move too, and a submodule the
  other machine doesn't have yet is set up there with its own remote.
- Secret and Git LFS files are found across the whole repository when Claude
  runs in a subfolder.
- `exclude` takes file and folder names; a pattern with a slash is refused.

## 0.1.0

First release.

- Hand a running Claude Code session to another machine by asking for it in
  plain words; it continues there in a new session with Remote Control on.
- Moves the conversation, commits and branch, staged and unstaged changes,
  untracked files, secret files, Claude's project memory and subagent
  transcripts. Folders that aren't git repositories are copied whole.
- Background work is stopped first and described in a note the new session
  uses to restart it.
- Opens the session in its own tab in [Rex](https://superlogical.com/rex),
  switching Rex to it, or in tmux.
- `cielago pull` brings a session back from a machine that can't reach yours.
- Parks the checkout a session leaves, so it can come back to it, and refuses
  to overwrite work made anywhere else.
- `cielago init`, `doctor`, `status` and `unpark`.
- Works between macOS and Linux machines in any direction.
