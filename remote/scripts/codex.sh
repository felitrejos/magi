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


started_at() {
  at=$(started "$1")
  if [ -r "/proc/$1/stat" ]; then
    echo $(($(awk '/^btime/ {print $2}' /proc/stat) + at / 100))
  else
    LC_ALL=C TZ=UTC date -j -f '%a %b %d %T %Y' "$at" +%s
  fi
}

codexes() {
  processes | program_is codex | grep -v ' app-server\( \|$\)'
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
  running)
    id=$1 rollout=$2 dir=$3
    [ -f "$rollout" ] || exit 11
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
    [ -f "$1" ] || exit 11
    turn "$1"
    ;;
  opened)
    for lock in "$codex_home"/thread-writer-locks/*.lock; do
      pids=$(holders "$lock" | tr '\n' ,)
      if [ -n "$pids" ]; then
        id=${lock##*/}
        id=${id%.lock}
        printf '%s %s %s\n' "$id" "${pids%,}" "$(find "$sessions" -name "rollout-*-$id.jsonl" | head -n 1)"
      fi
    done
    ;;
  states)
    while read -r id pids rollout; do
      state=idle
      if busy "$rollout"; then state=busy; fi
      echo "codex $id $state $pids"
    done
    ;;
  app)
    case $1 in \~/*) set -- "$HOME/${1#\~/}" ;; esac
    exec "$1" app-server
    ;;
  open)
    lock=$codex_home/thread-writer-locks/$1.lock
    if [ -f "$lock" ]; then
      for pid in $(holders "$lock"); do printf '%s %s\n' "$pid" "$(args_of "$pid")"; done
    fi
    ;;
  started)
    for pid; do
      at=$(started_at "$pid")
      if [ -z "$at" ]; then
        echo "couldn't read when process $pid started" >&2
        exit 1
      fi
      printf '%s %s\n' "$pid" "$at"
    done
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
    mkdir -p "$(dirname "$sessions/$1")" && cat > "$sessions/$1.magi-$$" && mv "$sessions/$1.magi-$$" "$sessions/$1" && printf '%s\n' "$sessions/$1"
    ;;
  size)
    wc -c < "$1"
    ;;
  path)
    printf '%s\n' "$sessions/$1"
    ;;
esac
