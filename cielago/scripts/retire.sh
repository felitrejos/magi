#!/bin/sh
pid=$1 desktop=$2 transcript=$3 host=$4 rex_session=$5 rex_block=$6 session_id=$7
if [ -n "$pid" ]; then
  if [ "$desktop" != 1 ]; then
    sleep 1
    kill -TERM "$pid" 2>/dev/null
    i=0
    while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.2; i=$((i + 1)); done
    if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; fi
  fi
  while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
fi
mv -- "$transcript" "$transcript.handed-off-to-$host" || exit
record=$HOME/.cielago/tabs/$session_id
if [ -f "$record" ]; then
  { read -r launcher; read -r tab_session; read -r tab_window; } < "$record"
  rm -f "$record"
  if [ "$launcher" = tmux ]; then tmux kill-window -t "$tab_window"; else rex window close -s "$tab_session" "$tab_window"; fi
elif [ "$desktop" != 1 ] && [ -n "$rex_session" ] && [ -n "$rex_block" ]; then
  rex block close -s "$rex_session" "$rex_block"
fi
