---
name: handoff
description: Move this running session to another machine so it continues there, for example "hand this off to my homelab", "transfer this session to the server", "continue this on the other machine", or back again with "hand this back to my Mac". Use whenever the user wants this conversation to keep going on a different computer.
allowed-tools: Bash(magi arm:*)
---

# Hand this session off

magi moves this session to another machine once your turn ends. It syncs the project (for a git repository the branch, commits, staged and unstaged changes and secret files, for any other folder its files), the transcript and project memory, starts a new session there in a Rex or tmux tab with Remote Control, then closes this copy.

Handing back works the same way. If this session was handed off to this machine and the user says "hand this back to my Mac", follow the same steps with the Mac as the machine. This machine doesn't need to be set up to reach it. When it isn't, magi asks the overview to move the session, and the machine that can reach the other one does the move. That needs magi-overview installed on this machine with its agents running. Without it, tell the user to run `magi pull <this machine>` on the machine they want the session on.

Nothing that is running moves, so do this in order.

1. If you have background shells, dev servers, monitors or background subagents running, stop them. Let a subagent that is about to finish complete instead. Write one or two sentences saying what each was doing and what is left, for example "dev server on :3000 (pnpm dev); subagent was halfway through migrating the billing tests, files done: a.ts, b.ts". Skip this if nothing is running.
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

3. If it says there is no such host, it lists the configured ones. Run it again with the one the user meant, or ask if none clearly matches. If it says it doesn't know the machine but lists machines the overview sees, pick the one the user meant from that list (for example "my Mac" is the Mac's machine name there) and run it again. If it lists none, magi-overview isn't installed here or its agents aren't running, so tell the user to run `magi pull <this machine>` on the machine they want it on.
4. If it fails for any other reason, tell the user why and stop.
5. If it succeeds, end your turn right away with one short line, for example "Handing off to homelab, it'll open there in a few seconds." Make no other tool calls, because the handoff starts when your turn ends.

If the handoff fails after your turn, you get a message saying what went wrong and this session keeps running here. Explain it to the user in plain words, and restart anything you stopped in step 1.
