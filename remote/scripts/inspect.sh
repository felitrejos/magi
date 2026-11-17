#!/bin/sh
set -e
export LC_ALL=C
cwd=$1
outermost=$2

emit() { printf '%s\0%s\0' "$1" "$2"; }
list() { emit "$1" "$(tr -cd '\0' < "$2" | wc -c | tr -d ' ')"; cat "$2"; }

cd -- "$cwd"
emit cwd "$(pwd -P)"
if ! top=$(git rev-parse --show-toplevel 2>&1); then
  case $top in
    *"not a git repository"*) emit plain 1; exit 0 ;;
  esac
  echo "$top" >&2
  exit 1
fi
cd "$top"
if [ "$outermost" = 1 ]; then
  while super=$(git rev-parse --show-superproject-working-tree) && [ -n "$super" ]; do
    cd "$super"
  done
fi
emit root "$(pwd -P)"

d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT

common=$(git rev-parse --path-format=absolute --git-common-dir)
emit mainroot "$(cd "$(dirname "$common")" && pwd -P)"
emit origin "$(git remote get-url origin 2>/dev/null || :)"
emit branch "$(git symbolic-ref --short -q HEAD || :)"

head=
if git rev-parse -q --verify HEAD >/dev/null; then
  git rev-list --max-parents=0 HEAD | sort > "$d/roots"
  emit rootcommit "$(head -n 1 "$d/roots")"
  head=$(git rev-parse HEAD)
  emit head "$head"
fi

[ -z "$(git ls-files --unmerged)" ] || exit 8
for m in MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD rebase-merge rebase-apply; do
  [ ! -e "$(git rev-parse --path-format=absolute --git-path "$m")" ] || exit 9
done

itree=$(git write-tree)
index=$(git rev-parse --path-format=absolute --git-path index)
if [ -f "$index" ]; then cp "$index" "$d/index"; fi
GIT_INDEX_FILE="$d/index" git add -A
wtree=$(GIT_INDEX_FILE="$d/index" git write-tree)
export GIT_AUTHOR_NAME=magi GIT_AUTHOR_EMAIL=magi@localhost GIT_COMMITTER_NAME=magi GIT_COMMITTER_EMAIL=magi@localhost
if [ -n "$head" ]; then
  wt=$(git commit-tree "$wtree" -p "$head" -m 'magi: working tree')
  idx=$(git commit-tree "$itree" -p "$head" -m 'magi: staging area')
else
  wt=$(git commit-tree "$wtree" -m 'magi: working tree')
  idx=$(git commit-tree "$itree" -m 'magi: staging area')
fi
emit worktree "$wt"
emit index "$idx"

base=${head:-$(git hash-object -t tree /dev/null)}
git diff -z --name-only --no-renames --diff-filter=d "$base" "$wt" > "$d/changed"
git check-attr -z --stdin filter < "$d/changed" > "$d/attrs"
list attrs "$d/attrs"

: > "$d/objects"
if git grep -q -e 'filter=lfs' "$wt" -- ':(glob)**/.gitattributes' && git lfs version >/dev/null 2>&1; then
  store=$(git lfs env 2>/dev/null | sed -n 's/^LocalMediaDir=//p')
  if [ -n "$store" ] && [ -d "$store" ]; then
    emit lfsstore "$store"
    git lfs ls-files --long "$wt" > "$d/lfs"
    while read -r oid _; do
      object=$(printf '%s' "$oid" | cut -c1-2)/$(printf '%s' "$oid" | cut -c3-4)/$oid
      if [ -f "$store/$object" ]; then printf '%s\0' "$object"; fi
    done < "$d/lfs" > "$d/objects"
  fi
fi
list lfsobjects "$d/objects"

