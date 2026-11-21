# Security

magi is one program, `magi`, that runs on your own machines. The Claude
Code plugin runs it, and you ask Claude for what you need. It reaches your
other machines only through your own `ssh`. Its other connections are
HTTPS downloads from GitHub: the release binary, the tested versions list
and, on a Mac, the menu bar app. It opens no network listener except the
ports you ask it to forward, which listen on `127.0.0.1`.

## What runs where

- **The machine you start from** runs `magi` from the plugin's two hooks,
  its MCP tools and its handoff skill, and as the `magi watch` background
  service once you set a machine up.
- **The other machine** runs short shell scripts magi sends over SSH, and
  the session or task in a tmux window. Setup installs the plugin there.
- **Claude** runs three magi commands itself, `magi arm`, `magi wait` and
  `magi tasks wait`. Everything else is a tool call.

## The launcher's download

The plugin holds no binary. Its launcher, `plugin/bin/magi`, is a short
shell script every hook and tool goes through.

- It runs `~/.magi/bin/magi-<version>` for the plugin's version. If that
  is missing, it downloads `magi_<os>_<arch>.tar.gz` and `checksums.txt`
  from the GitHub release `v<version>` of `felitrejos/magi` over HTTPS.
- It installs nothing unless the archive's SHA-256 matches its line in
  `checksums.txt`.
- The checksum comes from the same release as the archive. It catches a
  damaged download, not a bad release. Releases aren't signed, so you
  trust GitHub's TLS and whoever can write releases to that repository.
- `MAGI_VERSION` and `MAGI_RELEASE_URL` in the environment change which
  version runs and where it and the menu bar app download from, `file://`
  included.

Release builds come from GitHub Actions, with actions pinned to commit
hashes, when a `v*` tag is pushed. A release publishes only after the
test suite passes on macOS, Ubuntu and Alpine.

## Setting up another machine

`setup_machine` runs after Claude says what it installs and you say yes.

- It checks that `ssh` logs in. It never accepts a host key for you.
- It saves the machine and the project folders it found in
  `~/.config/magi/config.toml` here.
- On the other machine it adds the marketplace `felitrejos/magi`, installs
  or updates the plugin with the `claude plugin` commands, and runs the
  launcher once. It installs nothing else and uses no `sudo`.
- Here it installs the watcher service described below and, on a Mac,
  the menu bar app.
- It reads the tool versions and the claude.ai login on both machines.
- It moves a session in a throwaway `magi-try-*` folder there and back,
  then removes it on both machines. That is three Claude turns on your
  account.

`uninstall` removes the watcher service here and `~/.magi/bin`. It keeps
your config, the rest of `~/.magi` and the menu bar app, and changes
nothing on other machines.

## SSH

Every connection is `ssh` to a destination from your config with
`BatchMode=yes`, so it never asks for a password. magi sets no host key
options, so your SSH config and `known_hosts` decide which hosts are
trusted, and an unknown host fails to connect. Git objects travel with
`git push` and `git fetch`, files as a `tar` stream.

## What a move copies

- The branch, commits, staged and unstaged changes, the transcript with
  anything pasted into the chat, and Claude's project memory.
- Files git doesn't track and doesn't ignore, unless their name is in
  `exclude`. Without an `exclude` of your own the names are
  `node_modules`, `.venv`, `dist`, `build` and `target`.
- Of the files git ignores, only those matching your `secrets` patterns,
  which are `.env` and `.env.*` unless you set others. magi doesn't print
  their contents. Logins, SSH keys and credential stores never move.
- A folder that isn't a git repository, whole, without the names in
  `exclude`.

Nothing that is running moves. The receiving machine refuses the move if
its checkout has work or commits of its own. A move that fails stops the
new copy and leaves the session running where it was. If you trust the
project's folder in Claude Code where the session leaves, magi marks its
copy as trusted in `~/.claude.json` where it arrives. The Stop hook runs
the handoff, with a limit of 200 seconds. The prompt hook refuses new
input in the copy a session left, and contacts nothing.

## When Claude asks you first

Before a move copies anything, magi lists what goes. That is the branch
and its changed files, untracked and secret files by name, the size, what
`exclude` leaves out, and whether the project's `.magi.toml` would run.
When the list holds a secret file, an untracked file, or 50 MB or more,
`magi arm` arms nothing and `move_session` moves nothing until Claude
calls it again with your yes.

The rule lives in magi, so Claude can't skip the first answer. The yes
itself passes through Claude, and nothing checks that you gave it, as
with every tool Claude calls for you.

## A move that was cut off

- Every move writes a journal, `~/.magi/moves/<session-id>.json`, before
  it changes anything and removes it as its last step.
- magi settles a leftover journal on its own, when a Claude Code session
  starts, once a minute from the watcher, on that session's next move and
  when you ask what magi did.
- The live copy is the one that took new prompts since the move started.
  If both did, it is the one still running. If neither did, it is the
  copy the session was leaving, unless the move had reached its last step.
- Recovery stops the other copy's Claude process, closes the tmux window
  the move opened, and renames that copy's transcript so its turns are
  kept. It never deletes a folder or a transcript.
- If both copies took new prompts and both or neither is running, magi
  stops nothing and leaves the choice to you.
- If the other machine doesn't answer, magi changes nothing there and
  tries again on each check.

## The watcher

`magi watch` lets a laptop that takes no incoming SSH take sessions back.
`setup_machine` installs it as a launchd agent on macOS or a systemd user
unit on Linux. It runs as you, never as root, and starts at login.

- It keeps one outgoing SSH connection to each machine in your config and
  runs a small loop there. The loop refreshes
  `~/.magi/watchers/<laptop name>`, which holds the laptop's `secrets` and
  `exclude` lists, and reports hand-back requests.
- A request is the file `~/.magi/wanted/<session-id>.json` that `magi arm`
  writes on the far machine. It can only make the laptop pull a session
  from the machine the request is on.
- Every 2 seconds the loop reads Claude Code's own record of each live
  session in `~/.claude/sessions`, on the laptop too. When that changes,
  and every 30 seconds, it works out where each session runs: it asks
  tmux for every pane and its tmux session, and every attached client
  with its tty, process and last activity, and lists every process's
  number and parent. On Linux it reads `REX_SESSION` and `REX_BLOCK` from
  each session process's `/proc/<pid>/environ`. On macOS it lists every
  process's tty, follows the parent chain of each session or its tmux
  client with `ps`, and runs `plutil` on the `Info.plist` of the app
  bundle at the top of that chain to read its bundle id. When a session
  is waiting, it runs `claude agents --json` once to learn on what.
- About every 30 seconds it reads load, memory, memory pressure, swap,
  the boot time, the disk your home folder is on, the battery's charge
  and whether the machine runs on it, and every process's number,
  parent, CPU time and memory. It also samples CPU use, the network
  interface of the default route and its byte counters, and the
  temperature. On macOS that's `sysctl`, `pmset -g batt`, `iostat`,
  `notifyutil -g com.apple.system.thermalpressurelevel`,
  `route -n get default` and `netstat -ibn`; on Linux, files in `/proc`
  (`/proc/stat`, `/proc/net/route` and `/proc/net/dev` among them),
  `/sys/class/power_supply`, and the CPU temperature sensor in
  `/sys/class/hwmon` or `/sys/class/thermal`.
- On the laptop, the watcher runs `tailscale status --json` for each
  other machine about every 30 seconds, when Tailscale is installed, to
  learn whether the connection goes through a Tailscale relay.
- When a loop starts, it lists every process with its command line once,
  to stop older copies of itself that lost their connection.
- The watcher writes what it hears to `~/.magi/live/<machine>.json` on the
  laptop, readable only by you. That is whether the machine answers, its
  SSH destination, load, memory, memory pressure, swap, disk, boot time,
  battery, whether it's reached through a relay, the last hour of CPU,
  temperature, thermal level and network traffic, and each session's id,
  title, folder, state, what it waits on, CPU and its last hour, memory,
  Remote Control link, Claude desktop session id (`hostSession`), tmux
  pane, Rex session and block ids, and on macOS the bundle id of the app
  it runs in (`app`) and its terminal device (`tty`). The file holds no
  conversation text beyond what a session waits on, and no file
  contents.
- Its log holds machine names, short session ids, folder paths, branch
  and file names, and errors.

## The menu bar app

`setup_machine` on a Mac installs the app, and the watcher keeps it at
magi's version when magi updates.

- It downloads `magi.app.zip` and `checksums.txt` from the same release
  as the binary, and installs nothing unless the zip's SHA-256 matches.
- The app is signed with magi's own certificate, the same for every
  release, so macOS keeps its permissions across updates. Its private
  key is in this repository's GitHub Actions secrets, for the release
  workflow, and in the owner's offline copy. The app isn't notarized:
  magi downloads it with `curl`, which marks nothing as quarantined, so
  Gatekeeper doesn't check it.
- magi installs nothing unless `magi.app` in the zip is a folder, not a
  link, its version is magi's, and `codesign` verifies it against the
  requirement pinned in magi, `remote/app.requirement`, which names the
  bundle id and the certificate's SHA-1. The pin covers the app only:
  the `magi` binary is checked against its checksum but isn't signed.
- It replaces `/Applications/magi.app` only when that is magi's app, by
  its bundle id. Writing to `/Applications` needs an administrator
  account, and magi refuses before it changes anything when this
  account can't.
- It swaps `/Applications/magi.app` for the new one in a single rename,
  then quits every running app with magi's bundle id and unregisters
  other copies of it from LaunchServices. `setup_machine` opens the
  app. The watcher reopens it only if it was running, and if an update
  fails it tries again once an hour.
- When the app is already installed at magi's version and passes the
  pinned requirement, `setup_machine` leaves it as it is and only opens
  it if it isn't running.

The macOS menu bar app reads `~/.magi/live` and shows what is in it. To
open a session where it runs, it opens apps and links through macOS and
runs `osascript`, `rex` and `tmux` itself, never `ssh` or `magi`.

- It runs your login shell with `command -v rex` or `command -v tmux` to
  find those tools on your `PATH`, once per launch for each, and again
  after a lookup that failed.
- It reads `~/.claude.json` for the one setting that says which terminal
  you last used (`deepLinkTerminal`); it reads nothing else there.
- When opening a session fails, it writes the route (the link, SSH
  destination, Rex ids or tty), the command, its exit status and its
  error output to the macOS system log, where they are not redacted.
- A notification shows the session's title, machine, folder and the
  question it waits on, and macOS may show it on the lock screen.
- `osascript` sends Apple Events to Terminal or Ghostty, which macOS
  asks you to allow once per app: it brings the window holding the
  session's terminal to the front, or opens a new Ghostty window.
- A session in tmux with no terminal attached opens in a new window
  running `tmux attach`, through `ssh -t` for another machine. In
  Terminal that window runs a temporary `.command` file that deletes
  itself first. Apart from its own settings, and creating the
  `~/.magi/live` folder when it is missing, that is the only file the
  app writes.

## Sending work

- `run_elsewhere`, `delegate` and `session_elsewhere` work in a fresh copy
  under `~/.magi/tasks` on the other machine and never write to your
  checkouts there. Claude proposes the machine and the work, and you
  approve both.
- A delegated Claude runs headless in `acceptEdits` mode by default, or
  `dontAsk` or `auto` when Claude asks for one, never with permission
  checks bypassed.
- A delegated Claude gets only the magi tools `machines`, `sessions`,
  `diff` and `task_status`. It can't send work on, move a session, set up
  a machine or approve a project.
- A collected delegate's changes are written into your working tree and
  kept as the branch `magi/task-<id>`. If they clash with edits made
  since, nothing is applied and they stay on the branch.

## Tested versions and the nightly

- Before a move, magi runs `claude --version` on both machines and refuses
  a version that is older than it supports, known to fail, or newer than
  the newest its tests passed with.
- The list is `remote/tested.json` on `main`. magi downloads it over HTTPS
  when its copy in `~/.magi` is more than an hour old.
- Someone who could change the list could block your moves or let an
  untested version through, and nothing else. magi runs nothing from it.
- A nightly GitHub Actions workflow runs the contract tests with the
  newest Claude Code on GitHub's runners and raises the newest tested
  version on `main` when they pass. [docs/support.md](docs/support.md)
  says what it tests.
- The nightly uses the repository secret `CLAUDE_CODE_OAUTH_TOKEN` to run
  Claude Code turns on the project's account. The test jobs can only read
  the repository. Only the job that commits the list can write to it.

## Project files and ports

- A project's `.magi.toml` runs nothing until you approve that exact file
  on that machine. Then its `setup`, `services` and `ports` run after
  moves and before `run_elsewhere` and `delegate` tasks.
- Claude calls `allow_project` with the file's SHA-256 as magi gave it.
  If the file changed since, it allows nothing.
- Then Claude Code shows you a dialog with the file's path, the machine
  and the exact commands and ports. Claude can't answer it, and magi
  records the approval only when you accept.
- The approval is the file's path and SHA-256 in `~/.magi/allowed`, so any
  edit needs a new one. A file with unknown keys, or control or format
  characters in a command, is refused.
- `ports` are forwarded from the machine a session left with `ssh -N -L`,
  bound to `127.0.0.1`, until the session comes back or moves on.

## Files magi writes

Settings are in `~/.config/magi/config.toml`, or under `XDG_CONFIG_HOME`.
State is in `~/.magi`.

| Path in `~/.magi` | What it holds |
|---|---|
| `bin/` | The downloaded magi binaries and the link `magi` to the current one |
| `armed/`, `wanted/` | A session set to move when its turn ends, and hand-back requests |
| `watchers/`, `watch.lock`, `app.lock` | One file per laptop watching this machine, the watcher's lock, and the menu bar app's install lock |
| `live/` | One file per machine, written by `magi watch` |
| `moved/`, `handoffs.jsonl` | Where each session went, and a log of handoffs and settled moves |
| `moves/` | One journal per move in progress or cut off |
| `forwards/`, `tabs/`, `setup/` | Forwarded ports, and a moved session's tmux window and setup state |
| `tasks/` | Task records and logs here, working tree copies on the other machine |
| `allowed`, `folders` | Approved `.magi.toml` hashes, and folders copied here that aren't git repositories |
| `ssh/`, `tested.json` | SSH control sockets, and the downloaded tested versions list |

Outside those, magi writes the project and session files it moves, the
trust entry in `~/.claude.json`, the watcher's service file and log, and
on a Mac `/Applications/magi.app`.


## Reporting a problem

Report security issues privately through GitHub's "Report a
vulnerability" on this repository, not in a public issue.
