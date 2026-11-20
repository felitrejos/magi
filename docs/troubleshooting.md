# Troubleshooting

Each entry gives a message from magi, its cause, and what to ask Claude or
do. `homelab` and the paths stand for your own. A failed move leaves the
session running where it was. When you aren't sure what happened, ask
"what did magi do?".

**`homelab has Claude Code 2.1.340, which hasn't passed magi's nightly tests yet`**

magi only moves a session between Claude Code versions its tests passed
with, and a nightly job tests the newest one. Try again the next day. If
the nightly failed, an issue on the repository names the version.

**`older than magi supports`** or **`which fails magi's tests`**

Update Claude Code on that machine.

**`ssh homelab failed: ... Permission denied`**

magi can't type a password. Add your SSH key to that machine or use
Tailscale SSH, check that `ssh homelab true` returns without a prompt,
then ask Claude to set the machine up again.

**`ssh homelab failed: ... Host key verification failed`**

magi never accepts a host key for you. Run `ssh homelab` once in a
terminal, check the fingerprint and accept it.

**`Check that homelab is on and reachable from here`**

Check that the machine is on, awake and on your network or VPN.

**`homelab runs in WSL ... which magi doesn't support`**

Use macOS and Linux machines.

**`the new session was waiting for you to trust /home/you/projects/app`**

You never trusted that folder on the machine the session left, so its
copy isn't trusted either. Open Claude Code in the folder on the receiving
machine once and accept the dialog, then move again. If a move that
worked says `marking ... trusted there failed`, accept the dialog in the
session's tmux window.

**`sync: homelab stopped answering`**

SSH to that machine died or ran out of time, in the step the first word
names. Check the connection, then ask "what did magi do?". If the machine
is gone for good, Claude gives you the path of the move's journal, and
you remove that file yourself.

The same message appears when a copy doesn't fit the 80 seconds a handoff
has. Move the large file out of the project, add its name to `exclude` in
`~/.config/magi/config.toml`, or delete build output. Bringing a session
to the machine you are on has three minutes, so "bring that session here"
can fit a copy a handoff can't.

**`Not armed: this copy needs the user's yes first`**

This isn't an error. The copy includes secret files, untracked files, or
50 MB or more, so Claude shows you the list. Say yes and the handoff
goes. Say no and nothing moves.

**`has uncommitted changes made on this machine; commit or stash them first`**

The checkout the session would land in has edits of its own. Commit or
stash them there. A checkout a session left is parked, and the returning
session replaces its uncommitted changes unless you edited them. To keep
working in one, say "unpark that checkout".

**`branch feature has commits this machine does not; pull them here first`**

Pull or push so the branch agrees on both machines. For
`branch feature is checked out in`, switch that git worktree on the
receiving machine to another branch. For `would be overwritten`, move
that ignored file away there.

**`the session did not come up with Remote Control on homelab in time`**

magi waits 45 seconds for the moved session to start. A handoff then
stops the new copy and the session carries on where it was. A session
brought to you has already moved, so it runs in its tmux window without
a link. On the receiving machine, check that Claude Code is logged in
with a claude.ai account (`claude auth login`), that no trust dialog is
waiting, and that it reaches the internet.

**`This session moved to homelab, so this copy takes no more messages`**

You typed into the copy a session left. Continue it on the other machine,
or start a new Claude session here and ask it to bring that session back.

**`no machine called "mac" takes sessions back from this one`**

Hand back works when the machine you name was set up with this one. On
that machine, say "set up magi with" and this machine's name. If setup
warned that `magi watch isn't running here`, read the watcher's log,
`~/Library/Logs/magi-watch.log` on macOS or
`journalctl --user -u magi-watch` on Linux.

**`is in the middle of a turn or waiting on the user, so it wasn't moved`**

magi moves a session between turns. Let the turn finish or answer the
session. Claude can wait for it and then move it.

**`download doesn't match its checksum, so it wasn't installed`**

The launcher installs nothing when the SHA-256 differs. Try again, and
report it if it repeats. `magi has no build for` means your system isn't
macOS or Linux on arm64 or amd64. A `curl` error means the machine
couldn't reach GitHub's releases, or the plugin's version has no release.

**`was interrupted, and both copies have new turns since`**

magi settles a move that was cut off by itself, and
[SECURITY.md](../SECURITY.md#a-move-that-was-cut-off) says how. Here both
copies were used after the cut, so magi stops neither. Claude tells you
where each one is, and you choose.

## Still stuck

Ask "check magi on my homelab". Claude runs every check and the round
trip again and marks what fails. Then open an issue at
[github.com/felitrejos/magi](https://github.com/felitrejos/magi/issues)
with the message you got.
