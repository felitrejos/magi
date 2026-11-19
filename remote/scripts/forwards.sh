#!/bin/sh
[ -f "$HOME/.magi/forwards/$1.json" ] || exit 0
for c in "$HOME/.local/bin/magi" "$(command -v magi 2>/dev/null)"; do
  if [ -n "$c" ] && [ -x "$c" ]; then
    exec "$c" forwards close "$1"
  fi
done
echo "magi isn't installed there to stop it" >&2
exit 1
