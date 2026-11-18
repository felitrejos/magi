#!/bin/sh
set -e
command=$1
shift
format='#{session_id} #{window_id} #{pane_id} #{pane_pid}'
case $command in
  sessions)
    tmux list-sessions -F '#{session_id} #{session_name}' 2>/dev/null || :
    ;;
  new-session)
    name=$1 label=$2 dir=$3
    shift 3
    unset LC_ALL
    tmux new-session -d -P -F "$format" -s "$name" -n "$label" -c "$dir" "$@"
    ;;
  new-window)
    session=$1 label=$2 dir=$3 detached=$4
    shift 4
    if [ "$detached" = 1 ]; then set -- -d "$@"; fi
    tmux new-window -P -F "$format" -t "$session:" -n "$label" -c "$dir" "$@"
    ;;
  split)
    pane=$1 direction=$2 dir=$3
    shift 3
    if [ "$direction" = below ]; then set -- -v -l 35% "$@"; else set -- -h "$@"; fi
    tmux split-window -d -P -F '#{pane_id}' -t "$pane" -c "$dir" "$@"
    ;;
  select)
    tmux select-window -t "$1"
    tmux list-clients -F '#{client_tty}' 2>/dev/null | while read -r client; do
      tmux switch-client -c "$client" -t "$1" && echo "$client"
    done
    ;;
  kill-session)
    tmux kill-session -t "$1"
    ;;
  kill-window)
    window=$1 pid=$2
    if [ -n "$pid" ] && ! tmux list-panes -t "$window" -F '#{pane_pid}' 2>/dev/null | grep -qx "$pid"; then exit 0; fi
    tmux kill-window -t "$window"
    ;;
esac
