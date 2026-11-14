---
name: handoff
description: Move this running session to another machine so it continues there with Remote Control, for example "hand this off to my homelab", "transfer this session to the server", "continue this on homelab". Use whenever the user wants this conversation to keep going on a different computer.
allowed-tools: Bash(cielago arm:*)
---

# Hand this session off

cielago moves this session to another machine once your turn ends: it syncs the git branch, uncommitted changes and secret files, copies the transcript, starts the session there with Remote Control, then closes this copy.

1. Take the host name from what the user said ("my homelab" → `homelab`) and run:

   ```bash
   cielago arm <host> ${CLAUDE_SESSION_ID}
   ```

2. If it says there is no such host, it lists the configured ones: run it again with the one the user meant, or ask if none clearly matches.
3. If it fails for any other reason, tell the user why and stop.
4. If it succeeds, end your turn right away with one short line, for example "Handing off to homelab, open it from the Claude app in a few seconds." Make no other tool calls: the handoff starts when your turn ends.

If the handoff fails after your turn, you get a message saying what went wrong and this session keeps running here. Explain it to the user in plain words.
