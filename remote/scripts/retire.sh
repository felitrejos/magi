#!/bin/sh
pid=$1 transcript=$2 host=$3 rex_session=$4 rex_block=$5 session_id=$6 tmux_pane=$7
if [ -n "$pid" ]; then
  sleep 1
  kill -TERM "$pid" 2>/dev/null
  i=0
  while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.2; i=$((i + 1)); done
  if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid"; fi
fi
mv -- "$transcript" "$transcript.handed-off-to-$host" || exit
record=$HOME/.magi/tabs/$session_id
if [ -f "$record" ]; then
  { read -r launcher; read -r tab_session; read -r tab_window; read -r tab_tag; } < "$record"
  rm -f "$record"
  if [ "$launcher" = tmux ]; then
    tmux if-shell -F -t "$tab_window" "#{==:#{@magi_tab},$tab_tag}" "kill-window -t $tab_window"
  else
    rex window close -s "$tab_session" "$tab_window"
  fi
elif [ -n "$rex_session" ] && [ -n "$rex_block" ]; then
  rex block close -s "$rex_session" "$rex_block"
elif [ -n "$tmux_pane" ]; then
  tmux kill-pane -t "$tmux_pane"
fi
