#!/bin/sh
name=$1 cli=$2 codex=$3
case $cli in \~/*) cli=$HOME/${cli#\~/} ;; esac
wanted=$HOME/.magi/wanted
locks=${CODEX_HOME:-$HOME/.codex}/thread-writer-locks
if [ -n "$name" ]; then
  mkdir -p "$HOME/.magi/watchers" "$wanted"
  ps -A -o pid= -o ppid= -o args= | loop="magi set -- \\047$name\\047" awk '$2 == 1 && index($0, ENVIRON["loop"]) && index($0, ".magi/watchers") { print $1 }' | while read -r pid; do kill "$pid"; done
fi
tick=0 registry=- shown=- scanned=-
usage() {
  if [ -r /proc/self/stat ]; then
    cat /proc/[0-9]*/stat 2>/dev/null | sed 's/^\([0-9]*\) (.*) /\1 /' | awk -v hz="$(getconf CLK_TCK)" -v page="$(getconf PAGESIZE)" '{ print $1, $3, ($13 + $14 + $15 + $16) / hz, $23 * page / 1024 }'
  else
    ps -A -o pid= -o ppid= -o time= -o rss=
  fi
}
scan() {
  now=
  for lock in "$locks"/*.lock; do now="$now $lock"; done
  for pid in $held; do kill -0 "$pid" 2>/dev/null && now="$now $pid"; done
}
while :; do
  if [ -n "$name" ]; then
    : > "$HOME/.magi/watchers/$name"
    for f in "$wanted"/*.json; do
      [ -f "$f" ] || continue
      grep -qF "\"to\":\"$name\"" "$f" || continue
      pid=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$f")
      if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        id=${f##*/}
        printf '%s\n' "${id%.json}"
      else
        rm -f "$f"
      fi
    done
  fi
  live='' pids=''
  for f in "$HOME"/.claude/sessions/*.json; do
    pid=${f##*/}
    pid=${pid%.json}
    kill -0 "$pid" 2>/dev/null || continue
    entry=
    IFS= read -r entry < "$f"
    live="${live}claude $entry
"
    pids="$pids $pid"
  done
  if [ "$live" != "$registry" ]; then
    registry=$live
    case $live in *'"status":"waiting"'*) live="${live}waiting $("$cli" agents --json | tr -d '\n')
" ;; esac
    claude=$live
  fi
  if [ -n "$codex" ]; then
    scan
    if [ "$now" != "$scanned" ]; then
      opened=$(sh -c "$codex" magi opened)
      held=$(printf '%s\n' "$opened" | cut -d ' ' -f 2 | tr , ' ')
      scan
      scanned=$now grown=-
      set --
      while read -r _ _ rollout; do
        if [ -n "$rollout" ]; then set -- "$@" "$rollout"; fi
      done <<EOF
$opened
EOF
    fi
    if [ $# -eq 0 ]; then
      threads=
    elif [ "$grown" = - ] || [ $((tick % 3)) -eq 0 ]; then
      sizes=$(wc -c "$@")
      if [ "$sizes" != "$grown" ]; then
        grown=$sizes
        threads=$(printf '%s\n' "$opened" | sh -c "$codex" magi states)
      fi
    fi
  fi
  if [ "$claude$threads" != "$shown" ]; then
    shown=$claude$threads
    printf '%s' "$claude"
    if [ -n "$threads" ]; then printf '%s\n' "$threads"; fi
    echo sessions
  fi
  if [ $((tick % 15)) -eq 0 ]; then
    printf 'stats %s\n' "$({ load; df -Pk "$HOME"; usage; } | awk -v roots="$pids $held" '
      function seconds(t, n, p, i, s) {
        n = split(t, p, /[-:]/)
        for (i = 1; i <= n; i++) s = s * (i == 2 && t ~ /-/ ? 24 : 60) + p[i]
        return s
      }
      NR == 1 { cores = $1 }
      NR == 2 { load = $1 }
      NR == 3 { total = $1; used = $1 - $2 }
      NR == 5 { disk = sprintf("%.0f %.0f", $3 / 1024, $2 / 1024) }
      NR > 5 { parent[$1] = $2; cpu[$1] = seconds($3); rss[$1] = $4 }
      END {
        n = split(roots, r, " ")
        for (i = 1; i <= n; i++) root[r[i]] = 1
        for (p in parent) {
          q = p
          while (q > 0 && !(q in root)) q = (q in parent) ? parent[q] : 0
          if (q in root) { c[q] += cpu[p]; m[q] += rss[p] }
        }
        printf "%s %s %.0f %.0f %s", cores, load, used / 1048576, total / 1048576, disk
        for (i = 1; i <= n; i++) printf " %s:%.2f:%.0f", r[i], c[r[i]], m[r[i]] / 1024
        print ""
      }')"
  fi
  tick=$((tick + 1))
  sleep 2
done
