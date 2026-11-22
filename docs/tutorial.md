# Tutorial

This takes about five minutes. You start with Claude Code on one machine
and end with a session that moves to a second machine and back. The
examples call them your Mac and `homelab`. You never type a magi command.

You need two machines that run macOS or Linux, with the tools the
[README](../README.md#install) lists, and `ssh homelab` from your Mac has
to log in without a password prompt.

## 1. Install and set up

Run these in Claude Code on your Mac.

```text
/plugin marketplace add felitrejos/magi
/plugin install magi@magi
```

Then say "set up magi with my homelab". Claude tells you what it will
install and waits for your yes. Setup installs the plugin on `homelab`,
starts a background watcher on your Mac, installs the menu bar app there,
checks both machines, and moves a throwaway session there and back. That takes a minute or two and three
short Claude turns on your account. When everything passes, the result
ends with a line that starts "magi works with homelab".

If a check fails, Claude says what to fix.
[Troubleshooting](troubleshooting.md) covers each message.

## 2. Hand a session off

Open Claude Code in a project on your Mac, leave an edit uncommitted, and
say "hand this off to my homelab".

Claude stops anything it has running in the background, because nothing
that is running moves. It shows you what goes across and may ask first.

The move starts when Claude's turn ends. The result gives each step's
time and the session's Remote Control link. Open the link to continue in
the Claude app, or attach to tmux on `homelab`. The copy on your Mac
refuses new messages.

## 3. Hand it back

In the session on `homelab`, say "hand this back to my Mac". Your Mac
doesn't need to accept SSH. Its watcher sees the request and pulls the
session over its own connection when the turn ends. Your uncommitted edit
is back in the checkout you left.

## 4. Move another session

Ask "does anything need me?" and Claude lists the sessions on all your
machines. Then say "move the billing session to the homelab" or "bring
the billing session here". Claude says which session goes where and
waits for your yes. A session in the middle of a turn, or waiting on you,
isn't moved. A session you quit moves too, by its full id.

## 5. Approve a project's setup

A project can say what to run after it arrives. Put a `.magi.toml` in its
root.

```toml
setup = "pnpm install"
services = ["pnpm dev"]
ports = [3000]
```

Say "let this project's setup run". Claude Code shows a dialog with the
exact commands and ports, which Claude can't answer for you. Any edit to
the file needs a new yes.

After that, each move runs `setup` and each service in a tmux pane beside
the session. After a handoff, `localhost:3000` on the machine you left
reaches the session's server until the session comes back.

## 6. Use the menu bar app

Setup put magi's app in `/Applications` and opened it. It needs macOS 26
or later. The first time, a window checks that your machines answer and
offers to open magi at login. You can skip both steps. The app's icon in
the menu bar gets a badge when a session is waiting on you.

- **Click the icon.** The panel lists the sessions on all your machines,
  the waiting ones first. Click a session to open it where it runs. The
  first time that is a Ghostty or Terminal window, macOS asks once
  whether magi may control that app. Allow it, then click again if
  nothing came to the front.
- **Click a machine's tile** at the bottom for its CPU, memory,
  temperature and disk. Click it again to go back.
- **Click a card** for its list: the busiest processes for CPU and
  memory, the volumes for disk, the interfaces for network, and on
  Linux the sensors for temperature. A process that belongs to a session
  shows the session's title, and a click opens it. Escape steps back one
  level.
- **Notifications.** The first time a session starts waiting, macOS asks
  whether magi may send notifications. After that you get one per wait,
  and a click on it opens the session.
- **Settings.** Right-click the icon and choose Settings. General has
  Open at Login, which is off until you turn it on.

The app updates by itself when the plugin does.

## 7. Set limits and leave files behind

Setup wrote `~/.config/magi/config.toml` on the machine that sends the
work. Each machine is a table there, such as `[hosts.homelab]`, and
already has the lines setup wrote. Add the limits below them. `exclude`
goes at the top, before the first table.

```toml
exclude = ["node_modules", ".venv"]

[hosts.homelab]
max_agents = 2
min_free_memory = "4G"
quiet_hours = "22:00-07:00"
```

- **`max_agents`.** magi won't start a delegate or a new session there
  while that many agents already work there. Claude sessions in a turn
  count, and so do magi's delegates and sessions. Shell commands sent
  with "run the tests on my homelab" don't.
- **`min_free_memory`.** magi starts no new work there when less memory
  than that is free. Units are K, M, G and T, like `"512M"`.
- **`quiet_hours`.** magi starts no new work there in that window, by
  the clock of the machine sending the work. It may cross midnight.
- **`exclude`.** File and folder names, without slashes, that stay
  behind when magi copies a project to another machine. A name matches
  at any depth. In a git repo this applies to untracked files. In a
  plain folder it applies to everything by that name. Without
  `exclude`, magi leaves behind `node_modules`, `.venv`, `dist`, `build`
  and `target`. Setting it replaces that list.

A limit you leave out doesn't apply. magi also holds off a machine that
is 90% busy, with no setting. When a limit stops new work, Claude says
the machine has no room and why, so you can pick another machine or run
it here. Moving a session isn't limited.

If the file has a mistake, magi names the key and what it expected.

"Run the tests on my homelab" sends work there and brings the result
back.
