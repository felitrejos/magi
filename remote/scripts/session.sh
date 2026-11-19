#!/bin/sh
command=$1
shift

agents() {
  case $1 in \~/*) set -- "$HOME/${1#\~/}" ;; esac
  "$1" agents --json
}

same() {
  [ -z "$2" ] || [ "$(started "$1")" = "$(printf '%s' "$2" | tr -s ' ')" ]
}

case $command in
  running)
    id=$1
    if pgrep -f -- "[-]-resume $id" >/dev/null; then exit 10; fi
    listed=$(agents "$2") || exit
    case $listed in *"\"sessionId\": \"$id\""*) exit 10 ;; esac
    ;;
  moved)
    dir=$HOME/.magi/moved
    case $1 in
      clear) rm -f "$dir/$2" ;;
      mark) mkdir -p "$dir" && printf '%s\n' "$3" > "$dir/$2" ;;
    esac
    ;;
  live)
    agents "$1" || exit
    for f in "$HOME"/.claude/sessions/*.json; do
      if [ -f "$f" ]; then printf '\0'; cat "$f"; fi
    done
    ;;
  diff)
    cd -- "$1" || exit
    git diff HEAD -- 2>/dev/null || git diff --
    git ls-files --others --exclude-standard | sed 's/^/new file: /'
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
    grep -F '"cwd":"' "$1" | tail -n 20
    ;;
  stop)
    pid=$1 id=$2 since=$3
    args() {
      if [ -r "/proc/$pid/cmdline" ]; then tr '\0' ' ' < "/proc/$pid/cmdline"; else ps -o args= -p "$pid"; fi
    }
    if ! kill -0 "$pid" 2>/dev/null || ! args | grep -q claude || ! same "$pid" "$since"; then exit 0; fi
    listed=$(agents "$4") || exit
    status=$(printf '%s\n' "$listed" | awk -v pid="$pid," '$1 == "\"pid\":" { mine = $2 == pid } mine && $1 == "\"status\":" { print $2; exit }')
    case $status in '"busy"'* | '"waiting"'*) exit 10 ;; esac
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
  kill)
    pkill -TERM -f -- "[-]-resume $1"
    true
    ;;
esac
