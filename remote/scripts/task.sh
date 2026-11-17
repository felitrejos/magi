#!/bin/sh
command=$1 dir=$2
shift 2
login=${SHELL:-/bin/sh}
shell=$login
case ${shell##*/} in
  fish | csh | tcsh | nu | elvish | xonsh) shell=/bin/sh ;;
esac

case ${dir#"$HOME"/.magi/tasks/} in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *)
    echo "task.sh: $dir isn't a task folder" >&2
    exit 2
    ;;
esac

render() {
  if command -v jq >/dev/null 2>&1; then
    # shellcheck disable=SC2016
    jq -R -r --unbuffered '. as $line | (try fromjson catch null) as $m
      | if ($m | type) != "object" then $line
        elif $m.type == "assistant" then
          ($m.message.content[]?
            | if .type == "text" then .text
              elif .type == "tool_use" then "→ \(.name) \((.input.command // .input.file_path // .input.pattern // .input.description // "") | tostring | .[0:160])"
              else empty end)
        elif $m.type == "result" then "\nmagi: Claude finished (\($m.subtype // "done"))"
        else empty end'
  else
    cat
  fi
}

working() {
  n=0
  for d in "$HOME"/.magi/tasks/[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]; do
    if [ ! -d "$d" ] || [ -f "$d/exit" ]; then continue; fi
    case $(cat "$d/kind" 2>/dev/null) in
      delegate | session) ;;
      *) continue ;;
    esac
    if [ -f "$d/pid" ]; then
      read -r p < "$d/pid"
      if ! kill -0 "$p" 2>/dev/null; then continue; fi
    elif [ -z "$(find "$d" -prune -mmin -10)" ]; then
      continue
    fi
    n=$((n + 1))
  done
  for f in "$HOME"/.claude/sessions/*.json; do
    if [ ! -f "$f" ] || ! grep -q '"status": *"busy"' "$f" || grep -qF '/.magi/tasks/' "$f"; then continue; fi
    p=${f##*/}
    if kill -0 "${p%.json}" 2>/dev/null; then n=$((n + 1)); fi
  done
  echo "$n"
}

available() {
  if [ -r /proc/meminfo ]; then
    awk '/^MemAvailable:/ {printf "%.0f\n", $2 * 1024}' /proc/meminfo
  elif command -v vm_stat >/dev/null 2>&1; then
    vm_stat | awk '/page size of/ {size = $8} /^Pages (free|inactive|speculative):/ {sub(/\.$/, "", $NF); pages += $NF} END {printf "%.0f\n", pages * size}'
  fi
}

started() {
  if [ -r "/proc/$1/stat" ]; then
    sed 's/.*) //' "/proc/$1/stat" | cut -d ' ' -f 20
  else
    ps -o lstart= -p "$1" 2>/dev/null | tr -d ' '
  fi
}

alive() {
  [ -f "$dir/pid" ] || return 1
  { read -r pid; read -r since; } < "$dir/pid"
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && [ "$(started "$pid")" = "$since" ]
}

kill_tree() {
  if command -v pgrep >/dev/null 2>&1; then
    for child in $(pgrep -P "$1"); do kill_tree "$child"; done
  fi
  kill -TERM "$1" 2>/dev/null
}

setup() {
  if [ -n "$1" ]; then
    printf 'magi: setting up: %s\n' "$1"
    if ! "$shell" -lc "$1" </dev/null; then
      printf 'magi: setup failed, so the task did not start\n'
      return 1
    fi
  fi
}

claude_status() {
  printf '\033]7501;state=%s:app=claude-code\033\\\033]2;Claude Code\007' "$1"
}

begin() {
  printf '%s\n%s\n' "$$" "$(started "$$")" > "$dir/pid"
  trap 'echo cancelled > "$dir/exit"; exit 1' HUP TERM INT
}

finish() {
  if [ -f "$dir/status" ]; then mv -f "$dir/status" "$dir/exit"; else echo lost > "$dir/exit"; fi
  printf '\nmagi: task finished (%s). This tab closes when the result is collected.\n' "$(cat "$dir/exit")"
  exec "$login" -l
}

case $command in
  run)
    setup=$1 run=$2
    begin
    {
      if setup "$setup"; then
        printf 'magi: running: %s\n' "$run"
        "$shell" -lc "$run" </dev/null
        echo "exit $?" > "$dir/status"
      else
        echo "setup failed" > "$dir/status"
      fi
    } 2>&1 | tee "$dir/log"
    finish
    ;;
  delegate)
    setup=$1
    shift
    begin
    claude_status working
    {
      if setup "$setup"; then
        "$@" < "$dir/prompt"
        echo "exit $?" > "$dir/status"
      else
        echo "setup failed" > "$dir/status"
      fi
    } 2>&1 | tee "$dir/log" | render | tee "$dir/view"
    if [ "$(cat "$dir/status" 2>/dev/null)" = "exit 0" ]; then claude_status "done"; else claude_status "error"; fi
    finish
    ;;
  session)
    begin
    "$@"
    echo "exit $?" > "$dir/status"
    finish
    ;;
  status)
    if [ -f "$dir/exit" ]; then
      printf 'done\n%s\n' "$(cat "$dir/exit")"
    elif alive; then
      printf 'running\n\n'
    elif [ -d "$dir" ]; then
      printf 'lost\n\n'
    else
      printf 'gone\n\n'
    fi
    for f in "$dir/view" "$dir/log"; do
      if [ -f "$f" ]; then
        tail -c "$1" "$f"
        break
      fi
    done
    ;;
  log)
    if [ -f "$dir/log" ]; then cat "$dir/log"; fi
    ;;
  cancel)
    if [ ! -f "$dir/exit" ] && alive; then
      kill -TERM -- "-$pid" 2>/dev/null || kill_tree "$pid"
    fi
    true
    ;;
  result)
    checkout=$1 head=$2 base=$3 message=$4
    cd -- "$checkout" || exit
    export GIT_AUTHOR_NAME=magi GIT_AUTHOR_EMAIL=magi@localhost GIT_COMMITTER_NAME=magi GIT_COMMITTER_EMAIL=magi@localhost
    t=$(mktemp -d)
    trap 'rm -rf "$t"' EXIT
    now=$(git rev-parse -q --verify HEAD || :)
    parent=$base
    if [ "$now" != "$head" ] && ! git merge-base --is-ancestor "$now" "$base" 2>/dev/null; then parent=$now; fi
    GIT_INDEX_FILE="$t/index" git read-tree "$parent" || exit
    GIT_INDEX_FILE="$t/index" git add -A >/dev/null || exit
    tree=$(GIT_INDEX_FILE="$t/index" git write-tree) || exit
    if [ "$tree" = "$(git rev-parse "$parent^{tree}")" ]; then
      if [ "$parent" = "$base" ]; then exit 0; fi
      result=$parent
    else
      result=$(git commit-tree "$tree" -p "$parent" -m "$message") || exit
    fi
    git update-ref refs/cielago/result "$result"
    printf '%s\n' "$result"
    ;;
  reserve | room)
    kind=$1 max=$2 min=$3
    lock=$HOME/.magi/tasks/.lock
    mkdir -p "$HOME/.magi/tasks"
    i=0
    until mkdir "$lock" 2>/dev/null; do
      holder=$(cat "$lock/pid" 2>/dev/null)
      i=$((i + 1))
      if { [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; } || { [ -z "$holder" ] && [ "$i" -gt 50 ]; }; then
        rm -rf "$lock"
        i=0
      fi
      sleep 0.1 2>/dev/null || sleep 1
    done
    echo "$$" > "$lock/pid"
    trap 'rm -rf "$lock"' EXIT
    if [ "$max" -gt 0 ] && { [ "$kind" = delegate ] || [ "$kind" = session ]; }; then
      n=$(working)
      if [ "$n" -ge "$max" ]; then
        echo "$n agents are already running there (Claude sessions in a turn, and magi's delegates and sessions), its budget is $max" >&2
        exit 11
      fi
    fi
    if [ "$min" -gt 0 ]; then
      free=$(available)
      if [ -n "$free" ] && [ "$free" -lt "$min" ]; then
        echo "$((free / 1048576)) MB of memory is free there, its budget needs $((min / 1048576)) MB" >&2
        exit 11
      fi
    fi
    if [ "$command" = reserve ]; then
      mkdir -m 700 -- "$dir" || exit
      printf '%s\n' "$kind" > "$dir/kind"
    fi
    ;;
  remove)
    slug=$1
    rm -rf -- "$dir" 2>/dev/null
    case $slug in
      */* | *..*) ;;
      *-magi-tasks-"${dir##*/}"-*) rm -rf -- "$HOME/.claude/projects/$slug" ;;
    esac
    if [ -e "$dir" ]; then
      echo "some of $dir couldn't be removed; files a container wrote there as root need sudo or the container to delete" >&2
      exit 1
    fi
    ;;
  *)
    echo "task.sh: unknown command $command" >&2
    exit 2
    ;;
esac
