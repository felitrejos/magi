#!/bin/sh
command=$1
shift

same() {
  [ -z "$2" ] || [ "$(started "$1")" = "$(printf '%s' "$2" | tr -s ' ')" ]
}

args_of() {
  if [ -r "/proc/$1/cmdline" ]; then
    tr '\0' ' ' < "/proc/$1/cmdline"
  else
    ps -o args= -p "$1"
  fi
}

processes() {
  if [ -d /proc/self ]; then
    for dir in /proc/[0-9]*; do
      args=$(args_of "${dir#/proc/}" 2>/dev/null)
      if [ -n "$args" ]; then printf '%s %s\n' "${dir#/proc/}" "$args"; fi
    done
  else
    ps -axo pid=,args=
  fi
}

resuming() {
  processes | id=" --resume $1" awk '{
    program = $2
    if (program ~ /(^|\/)(sh|bash|dash|zsh|node)$/) program = $3
    sub(/.*\//, "", program)
    if (program == "claude" && index($0, ENVIRON["id"])) print $1
  }'
}

ours() {
  kill -0 "$1" 2>/dev/null || return 1
  case $(args_of "$1") in *claude*) same "$1" "$2" ;; *) return 1 ;; esac
}

alive() {
  if [ -r "/proc/$1/stat" ]; then
    state=$(sed 's/.*) //' "/proc/$1/stat" 2>/dev/null)
  else
    state=$(ps -o stat= -p "$1" 2>/dev/null)
  fi
  case $state in '' | Z*) return 1 ;; esac
}

end() {
  kill -TERM "$1" 2>/dev/null
  i=0
  while alive "$1" && [ "$i" -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
  if alive "$1"; then
    echo "the session on this machine didn't stop" >&2
    return 1
  fi
}

case $command in
  running)
    id=$1
    if [ -n "$(resuming "$id")" ]; then exit 10; fi
    listed=$("$2" agents --json) || exit
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
    "$1" agents --json || exit
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
    ours "$pid" "$since" || exit 0
    listed=$("$4" agents --json) || exit
    status=$(printf '%s\n' "$listed" | awk -v pid="$pid," '$1 == "\"pid\":" { mine = $2 == pid } mine && $1 == "\"status\":" { print $2; exit }')
    case $status in '"busy"'* | '"waiting"'*) exit 10 ;; esac
    end "$pid" || exit
    if [ -n "$(resuming "$id")" ]; then
      echo "another process is still running this session" >&2
      exit 1
    fi
    ;;
  kill)
    for pid in $(resuming "$1"); do kill -TERM "$pid"; done
    ;;
  end)
    while [ $# -gt 1 ]; do
      if ours "$1" "$2"; then end "$1" || exit; fi
      shift 2
    done
    ;;
  turns)
    if [ -f "$1" ]; then
      grep -F '"type":"user"' "$1" | grep -cv -e '"tool_result"' -e '"isMeta":true' -e '"type":"progress"' -e '\[magi\] This session was just handed off' || true
    else
      echo 0
    fi
    ;;
esac
