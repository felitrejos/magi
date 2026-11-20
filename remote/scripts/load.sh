#!/bin/sh
load() {
  if [ "$(uname -s)" = Darwin ]; then
    sysctl -n hw.ncpu vm.loadavg hw.memsize hw.pagesize vm.page_free_count vm.page_pageable_external_count vm.page_purgeable_count vm.swapusage kern.boottime kern.memorystatus_vm_pressure_level | awk '
      NR == 1 { print }
      NR == 2 { print $2, $3, $4 }
      NR == 8 { swap = sprintf("%.0f %.0f", $6, $3) }
      NR == 9 { booted = $4 + 0 }
      { v[NR] = $1 }
      END {
        printf "%.0f %.0f\n", v[3], (v[5] + v[6] + v[7]) * v[4]
        print swap, booted, (v[10] == 4 ? "critical" : v[10] == 2 ? "warning" : "normal")
      }'
  else
    getconf _NPROCESSORS_ONLN
    cut -d ' ' -f 1-3 /proc/loadavg
    awk '/^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 } /^SwapTotal:/ { st = $2 } /^SwapFree:/ { sf = $2 } /^btime / { booted = $2 }
      END {
        print t * 1024, a * 1024
        printf "%.0f %.0f %s", (st - sf) / 1024, st / 1024, booted
        if ((getline psi < "/proc/pressure/memory") > 0) {
          split(psi, f, /[ =]/)
          printf " %s", (f[5] >= 30 ? "critical" : f[5] >= 10 ? "warning" : "normal")
        }
        print ""
      }' /proc/meminfo /proc/stat
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
  listed=$("$1" agents --json) || return
  printf '%s\n' "$listed" | awk -v n="$n" '$1 == "\"cwd\":" { tasked = index($0, "/.magi/tasks/") } $1 == "\"status\":" && $2 ~ /^"busy"/ && !tasked { n++ } END { print n + 0 }'
}
