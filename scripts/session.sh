#!/bin/sh
command=$1
shift

started() {
  if [ -r "/proc/$1/stat" ]; then
    sed 's/.*) //' "/proc/$1/stat" | cut -d ' ' -f 20
  else
    TZ=UTC ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ' | sed 's/^ //;s/ $//'
  fi
}

same() {
  [ -z "$2" ] || [ "$(started "$1")" = "$(printf '%s' "$2" | tr -s ' ')" ]
}

case $command in
  sessions)
    for f in "$HOME"/.claude/sessions/*.json; do
      if [ -f "$f" ]; then cat "$f"; printf '\0'; fi
    done
    ;;
  transcript)
    if [ ! -f "$1" ]; then
      echo "the session's transcript $1 isn't there" >&2
      exit 1
    fi
    sidecar=${1%.jsonl}
    if [ -d "$sidecar" ]; then echo sidecar; fi
    if [ -d "$sidecar/subagents" ]; then echo subagents; fi
    ;;
  find)
    newest=
    for f in "$HOME"/.claude/projects/*/"$1.jsonl"; do
      if [ -f "$f" ] && { [ -z "$newest" ] || [ -n "$(find "$f" -prune -newer "$newest")" ]; }; then newest=$f; fi
    done
    printf '%s\n' "$newest"
    ;;
  cwd)
    tail -n 400 "$1" | grep -oE '"cwd":"([^"\\]|\\.)*"' | tail -n 1
    ;;
  stop)
    pid=$1 id=$2 since=$3
    args() {
      if [ -r "/proc/$pid/cmdline" ]; then tr '\0' ' ' < "/proc/$pid/cmdline"; else ps -o args= -p "$pid"; fi
    }
    if ! kill -0 "$pid" 2>/dev/null || ! args | grep -q claude || ! same "$pid" "$since"; then exit 0; fi
    if grep -q '"status": *"busy"' "$HOME/.claude/sessions/$pid.json" 2>/dev/null; then exit 10; fi
    kill -TERM "$pid"
    i=0
    while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
    if kill -0 "$pid" 2>/dev/null; then
      echo "the session on this machine didn't stop" >&2
      exit 1
    fi
    if pgrep -f -- "[-]-resume $id" >/dev/null; then
      echo "another process is still running this session" >&2
      exit 1
    fi
    ;;
  alive)
    pid=$1 since=$2
    if [ -r "/proc/$pid/cmdline" ]; then args=$(tr '\0' ' ' < "/proc/$pid/cmdline"); else args=$(ps -o args= -p "$pid" 2>/dev/null); fi
    case $args in
      *claude*) same "$pid" "$since" && exit 0 ;;
    esac
    exit 1
    ;;
  kill)
    pkill -TERM -f -- "[-]-resume $1"
    true
    ;;
esac
