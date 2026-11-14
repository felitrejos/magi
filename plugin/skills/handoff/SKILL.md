---
name: handoff
description: Move this running session to another machine so it continues there, for example "hand this off to my homelab", "transfer this session to the server", "continue this on the other machine". Use whenever the user wants this conversation to keep going on a different computer.
allowed-tools: Bash(cielago arm:*)
---

# Hand this session off

cielago moves this session to another machine once your turn ends: it syncs the project (for git repositories the branch, commits, staged and unstaged changes and secret files; for other folders their files), the transcript and project memory, starts a new session there in Rex with Remote Control, then closes this copy.

Nothing that is running moves. So first:

1. If you have background shells, dev servers, monitors or background subagents running, stop them. Let a subagent that is about to finish complete instead. Write one or two sentences saying what each was doing and what is left, for example "dev server on :3000 (pnpm dev); subagent was halfway through migrating the billing tests, files done: a.ts, b.ts". Skip this if nothing is running.
2. Take the host name from what the user said ("my homelab" → `homelab`) and run, passing the note from step 1 as the last argument when there is one:

   ```bash
   cielago arm <host> ${CLAUDE_SESSION_ID} "<note>"
   ```

3. If it says there is no such host, it lists the configured ones: run it again with the one the user meant, or ask if none clearly matches.
4. If it fails for any other reason, tell the user why and stop.
5. If it succeeds, end your turn right away with one short line, for example "Handing off to homelab, it'll open in Rex in a few seconds." Make no other tool calls: the handoff starts when your turn ends.

If the handoff fails after your turn, you get a message saying what went wrong and this session keeps running here. Explain it to the user in plain words, and restart anything you stopped in step 1.
