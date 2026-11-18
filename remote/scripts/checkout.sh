#!/bin/sh
set -e
mode=$1 repo=$2 H=$3 W=$4 I=$5 B=$6
parked=refs/worktree/magi/parked

cd -- "$repo"
export GIT_LFS_SKIP_SMUDGE=1

if [ "$mode" = copy ]; then
  if [ -n "$H" ] && [ -n "$B" ]; then
    git update-ref "refs/heads/$B" "$H"
    git symbolic-ref HEAD "refs/heads/$B"
  elif [ -n "$H" ]; then
    git update-ref --no-deref HEAD "$H"
  else
    git update-ref -d "refs/heads/$B"
    git symbolic-ref HEAD "refs/heads/$B"
  fi
  git read-tree --reset -u "$W"
  git read-tree "$I"
  for r in refs/magi/handoff refs/magi/worktree refs/magi/index; do
    git update-ref -d "$r"
  done
  exit 0
fi

[ "$mode" = apply ] || [ -e .git ] || exit 0

tree_now() {
  t=$(mktemp -d)
  index=$(git rev-parse --path-format=absolute --git-path index)
  if [ -f "$index" ]; then cp "$index" "$t/index"; fi
  GIT_INDEX_FILE="$t/index" git add -A >/dev/null
  GIT_INDEX_FILE="$t/index" git write-tree
  rm -rf "$t"
}

force=
if [ -n "$(git status --porcelain --untracked-files=all --ignore-submodules=none)" ]; then
  now=$(tree_now)
  if [ "$now" = "$(git rev-parse -q --verify "$parked^{tree}" || :)" ] || [ "$now" = "$(git rev-parse -q --verify "$W^{tree}" || :)" ]; then
    force=1
  else
    echo "$PWD has uncommitted changes made on this machine; commit or stash them first" >&2
    exit 3
  fi
fi
[ "$mode" = apply ] || exit 0

base=${H:-$(git hash-object -t tree /dev/null)}
git diff -z --name-only --diff-filter=A "$base" "$W" | tr '\0' '\n' | while IFS= read -r p; do
  if [ -e "$p" ] && git check-ignore -q -- "$p" && [ "$(git hash-object -- "$p")" != "$(git rev-parse "$W:$p")" ]; then
    printf '%s exists here, is ignored, and would be overwritten; move it away first\n' "$p" >&2
    exit 6
  fi
done

ahead() {
  printf 'branch %s has commits this machine does not; pull them here first\n' "$B" >&2
  exit 4
}

if [ -z "$H" ]; then
  if git rev-parse --verify --quiet "refs/heads/$B" >/dev/null; then ahead; fi
  git symbolic-ref HEAD "refs/heads/$B"
  if [ -n "$force" ]; then git clean -fdq; fi
  git read-tree --reset -u "$W"
else
  if [ -n "$B" ] && git rev-parse --verify --quiet "refs/heads/$B" >/dev/null; then
    rc=0
    git merge-base --is-ancestor "refs/heads/$B" "$H" || rc=$?
    if [ "$rc" -eq 1 ]; then ahead; fi
    [ "$rc" -eq 0 ]
  fi
  if [ -n "$B" ]; then
    elsewhere=$(git worktree list --porcelain | awk -v here="$(git rev-parse --show-toplevel)" -v want="branch refs/heads/$B" '/^worktree /{w=substr($0, 10)} $0 == want && w != here {print w}')
    if [ -n "$elsewhere" ]; then
      echo "branch $B is checked out in $elsewhere" >&2
      exit 7
    fi
  fi
  git checkout --quiet ${force:+--force} --detach "$H"
  if [ -n "$force" ]; then git clean -fdq; fi
  git read-tree -m -u HEAD "$W"
  if [ -n "$B" ]; then git checkout --quiet -B "$B"; fi
fi
git read-tree "$I"
for r in refs/magi/handoff refs/magi/worktree refs/magi/index "$parked"; do
  git update-ref -d "$r"
done
