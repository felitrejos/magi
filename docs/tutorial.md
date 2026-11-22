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
- **Notifications.** The first time a session starts waiting, macOS asks
  whether magi may send notifications. After that you get one per wait,
  and a click on it opens the session.
- **Settings.** Right-click the icon and choose Settings. General has
  Open at Login, which is off until you turn it on.

The app updates by itself when the plugin does.

"Run the tests on my homelab" sends work there and brings the result
back.
