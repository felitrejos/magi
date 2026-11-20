---
name: handoff
description: Move this running session to another machine so it continues there, for example "hand this off to my homelab", "transfer this session to the server", "continue this on the other machine", or back again with "hand this back to my Mac". Use whenever the user wants this conversation to keep going on a different computer.
allowed-tools: Bash(magi arm:*)
---

# Hand this session off

magi moves this session to another machine once your turn ends. It copies the project (for a git repository the branch, commits, staged, unstaged and untracked changes and secret files, for any other folder its files; untracked files and folders whose names magi's config excludes, such as node_modules, stay behind), the transcript and project memory, starts the session there in a tmux window with Remote Control, then closes this copy.

Handing back works the same way. If this session was handed off to this machine and the user says "hand this back to my Mac", follow the same steps with the Mac as the machine. The Mac doesn't have to be a configured host here. When it isn't, hand back works if the Mac was set up with this machine, because then it watches this one for sessions to take back. magi leaves a request here, the Mac pulls the session over its own SSH connection once your turn ends, and this session keeps running here until it does.

Nothing that is running moves, so do this in order.

1. If you have background shells, dev servers, monitors or background subagents running, stop them. Let a subagent that is about to finish complete instead. Write one or two sentences saying what each was doing and what is left, for example "dev server on port 3000 (pnpm dev); subagent was halfway through migrating the billing tests, a.ts and b.ts are done". Skip this if nothing is running.
2. Take the machine's name from what the user said ("my homelab" → `homelab`) and run this.

   ```bash
   magi arm <host> ${CLAUDE_SESSION_ID}
   ```

   If you wrote a note in step 1, pass it on stdin with a quoted heredoc, exactly like this, so nothing in it is expanded by the shell.

   ```bash
   magi arm <host> ${CLAUDE_SESSION_ID} - <<'NOTE'
   <note>
   NOTE
   ```

3. When `<host>` isn't a configured host, magi matches it against the machines that take sessions back from this one. It takes the exact name, or a part of a name that fits only one of them, so `mac` matches a machine called `macbook`. If the command says there is no such host, its message lists the configured hosts and, when there are any, the machines that take sessions back from this one. Run the command again with the name the user meant. If none fits, stop and tell the user that machine isn't set up to take sessions back. They can set it up from there by asking Claude to set up magi with this machine, or bring this session over by asking Claude there to move it.
4. If it fails for any other reason, tell the user why and stop.
5. If it succeeds, its first line starts with "Not armed", "Armed" or "Asked", and a block that starts with "What goes to" follows.
   - "Not armed" means the copy includes secret files, files git doesn't track, or 50 MB or more, so the user decides first. Nothing is armed, so ending your turn moves nothing. Show the user the block in a few lines, naming the untracked and secret files and the size it lists, ask whether to go ahead, and end your turn. If they say yes, run the same command again with `--yes` after the session id, `magi arm <host> ${CLAUDE_SESSION_ID} --yes`, passing the note again the same way if you wrote one, and go on as for "Armed". If they say no, restart anything you stopped in step 1.
   - "Armed" or "Asked" means the move starts when your turn ends. Pass the block on to the user in one or two lines, naming any untracked or secret files it lists. If it says the project's .magi.toml isn't allowed here, say its setup won't run on this move, and that a later session here can allow it for later moves. Don't call allow_project now. After "Asked", the other machine takes the session back. End your turn right away with one short line, for example "Handing off to homelab, it'll open there in a few seconds." Make no other tool calls, because the move starts when your turn ends.

If the handoff fails after your turn, you get a message saying what went wrong and this session keeps running here. Explain it to the user in plain words, and restart anything you stopped in step 1. When the other machine was asked to take the session back, a failure is written to the log of magi's watcher on that machine instead. It tries again each minute, and this session stays here meanwhile.
