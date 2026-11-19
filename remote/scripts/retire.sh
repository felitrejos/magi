#!/bin/sh
pid=$1 transcript=$2 host=$3 session_id=$4 tmux_pane=$5 journal=$6
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
  { read -r _; read -r tab_window; read -r tab_tag; } < "$record"
  rm -f "$record"
  tmux if-shell -F -t "$tab_window" "#{==:#{@magi_tab},$tab_tag}" "kill-window -t $tab_window"
elif [ -n "$tmux_pane" ]; then
  tmux kill-pane -t "$tmux_pane"
fi
rm -f -- "$journal"
