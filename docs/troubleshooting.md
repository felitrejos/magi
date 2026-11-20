# Troubleshooting

Each section starts with a message magi gives, as it appears in the code,
then says what it means and what to ask Claude or do. Names such as
`homelab` and paths stand for your own.

A failed move leaves the session running where it was. When you aren't
sure what happened, ask "what did magi do?". Claude calls `status`, which
settles any move that was cut off and lists what is left for you.

## Claude Code version refused

```text
homelab has Claude Code 2.1.340, which hasn't passed magi's nightly tests yet (tested up to 2.1.329); a new version usually passes within a day, so try again later
```

magi only moves a session when both machines run a Claude Code version
its contract tests passed with. A nightly job tests the newest release,
and magi picks up the new list within an hour without an update. Try
again the next day. If the nightly failed, an issue on the repository
names the version and the failed job.

```text
homelab has Claude Code 2.1.300, older than magi supports; update it to 2.1.319 or newer
```

Update Claude Code on that machine.

```text
homelab has Claude Code 2.1.325, which fails magi's tests, so magi doesn't move sessions with it; update it
```

That version is on the list of versions known to fail. Update Claude Code
on that machine.

## SSH asks for a password

```text
ssh homelab failed: ... Permission denied ...
magi needs ssh to log in there without a password prompt. Only the user can set that up, with an SSH key or Tailscale SSH. Then call setup_machine again.
```

magi runs `ssh` with `BatchMode=yes`, so it can never type a password.
Add your SSH key to that machine, for example with `ssh-copy-id homelab`,
or use Tailscale SSH. Check that `ssh homelab true` returns without a
prompt. Then say "set up magi with my homelab" again.

## Host key not accepted

```text
ssh homelab failed: ... Host key verification failed.
This machine doesn't know homelab's host key yet. Only the user can check and accept it, by running ssh homelab once in a terminal here. Then call setup_machine again.
```

magi never accepts a host key for you. Run `ssh homelab` once in a
terminal, check the fingerprint, and accept it. Then ask Claude to set
the machine up again.

## Machine off or unreachable

```text
ssh homelab failed: ...
Check that homelab is on and reachable from here, then call setup_machine again.
```

Check that the machine is on, awake and on your network or VPN.

## The new session waits at the trust dialog

```text
start: the session did not come up with Remote Control on homelab in time: the new session was waiting for you to trust /home/you/projects/app; accept that there
```

Claude Code asks you to trust a folder the first time it runs there. magi
carries your trust across when you trust the project's folder on the
machine the session leaves. If you never trusted it there, the copy
isn't trusted either. Open Claude Code in that folder on the receiving
machine once and accept the dialog, or trust the folder on the machine
you start from. Then ask for the move again.

You may also see this note on a move that worked.

```text
marking /home/you/projects/app trusted there failed, so Claude may ask you to trust it first
```

The session moved. Accept the dialog in its tmux window on that machine.

## A machine stopped answering

```text
sync: homelab stopped answering
```

The SSH connection to that machine died or ran out of time in the middle
of a move. The first word is the step it happened in. The session keeps
running where it was. In a real test with the connection frozen during
the copy, the handoff gave up after about 100 seconds, and magi settled
the move 40 seconds after the connection came back. The next handoff
worked.

Check the connection, then ask "what did magi do?". If the machine is
gone for good, `status` gives the path of the move's journal in
`~/.magi/moves`, and you remove that file yourself.

## The copy is too big for the link

The same message, `sync: homelab stopped answering`, appears when the
copy doesn't fit its time budget. A handoff has 80 seconds to read and
copy the project. A 300 MB untracked file over a 2.7 MB/s link didn't fit.

Before a move, magi shows the size of the changed and new files, and
Claude asks you first when it is 50 MB or more. If the size is large for
your link, do one of these before you say yes.

- Move the large file out of the project, or copy it across yourself.
- Add the file's or folder's name to `exclude` in
  `~/.config/magi/config.toml`. Untracked files with those names stay
  behind, and the list says they were left out.
- Delete build output you don't need.

Bringing a session to the machine you are on has a longer budget of three
minutes, so "bring that session here" from the other machine can fit a
copy a handoff can't.

## Not armed, Claude asks first

```text
Not armed: this copy needs the user's yes first. Show the user what goes, naming the untracked and secret files, and end your turn without arming. After their yes, run magi arm homelab <session id> --yes.
```

This isn't an error. The copy includes secret files, files git doesn't
track, or 50 MB or more, so magi armed nothing and Claude shows you the
list. Say yes and Claude arms the handoff. Say no and nothing moves.
`move_session` does the same and reports "Nothing moved" until you say
yes.

## The checkout on the other side has its own work

```text
/home/you/projects/app has uncommitted changes made on this machine; commit or stash them first
```

The checkout the session would land in has edits that didn't come from
the session. Commit or stash them there, then move again.

A checkout a session left is parked. When the session comes back, its
changes replace the uncommitted ones left there, unless you edited them
meanwhile. If you want to keep working in a parked checkout, say "unpark
that checkout". After that its changes count as yours, and a session
coming back is refused until you commit or stash them. "What did magi
do?" lists the parked checkouts.

```text
branch feature has commits this machine does not; pull them here first
```

The branch on the receiving machine has commits the session's copy lacks.
Pull or push so the two agree, then move again.

```text
branch feature is checked out in /home/you/projects/app-worktree
```

Another git worktree on the receiving machine has that branch checked
out. Switch that worktree to another branch.

```text
config.local exists here, is ignored, and would be overwritten; move it away first
```

A file the session adds already exists on the receiving machine as an
ignored file with different contents. Move it away there.

## WSL

```text
homelab runs in WSL (kernel 5.15.167.4-microsoft-standard-WSL2), which magi doesn't support. Set magi up between macOS and Linux machines instead.
```

`setup_machine` refuses WSL on either machine. Native Windows isn't
supported either. Use macOS and Linux machines.

## No Remote Control link

```text
start: the session did not come up with Remote Control on homelab in time
```

magi waits up to 45 seconds for the moved session to start with Remote
Control. On a handoff, magi then stops the new copy and the session
carries on where it was. On a move that brings a session to you, the
session has already moved, so the move succeeds and this appears as a
note. The session runs in its tmux window, without a link in the result.

Check these on the machine that receives the session.

- Claude Code is logged in with a claude.ai account. Setup fails with
  "Remote Control needs a claude.ai login" when it uses another kind of
  login. Run `claude auth login` there.
- The trust dialog isn't waiting. See the section above.
- The machine reaches the internet.

Then ask "does anything need me?". The `sessions` list gives each
session's Remote Control link when it has one.

## The session takes no more messages

```text
This session moved to homelab, so this copy takes no more messages. Continue it there, or start a new Claude session here and ask it to move session 3f9a2c1e here.
```

You typed into the copy a session left behind. Continue the session on
the other machine, or start a new Claude session here and ask it to bring
that session back.

## Hand back finds no machine

```text
no host "mac" in ..., and no machine called "mac" takes sessions back from this one. That machine isn't set up to take sessions back; the user can set it up from there by asking Claude to set up magi with this machine
```

Hand back works when the machine you name was set up with this one,
because its watcher then watches this machine. On that machine, say "set
up magi with" and this machine's name. Or start Claude there and ask it
to move the session to it.

If setup warned that `magi watch isn't running here`, the watcher's log
says why. On macOS it is `~/Library/Logs/magi-watch.log`. On Linux run
`journalctl --user -u magi-watch`.

## A session is busy

```text
session 3f9a2c1e is in the middle of a turn or waiting on the user, so it wasn't moved; ...
```

magi moves a session between turns. Let the turn finish, or answer the
session if it waits on you. Claude can wait for it and then move it.

## The download failed

```text
magi 0.10.0's download doesn't match its checksum, so it wasn't installed
```

The launcher installs nothing when the archive's SHA-256 differs from the
release's `checksums.txt`. Try again. If it repeats, report it.

```text
magi has no build for FreeBSD amd64
```

Releases hold builds for macOS and Linux on arm64 and amd64 only.

A `curl` error from the launcher means that machine couldn't reach
GitHub's releases, or that the plugin's version has no published release.

## A move was cut off

When SSH drops, a laptop sleeps or the hook is killed, magi works out
which copy is live and closes the other. It does that when a Claude Code
session starts, once a minute from the watcher, and when you ask "what
did magi do?". It never deletes a folder or a transcript.

```text
Moving session 3f9a2c1e to homelab was interrupted, and both copies have new turns since, so ...
```

Both copies were used after the cut. magi stops neither. Claude tells you
where each one is, and you choose which to keep.

## Still stuck

Ask "check magi on my homelab". Claude runs every check again and the round trip,
and marks each check that fails with what to fix. Open an issue at
[github.com/felitrejos/magi](https://github.com/felitrejos/magi/issues)
with the message you got.
