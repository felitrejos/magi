#!/bin/sh
command=$1
shift
codex_home=${CODEX_HOME:-$HOME/.codex}
sessions=$codex_home/sessions

holders() {
  if [ -d /proc/self/fd ]; then
    for fd in /proc/[0-9]*/fd/*; do
      if [ "$(readlink "$fd" 2>/dev/null)" = "$1" ]; then pid=${fd#/proc/}; echo "${pid%%/*}"; fi
    done
  else
    lsof -t -- "$1" 2>/dev/null
  fi
}

cwd_of() {
  if [ -d /proc/self/fd ]; then
    readlink "/proc/$1/cwd"
  else
    lsof -a -p "$1" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p'
  fi
}

codexes() {
  ps -axo pid=,args= | awk '{
    program = $2
    if (program ~ /(^|\/)(sh|bash|dash|zsh|node)$/) program = $3
    sub(/.*\//, "", program)
    if (program == "codex" && $0 !~ / app-server( |$)/) print
  }'
}

turn() {
  grep -E '"type":"(task_started|task_complete|turn_aborted)"' "$1" | tail -n 1
}

busy() {
  line=$(turn "$1")
  case $line in
    *'"type":"task_started"'*) [ -z "$2" ] || case $line in *"\"timestamp\":\"$2\""*) false ;; esac ;;
    *) false ;;
  esac
}

case $command in
  find)
    find "$sessions" \( -name "rollout-*-$1.jsonl" -o -name "rollout-*-$1.jsonl.zst" \) 2>/dev/null | {
      newest=
      while read -r f; do
        if [ -z "$newest" ] || [ -n "$(find "$f" -prune -newer "$newest")" ]; then newest=$f; fi
      done
      printf '%s\n%s\n' "$sessions" "$newest"
    }
    ;;
  cwd)
    grep -E '^\{[^{]*"type":"(session_meta|turn_context)"' "$1" | tail -n 1
    ;;
  running)
    id=$1 rollout=$2 dir=$3
    held=$(holders "$rollout")
    codexes | while read -r pid args; do
      case " $args " in
        *" $id "*) echo "$pid session" ;;
        *[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-*) ;;
        *)
          if printf '%s\n' "$held" | grep -qx "$pid"; then
            echo "$pid session"
          elif [ "$(cwd_of "$pid")" = "$dir" ]; then
            echo "$pid folder"
          fi
          ;;
      esac
    done
    ;;
  turn)
    turn "$1"
    ;;
  open)
    lock=$codex_home/thread-writer-locks/$1.lock
    if [ -f "$lock" ]; then
      for pid in $(holders "$lock"); do ps -o pid=,args= -p "$pid"; done
    fi
    ;;
  started)
    for pid; do printf '%s %s\n' "$pid" "$(TZ=UTC ps -o lstart= -p "$pid" | tr -s ' ' | sed 's/^ //')"; done
    ;;
  stop)
    rollout=$1 stale=$2
    shift 2
    if busy "$rollout" "$stale"; then exit 10; fi
    kill -TERM "$@" 2>/dev/null
    i=0
    while [ "$i" -lt 50 ]; do
      alive=
      for pid; do if kill -0 "$pid" 2>/dev/null; then alive=$pid; fi; done
      if [ -z "$alive" ]; then exit 0; fi
      sleep 0.2
      i=$((i + 1))
    done
    echo "Codex (pid $alive) didn't stop" >&2
    exit 1
    ;;
  kill)
    codexes | while read -r pid args; do
      case " $args " in
        *" $1 "*) kill -TERM "$pid" ;;
      esac
    done
    ;;
  write)
    mkdir -p "$(dirname "$sessions/$1")" && cat > "$sessions/$1.cielago-$$" && mv "$sessions/$1.cielago-$$" "$sessions/$1" && printf '%s\n' "$sessions/$1"
    ;;
  size)
    wc -c < "$1"
    ;;
  config)
    f=$codex_home/config.toml
    if [ -f "$f" ]; then cksum < "$f"; cat "$f"; else echo; fi
    ;;
  trust)
    f=$codex_home/config.toml
    sum=
    if [ -f "$f" ]; then sum=$(cksum < "$f"); fi
    if [ "$sum" != "$1" ]; then
      echo "it changed while cielago was updating it" >&2
      exit 3
    fi
    mkdir -p "$codex_home" && cat >> "$f"
    ;;
esac
