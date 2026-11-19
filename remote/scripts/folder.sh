#!/bin/sh
set -e
export LC_ALL=C
command=$1 dir=$2
shift 2
registry=$HOME/.magi/folders

case $command in
  list)
    cd -- "$dir"
    find . "$@" ! -name . -print0
    ;;
  size)
    cd -- "$dir"
    find . "$@" -type f -exec ls -ln {} + | awk '{n++; b += $5} END {printf "%.0f %.0f\n", n, b}'
    ;;
  fingerprint)
    cd -- "$dir"
    s=$(command -p -v stat || command -v stat)
    t=$(mktemp)
    trap 'rm -f "$t"' EXIT
    if "$s" -c %s . >/dev/null 2>&1; then
      find . "$@" -type d ! -name . -exec "$s" -c '%F:%a:%n' {} + > "$t"
      find . "$@" ! -type d -exec "$s" -c '%F:%a:%s:%Y:%n' {} + >> "$t"
    else
      find . "$@" -type d ! -name . -exec "$s" -f '%HT:%Lp:%N' {} + > "$t"
      find . "$@" ! -type d -exec "$s" -f '%HT:%Lp:%z:%m:%N' {} + >> "$t"
    fi
    sort "$t" | cksum
    ;;
  state)
    if [ ! -e "$dir" ]; then echo new; exit 0; fi
    p=$(cd -- "$dir" && pwd -P)
    if [ -e "$p/.git" ]; then
      printf 'git\t%s\t%s\n' "$(git -C "$p" remote get-url origin 2>/dev/null || :)" "$(git -C "$p" rev-list --max-parents=0 HEAD 2>/dev/null | sort | head -n 1)"
      exit 0
    fi
    r=
    if [ -f "$registry" ]; then
      r=$(p=$p awk -F '\t' '$NF == ENVIRON["p"] {f = $1} END {print f}' "$registry")
    fi
    if [ -n "$r" ]; then printf 'ours\t%s\n' "$r"; else echo foreign; fi
    ;;
  register)
    p=$(cd -- "$dir" && pwd -P)
    mkdir -p "$(dirname "$registry")"
    touch "$registry"
    t=$(mktemp)
    p=$p awk -F '\t' '$NF != ENVIRON["p"]' "$registry" > "$t"
    printf '%s\t%s\n' "$1" "$p" >> "$t"
    mv "$t" "$registry"
    ;;
  remove)
    cd -- "$dir"
    xargs -0 rm -rf --
    ;;
  claim)
    [ ! -e "$dir" ] || exit 7
    mkdir -p "$(dirname "$dir")"
    mkdir "$dir"
    ;;
  *)
    echo "folder.sh: unknown command $command" >&2
    exit 2
    ;;
esac
