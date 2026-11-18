#!/bin/sh
set -e
command=$1
shift
format='#{session_id} #{window_id} #{pane_id}'
tag() {
  window=${ids#* }
  tmux set-option -w -t "${window%% *}" @magi_tab "$1"
  echo "$ids"
}
case $command in
  new-session)
    name=$1 label=$2 dir=$3 tab=$4
    shift 4
    ids=$(tmux new-session -d -P -F "$format" -s "$name" -n "$label" -c "$dir" "$@")
    tag "$tab"
    ;;
  new-window)
    session=$1 label=$2 dir=$3 tab=$4 detached=$5
    shift 5
    if [ "$detached" = 1 ]; then set -- -d "$@"; fi
    ids=$(tmux new-window -P -F "$format" -t "$session:" -n "$label" -c "$dir" "$@")
    tag "$tab"
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
    tmux if-shell -F -t "$1" "#{==:#{@magi_tab},$2}" "kill-window -t $1"
    ;;
esac
