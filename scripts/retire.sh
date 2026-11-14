#!/bin/sh
pid=$1 desktop=$2 transcript=$3 host=$4 rex_session=$5 rex_block=$6
if [ "$desktop" != 1 ]; then
  sleep 1
  kill -TERM "$pid" 2>/dev/null
  i=0
  while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.2; i=$((i + 1)); done
  if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; fi
fi
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
mv -- "$transcript" "$transcript.handed-off-to-$host" || exit
if [ "$desktop" != 1 ] && [ -n "$rex_session" ] && [ -n "$rex_block" ]; then
  rex block close -s "$rex_session" "$rex_block"
fi
