#!/bin/sh
name=$1
wanted=$HOME/.magi/wanted
mkdir -p "$HOME/.magi/watchers" "$wanted"
while :; do
  : > "$HOME/.magi/watchers/$name"
  for f in "$wanted"/*.json; do
    [ -f "$f" ] || continue
    grep -qF "\"to\":\"$name\"" "$f" || continue
    pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f")
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      id=${f##*/}
      printf '%s\n' "${id%.json}"
    else
      rm -f "$f"
    fi
  done
  sleep 1
done
