# Security

magi is one program, `magi`, that runs on your own machines. You don't
type its commands. The Claude Code plugin runs it, and you ask Claude for
what you need. It talks to your other machines only through your own
`ssh`. Its other connections are two HTTPS downloads from GitHub, the
release binary and the tested versions list, both described below. It runs
no server and opens no network listener, except the ports you ask it to
forward, which listen on `127.0.0.1`.

## What runs where

- **The machine you start from** runs `magi` when Claude Code runs the
  plugin's hooks, tools and skill, and as the `magi watch` background
  service once you set a machine up.
- **The other machine** runs short shell scripts that magi sends over SSH,
  and the session or task itself in a tmux window. Setup installs the
  plugin there, so Claude Code runs the same hooks and tools on it.
- **The plugin** adds the handoff skill, two hooks, the MCP server and a
  launcher, `bin/magi`. All of them only run `magi`.
- **Claude itself** runs three magi commands. The handoff skill runs
  `magi arm`, and Claude runs `magi wait` and `magi tasks wait` as
  background shell commands. Everything else is a tool call.

## The launcher's download

The plugin holds no binary. Its launcher, `plugin/bin/magi`, is a short
shell script that every hook, tool and command goes through.

- It reads the plugin's version from the plugin's own `plugin.json` and
  runs `~/.magi/bin/magi-<version>`.
- If that file is missing, it downloads `magi_<os>_<arch>.tar.gz` and
  `checksums.txt` from the GitHub release `v<version>` of
  `felitrejos/magi` with `curl`. It allows only HTTPS, also on redirects.
- It computes the archive's SHA-256 and compares it with the line for
  that archive in `checksums.txt`. If they differ, or the line is missing,
  it installs nothing and stops.
- It unpacks only `magi` from the archive, moves it into place and points
  the link `~/.magi/bin/magi` at it. The watcher and port forwards run
  that link.
- The archive and its checksum come from the same release. The check
  catches a damaged or swapped download. It doesn't protect you from a
  bad release. What you trust is GitHub's TLS and whoever can write
  releases to that repository. Releases aren't signed.
- `MAGI_VERSION` and `MAGI_RELEASE_URL` in the environment change which
  version it runs and where it downloads from. `make dev` uses the first
  for a local build. With `MAGI_RELEASE_URL` it also accepts a `file://`
  address, which the tests use.
- The plugin reaches a machine through Claude Code's plugin marketplace,
  from the same repository.

## Setting up another machine

The `setup_machine` tool runs when you ask Claude to set magi up with a
machine. Claude says what it installs and waits for your yes. It does
these in order.

- It runs `ssh` to the machine with `true`. If that fails, it stops and
  says what only you can do, such as accepting the host key or adding an
  SSH key. It never accepts a host key for you.
- It reads the kernel name on both machines and stops if either runs in
  WSL, which magi doesn't support.
- It looks for `claude` in the usual install paths there.
- It saves the machine in `~/.config/magi/config.toml` here, with the
  project folders it found on both machines. It writes no `secrets` or
  `exclude` there. The defaults under "Moving a session" apply when the
  config has none.
- On the other machine it adds the marketplace `felitrejos/magi` with
  `claude plugin marketplace add` and installs the plugin with
  `claude plugin install magi@magi`. If the plugin
  is there already, it turns it on or updates it when it is older than
  the one here. Then it runs the plugin's launcher there once with
  `version`, which makes the download described above on that machine.
  It installs nothing else there and uses no `sudo`.
- Here it installs the `magi watch` service described under "The
  watcher".
- It checks git, `tar`, tmux, the Claude Code version, the claude.ai
  login and the project folders on both machines. It reads the
  login with `claude auth status`.
- It creates a `magi-try-*` folder in your first local root, moves a
  session in it to the other machine and back, then removes the folder
  and its transcripts on both machines. That makes three Claude turns on
  your account.

The `uninstall` tool stops and removes the watcher service here and
deletes `~/.magi/bin`. It keeps your config and the rest of `~/.magi`,
and it changes nothing on other machines. You remove the plugin yourself
with `/plugin`.

## SSH

- Every connection is `ssh` to a destination from your config, with
  `BatchMode=yes`, so it never asks for a password. magi never sets
  `StrictHostKeyChecking` or `UserKnownHostsFile`, so your SSH config and
  `known_hosts` decide which hosts are trusted.
- Because nothing can prompt, a host whose key isn't in your `known_hosts`
  fails to connect instead of asking you to accept it. That holds unless
  your own SSH config accepts new keys.
- Connections to one machine share a control socket in `~/.magi/ssh`,
  which closes 60 seconds after its last use. A port forward uses a
  connection of its own.
- Git objects travel with `git push` and `git fetch` over that connection.
  Files travel as a `tar` stream over it.

## Moving a session

- magi copies the branch, commits, staged and unstaged changes, the
  transcript and Claude's project memory. Anything pasted into the chat
  moves with the transcript.
- Of the files git ignores, only those matching the `secrets` patterns in
  your config are copied, disk to disk. magi doesn't print their contents.
  Logins, SSH keys and credential stores never move. When the config has
  no `secrets`, the patterns are `.env` and `.env.*`.
- Files git doesn't track yet and doesn't ignore travel with the uncommitted
  work, in submodules too, unless their name is in `exclude`. An untracked
  file or folder with a name in `exclude` stays behind, and the list
  before the move names it as left out. Tracked files under those names
  still go. When the config has no `exclude`, the names are
  `node_modules`, `.venv`, `dist`, `build` and `target`.
- `exclude` also applies to the search for secret files.
- A folder that isn't a git repository is copied whole, without the names
  in `exclude`. Files in it that match `secrets` go as part of the folder.
- On the receiving machine magi recreates your branch and uncommitted
  work. It refuses if that checkout has work or commits of its own.
- A handoff is refused when the session already runs on the destination.
  One that fails stops the new copy and leaves the session running where
  it was.
- If you trust a project's folder in Claude Code on the machine a session
  leaves, magi marks its copy on the receiving machine as trusted in
  `~/.claude.json` there, so the moved session starts without asking.
  That holds in both directions. It replaces that file only if nothing
  changed it meanwhile. A folder you haven't trusted where the session
  was is never trusted where it goes.
- A move in either direction runs the same steps. The receiving machine
  opens the session in a tmux window with Remote Control, runs an
  approved project's `setup` and `services` in panes beside it, and the
  result gives the session's Remote Control link.
- When SSH to a machine dies or runs out of time during a move, the error
  says that machine "stopped answering".
- The Stop hook (`magi hook stop`) runs the handoff, with a time limit of
  200 seconds. The prompt hook (`magi hook prompt`) refuses new input in
  the copy a session left. It reads the session id from the hook's input
  and the marker in `~/.magi/moved`, and contacts nothing.

### What magi shows before a move

Before a move copies anything, magi prints a block headed "What goes to"
and the machine's name. It lists these.

- The branch or detached HEAD, its commits and the number of changed
  files, submodules included.
- Files git doesn't track yet, by name. It names at most five, then says
  how many more there are.
- The size of the changed and new files, as an upper bound.
- The untracked names that `exclude` leaves out.
- Secret files, by name only and with the same limit.
- The number of files that go through Git LFS.
- For a folder that isn't a git repository, its file count and size
  without the names in `exclude`, and those names.
- What the project's `.magi.toml` holds, as whether it has a `setup`
  command, its `services` and its `ports`. For a file you haven't
  approved here, it says that nothing in it runs on this move, and gives
  the commands and the file's SHA-256.
- The conversation and its project memory.
- A last line on what doesn't go. That is anything running and, in a git
  repository, anything git ignores apart from the secret files.

Where you see it depends on how the move starts.

- `magi arm`, which the handoff skill runs, prints it after "Armed",
  "Asked" or "Not armed". Claude passes it on to you.
- The `move_session` tool returns it to Claude, for a move in either
  direction.
- When a session comes here, through `move_session` or the watcher, the
  block says what comes back, read on the other machine. It leaves out
  the `.magi.toml` line. The watcher writes it to its log.
- For a hand back, `magi arm` builds the block with the `secrets` and
  `exclude` of the machine that takes the session back. That machine's
  watch loop leaves them in its watcher file here.

### When Claude asks you first

magi asks before it copies when the block holds any of these.

- A secret file.
- A file git doesn't track yet.
- 50 MB or more of changed and new files, or a folder that isn't a git
  repository of that size.

Then `magi arm` prints "Not armed" and arms nothing, so the end of the
turn moves nothing. The handoff skill tells Claude to show you the list
and ask. After your yes Claude runs `magi arm` again with `--yes`. This
holds for a hand back too. `move_session` moves nothing and returns the
list until Claude calls it again with `confirmed` set, which its
description allows only after your yes.

The rule lives in magi, so Claude can't skip the first answer. The yes
itself passes through Claude. Nothing checks that you gave it, as with
every tool Claude calls for you. If `magi arm` can't read what goes, it
arms nothing and returns the error.

### A move that was cut off

- Every move writes a journal, `~/.magi/moves/<session-id>.json`, before
  it changes anything, and removes it as its last step. A journal whose
  move is no longer running belongs to a move that was cut off.
- magi settles those on its own. It checks every journal when the
  plugin's MCP server starts, which is when a Claude Code session starts,
  and once a minute from the watcher. The `status` tool and the next move
  of that session check too. Each check contacts the other machine over
  SSH for at most 15 seconds.
- A settled move is written to `handoffs.jsonl` with what magi did, and
  `status` shows it among the recent handoffs. The watcher also writes it
  to its log.
- The live copy is the one that took new prompts since the move started.
  If both did, it is the one still running. If neither did, it is the
  copy the session was leaving, unless the move had reached its last step.
- Recovery stops the other copy's Claude process with `TERM`. It
  closes the tmux window the move opened, but only while the window still
  carries the tag magi gave it, or the tmux session if the move created
  it. It renames that copy's transcript to
  `<transcript>.handed-off-to-<machine>`, so turns taken there are kept.
  It stops that session's port forwards, and it updates magi's own records
  in `~/.magi` and the parked mark in a git checkout.
- Recovery never deletes a folder or a transcript. When the move created
  a folder that is still there, it tells you the path and leaves it.
- If both copies took new prompts and both or neither is running, magi
  stops nothing. `status` says where each copy is, and the choice is
  yours.
- If the other machine doesn't answer, magi changes nothing there and
  keeps the journal, and tries again on each check. `status` gives the
  journal's path, which you remove yourself if that machine is gone for
  good.

## The watcher

`magi watch` lets a laptop that takes no incoming SSH take sessions back.
`setup_machine` installs it as a service on the machine you set up from.
It also records what runs on each machine, described under "What the
watcher reads".

- It keeps one outgoing SSH connection to each machine in your config. On
  each it runs a small loop that refreshes `~/.magi/watchers/<laptop name>`
  every 2 seconds and prints the ids of hand-back requests addressed to the
  laptop. That file holds the laptop's `secrets` and `exclude` lists.
- When a loop starts, it stops older loops for the same laptop name that
  lost their SSH connection and were left running. To find them it runs
  `ps -A -o pid= -o ppid= -o args=` once, which lists every process on
  that machine with its command line, and it kills only processes that
  carry the loop's own marker and whose top marked parent is process 1.
- A request is the file `~/.magi/wanted/<session-id>.json` on the far
  machine. `magi arm` writes it there when you ask a session to go back to
  a machine that isn't in that machine's config but has refreshed its
  watcher file in the last 15 seconds. It holds the laptop's name, the
  session's process id and the note on what was running.
- The laptop then pulls the session over its own connection, the same way
  `move_session` does, opens it in a tmux window and removes the request. If
  the pull fails, it tries again each minute. A request can only make the
  laptop pull a session from the machine the request is on.
- The loop deletes a request addressed to the laptop whose session process
  is gone.
- The service is a launchd agent on macOS
  (`~/Library/LaunchAgents/magi-watch.plist`, log in
  `~/Library/Logs/magi-watch.log`) or a systemd user unit on Linux
  (`~/.config/systemd/user/magi-watch.service`, log in the journal). It
  runs as you, never as root, starts at login and restarts if it stops.
  The service file holds the path `~/.magi/bin/magi` and a copy of your
  `PATH`. The `uninstall` tool stops it and removes the file.
- Once a minute the watcher settles moves that were cut off. It also
  checks whether `~/.magi/bin/magi` points at a newer build and whether
  magi's config changed, for example after `setup_machine` added a
  machine. If either did, the watcher exits once no session is being
  taken back, and the service starts it again.
- The log holds machine names, short session ids, folder paths, errors,
  what magi did to settle a move, and the block of what comes back, with
  its branch and file names. magi writes no conversation text to it.

### What the watcher reads

The same loop also reports what runs on each machine. `magi watch` runs it
on this machine too, without SSH, and there it writes no watcher file and
looks for no requests. The loop only reads. Every 2 seconds it does this.

- It reads the first line of each `~/.claude/sessions/<pid>.json` whose
  process is alive. That is Claude Code's own record of a session, with
  its id, folder, name and state. The loop sends those lines whole, and
  only when one of them changed. magi keeps the fields listed below.
- When they changed and a session is waiting, it runs
  `claude agents --json` once to learn what the session waits on, and
  sends the output. It doesn't run that command otherwise.

About every 30 seconds the loop sends one line of numbers.

- Cores, load average and memory, from `sysctl` on macOS or `/proc` on
  Linux.
- The used and total size of the disk your home folder is on, from
  `df -Pk "$HOME"`.
- For each session, CPU time and memory summed over its process and the
  processes under it. On macOS this comes from one
  `ps -A -o pid= -o ppid= -o time= -o rss=`. On Linux it reads
  `/proc/<pid>/stat` for every process and keeps the same four values.
  Either way it covers every process on the machine, by number, parent,
  CPU time and memory only, and reads no command lines.
- On Linux the CPU time includes child processes that already exited.
  macOS doesn't expose that to other processes, so there a child that
  started and exited between two samples isn't counted.

magi writes what it heard to `~/.magi/live/<machine>.json` on the machine
that runs `magi watch`. Only you can read the file, and each write
replaces it whole. It holds these.

- Whether the machine answers and when magi last heard from it. magi sets
  "not answering" when the connection ends. When `magi watch` is stopped
  with `TERM` or `INT`, or restarts itself, it first marks every machine
  as not answering in these files. If it is killed outright or the
  machine loses power, the files keep their last content, so a reader
  should still treat a `heard` time older than about 40 seconds as the
  machine not answering.
- Cores, load, and used and total memory and disk in MB.
- For each session, its id, title, folder, its state
  (busy, idle or waiting, and on what), since when, CPU as a percentage of
  one core and memory in MB. The title is the session's name, or the name
  of its folder when it has none.

It holds no conversation text and no file contents.

## Sending work

- `run_elsewhere`, `delegate` and `session_elsewhere` work in a fresh copy
  under `~/.magi/tasks` on the other machine and never write to your
  checkouts there.
- Claude proposes the machine and the work, and you approve both. The
  tools that start, continue or stop work are not marked read-only, so an
  MCP client can ask before using them.
- A delegated Claude runs headless in `acceptEdits` mode by default, or in
  `dontAsk` or `auto` when Claude asks for one. Other prompts are answered
  "no" unless you allowed the tool, and it never runs with permission
  checks bypassed.
- magi starts a delegated Claude with `MAGI_DELEGATE` set. The magi MCP
  server in that Claude then leaves out every tool that is your call.
  Those are `setup_machine`, `move_session`, `allow_project`, `unpark`,
  `run_elsewhere`, `delegate`, `session_elsewhere`, `task_reply`,
  `task_result`, `cancel_task`, `uninstall` and `status`. It keeps
  `machines`, `sessions`, `diff` and `task_status`. So a delegate can't
  send work on, move a session or approve a project. `status` is left
  out because calling it settles moves, and a delegate's server doesn't
  settle moves or tidy tasks when it starts either.
- When a finished `delegate` is collected, its changes are written into
  your working tree here and kept as the branch `magi/task-<id>`. If they
  clash with edits made here since the task started, nothing is applied
  and they stay on the branch.
- `magi mcp` talks only over its standard input and output, to the program
  that started it.

## Reading machines and sessions

- To propose a machine, magi reads its system, architecture, installed
  build tools, cores, load average and memory over SSH, with `uname`,
  `sw_vers` or `/etc/os-release`, `command -v`, `docker info` or
  `podman info`, and `sysctl` or `/proc`. It counts working Claude
  sessions with `claude agents --json`. This machine is read the same way
  without SSH.
- The `sessions` tool reads `claude agents --json` and
  `~/.claude/sessions/*.json` on this machine and on each machine in your
  config. `magi wait` reads the same.
  The `diff` tool runs `git diff HEAD` and lists untracked files in a
  session's folder. All three only read.

## Tested versions

- `remote/tested.json` in this repository holds, for Claude Code, the
  oldest version magi supports, the newest version its contract tests
  passed with, and a list of versions known to fail. It also holds a
  frozen `codex` range. This code ignores it. It stays only so released
  0.9 installs, which still read it, can parse the file.
- magi downloads that file from
  `https://raw.githubusercontent.com/felitrejos/magi/main/remote/tested.json`
  and keeps it in `~/.magi/tested.json`. It downloads again when that copy
  is more than an hour old, and only for a move in either direction or
  the checks in `setup_machine`. The request has a 5 second limit. A download magi can't
  parse is thrown away. If the download fails, magi uses the copy it has, and
  with no copy the list built into the program.
- Before a move copies anything, magi runs `claude --version` on both
  machines. It refuses when
  either version is older than the oldest supported, on the failing
  list, or newer than the newest tested. The checks in `setup_machine`
  fail on the same versions.
- The list only widens or narrows which versions magi allows. Someone who
  could change it on GitHub, or the copy in `~/.magi`, could block your
  moves or let an untested version through. It can do nothing else. magi
  runs nothing from it and sends nothing because of it.
- A nightly GitHub Actions workflow, `.github/workflows/nightly.yml`,
  keeps the list current. It runs on GitHub's Ubuntu and macOS runners,
  not on anyone's own machine. It downloads the newest Claude Code from
  its official release source, checks the download against its published
  SHA-256, and runs the contract tests with it on both runners.
- The workflow needs one repository secret, `CLAUDE_CODE_OAUTH_TOKEN`,
  which the owner makes with `claude setup-token`. The contract tests use
  it to run real Claude Code turns on the owner's account. Whoever can
  read that secret or change the workflow on `main` can use that account
  through Claude Code. The jobs that run the tests have read-only access
  and keep no git credentials. Only the job that commits the list can
  write to the repository.
- When both runners pass, the workflow raises the newest tested version
  in the file and pushes that commit to `main`. When a job fails, it
  opens a GitHub issue that names the failed job, or comments on the open
  one. It never writes the failing list. That list is edited by hand.

## Project files and ports

- A project's `.magi.toml` runs nothing until you approve that exact
  file on that machine. Its `setup` command, `services` and `ports` then
  run after moves and before `run_elsewhere` and `delegate` tasks.
- When magi meets a file you haven't approved, its note gives Claude the
  file's commands and its SHA-256. Claude calls `allow_project` with the
  folder and that hash.
- `allow_project` reads the file again. If its SHA-256 isn't the one
  Claude passed, it allows nothing. So Claude can't approve a file that
  changed after magi described it.
- Then magi asks you through MCP elicitation. Claude Code shows a dialog
  of its own with the file's path, the machine and the exact `setup` and
  `services` commands and ports. Claude can't answer that dialog. magi
  records the approval only when you accept, and anything else allows
  nothing.
- The approval is the file's path and SHA-256 in `~/.magi/allowed`. Any
  edit changes the hash, so the file needs a new approval.
- A file is refused if it has unknown keys, or control or format
  characters in a command, so the dialog shows what will run.
- Its `ports` are forwarded from the machine a session left with
  `ssh -N -L`, bound to `127.0.0.1`, so other devices on that network
  can't use them. The `status` tool lists them. A forward stops when the
  session comes back or moves on.

## Files magi writes

Settings are in `~/.config/magi/config.toml`, or under `XDG_CONFIG_HOME`
when that is set. State is in `~/.magi`.

| Path in `~/.magi` | What it holds |
|---|---|
| `bin/` | The magi binaries the launcher downloaded, one per version, and the link `magi` to the current one |
| `armed/` | A session set to move when its turn ends, and a lock while it moves |
| `wanted/` | Hand-back requests waiting for a watcher |
| `watchers/` | One file per laptop watching this machine, with that laptop's `secrets` and `exclude` lists |
| `watch.lock` | Keeps a second `magi watch` from starting |
| `live/` | One file per machine, written by `magi watch`, with whether it answers, its load, memory and disk, and its sessions with their state, CPU and memory |
| `moved/` | Where each session that left went |
| `handoffs.jsonl` | Time, session id, machines and folders of each handoff, and what magi did to settle a move that was cut off |
| `moves/` | One journal per move in progress or cut off, kept until the move is settled |
| `forwards/` | Forwarded ports and the process forwarding them |
| `tabs/`, `setup/` | The tmux window of a moved session and its setup state |
| `tasks/` | Task records and logs here, working tree copies on the other machine |
| `allowed` | Hashes of the `.magi.toml` files you approved |
| `folders` | Folders magi copied here for projects that aren't git repositories, with a fingerprint of each |
| `ssh/` | SSH control sockets |
| `tested.json` | The downloaded list of Claude Code versions magi was tested with |

Outside those, magi writes the project and session files it moves, the
trust entry in `~/.claude.json`, and the watcher's service file and log.

## Releases

Release builds come from GitHub Actions with actions pinned to commit
hashes. A release runs when a `v*` tag is pushed. It publishes only after
every CI job passes, which is the test suite on macOS, Ubuntu and Alpine.
It stops if the tag doesn't match the version in the plugin's
`plugin.json`. It builds `magi`
for macOS and Linux on arm64 and amd64, packs each in a `tar.gz` and
writes their SHA-256 sums to `checksums.txt`. Those are the files the
launcher downloads. There is no install script.

No CI job moves a session between two real machines. Before a release
the owner asks Claude to "check magi on my homelab", which runs `setup_machine`'s
checks and its round trip between a Mac and a Linux server.

## Reporting a problem

Please report security issues privately through GitHub's "Report a
vulnerability" on this repository rather than in a public issue.
