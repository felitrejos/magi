#!/bin/sh
command=$1 dir=$2
shift 2
login=${SHELL:-/bin/sh}
shell=$login
case ${shell##*/} in
  fish | csh | tcsh | nu | elvish | xonsh) shell=/bin/sh ;;
esac

case ${dir#"$HOME"/.cielago/tasks/} in
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
        elif $m.type == "result" then "\ncielago: Claude finished (\($m.subtype // "done"))"
        else empty end'
  else
    cat
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
    printf 'cielago: setting up: %s\n' "$1"
    if ! "$shell" -lc "$1" </dev/null; then
      printf 'cielago: setup failed, so the task did not start\n'
      return 1
    fi
  fi
}

begin() {
  printf '%s\n%s\n' "$$" "$(started "$$")" > "$dir/pid"
  trap 'echo cancelled > "$dir/exit"; exit 1' HUP TERM INT
}

finish() {
  if [ -f "$dir/status" ]; then mv -f "$dir/status" "$dir/exit"; else echo lost > "$dir/exit"; fi
  printf '\ncielago: task finished (%s). This tab closes when the result is collected.\n' "$(cat "$dir/exit")"
  exec "$login" -l
}

case $command in
  run)
    setup=$1 run=$2
    begin
    {
      if setup "$setup"; then
        printf 'cielago: running: %s\n' "$run"
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
    {
      if setup "$setup"; then
        "$@" < "$dir/prompt"
        echo "exit $?" > "$dir/status"
      else
        echo "setup failed" > "$dir/status"
      fi
    } 2>&1 | tee "$dir/log" | render | tee "$dir/view"
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
    export GIT_AUTHOR_NAME=cielago GIT_AUTHOR_EMAIL=cielago@localhost GIT_COMMITTER_NAME=cielago GIT_COMMITTER_EMAIL=cielago@localhost
    t=$(mktemp -d)
    trap 'rm -rf "$t"' EXIT
    now=$(git rev-parse -q --verify HEAD || :)
    parent=$base
    if [ "$now" != "$head" ]; then parent=$now; fi
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
  remove)
    rm -rf -- "$dir"
    ;;
  *)
    echo "task.sh: unknown command $command" >&2
    exit 2
    ;;
esac
