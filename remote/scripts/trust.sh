#!/bin/sh
umask 077
f="$HOME/.claude.json"
t="$f.magi-$$"
cat > "$t" || exit 1
if [ "$(cksum < "$f")" != "$1" ]; then
  rm -f "$t"
  echo "it changed while magi was updating it" >&2
  exit 3
fi
mv "$t" "$f"
