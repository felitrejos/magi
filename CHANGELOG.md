# Changelog

## 0.2.0

- Repositories without any commits can be handed off.
- Folders that aren't git repositories are mirrored, deletions included, and a
  copy changed since a session left it is never overwritten. A session can
  come back to the folder it started in.
- Uncommitted Git LFS files arrive with their real content.
- Uncommitted changes inside git submodules move too, and a submodule the
  other machine doesn't have yet is set up there with its own remote.
- Secret and Git LFS files are found across the whole repository when Claude
  runs in a subfolder.
- `exclude` takes file and folder names; a pattern with a slash is refused.

## 0.1.0

First release.

- Hand a running Claude Code session to another machine by asking for it in
  plain words; it continues there in a new session with Remote Control on.
- Moves the conversation, commits and branch, staged and unstaged changes,
  untracked files, secret files, Claude's project memory and subagent
  transcripts. Folders that aren't git repositories are copied whole.
- Background work is stopped first and described in a note the new session
  uses to restart it.
- Opens the session in its own tab in [Rex](https://superlogical.com/rex),
  switching Rex to it, or in tmux.
- `cielago pull` brings a session back from a machine that can't reach yours.
- Parks the checkout a session leaves, so it can come back to it, and refuses
  to overwrite work made anywhere else.
- `cielago init`, `doctor`, `status` and `unpark`.
- Works between macOS and Linux machines in any direction.
