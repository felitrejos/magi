#!/bin/sh
name=$1 cli=$2 copies=$3
wanted=$HOME/.magi/wanted
if [ -n "$name" ]; then
  mkdir -p "$HOME/.magi/watchers" "$wanted"
  printf '%s\n' "$copies" > "$HOME/.magi/.watcher-$name" && mv -f "$HOME/.magi/.watcher-$name" "$HOME/.magi/watchers/$name"
  ps -A -o pid= -o ppid= -o args= | loop="set -- \\047$name\\047 " awk '
    index($0, ENVIRON["loop"]) && index($0, ".magi/watchers") { parent[$1] = $2 }
    END { for (p in parent) { top = p; while (parent[top] in parent) top = parent[top]; if (parent[top] == 1) print p } }' | while read -r pid; do kill "$pid"; done
fi
tick=0 registry=- os=$(uname -s)
battery() {
  if [ "$os" = Darwin ]; then
    pmset -g batt
  else
    grep -s . /dev/null /sys/class/power_supply/*/type /sys/class/power_supply/*/scope /sys/class/power_supply/*/capacity /sys/class/power_supply/*/status
  fi
}
usage() {
  if [ -r /proc/self/stat ]; then
    cat /proc/[0-9]*/stat 2>/dev/null | sed 's/^\([0-9]*\) (.*) /\1 /' | awk -v hz="$(getconf CLK_TCK)" -v page="$(getconf PAGESIZE)" '{ print $1, $3, ($13 + $14 + $15 + $16) / hz, $23 * page / 1024 }'
  else
    ps -A -o pid= -o ppid= -o time= -o rss=
  fi
}
sensor() {
  for want in 'coretemp Package id 0' 'k10temp Tdie' 'k10temp Tctl'; do
    for label in /sys/class/hwmon/hwmon*/temp*_label; do
      if [ -r "$label" ] && read -r chip < "${label%/*}/name" && read -r text < "$label" && [ "$chip $text" = "$want" ]; then
        echo "${label%_label}_input"
        return
      fi
    done
  done
  for want in x86_pkg_temp cpu-thermal; do
    for zone in /sys/class/thermal/thermal_zone*; do
      if [ -r "$zone/type" ] && read -r kind < "$zone/type" && [ "$kind" = "$want" ]; then
        echo "$zone/temp"
        return
      fi
    done
  done
}
sensor=$(sensor) critical=
if [ -r "${sensor%_input}_crit" ]; then read -r critical < "${sensor%_input}_crit"; fi
sample() {
  if [ "$os" = Darwin ]; then
    { iostat -n 0 -C -c 2 -w 1; notifyutil -g com.apple.system.thermalpressurelevel; route -n get default 2>/dev/null; netstat -ibn; } | awk '
      NR == 4 { printf " cpu=%d", $1 + $2 }
      $1 == "com.apple.system.thermalpressurelevel" { printf " thermal=%d", $2 }
      $1 == "interface:" { iface = $2 }
      $1 == iface && $3 ~ /^<Link/ { printf " net=%s:%s:%s", $(NF - 4), $(NF - 1), iface }'
    return
  fi
  read -r _ user nice system idle iowait irq softirq steal _ < /proc/stat
  printf ' ticks=%s:%s' $((user + nice + system + irq + softirq + steal)) $((user + nice + system + idle + iowait + irq + softirq + steal))
  if [ -n "$sensor" ]; then
    read -r milli < "$sensor"
    printf ' temp=%d.%d' $((milli / 1000)) $((milli % 1000 / 100))
    if [ -n "$critical" ]; then printf ':%d' $((critical / 1000)); fi
  fi
  iface=
  while read -r name destination _; do
    case $name in lo | veth* | docker* | br-* | tailscale*) continue ;; esac
    if [ -z "$iface" ] && [ "$destination" = 00000000 ]; then iface=$name; fi
  done < /proc/net/route
  while IFS=': ' read -r name received _ _ _ _ _ _ _ sent _; do
    if [ "$name" = "$iface" ]; then printf ' net=%s:%s:%s' "$received" "$sent" "$iface"; fi
  done < /proc/net/dev
}
while :; do
  if [ -n "$name" ]; then
    touch "$HOME/.magi/watchers/$name"
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
    tabs=$({ tmux list-panes -a -F 'pane #{pane_pid} #{window_id}' 2>/dev/null; ps -A -o pid= -o ppid= | sed 's/^/process /'; } | awk -v pids="$pids" '
      $1 == "pane" { pane[$2] = $3 }
      $1 == "process" { parent[$2] = $3 }
      END {
        n = split(pids, p, " ")
        for (i = 1; i <= n; i++) {
          q = p[i]
          while (q > 1 && !(q in pane)) q = parent[q] + 0
          if (q in pane) print "tab", p[i], pane[q]
        }
      }')
    if [ -n "$tabs" ]; then live="$live$tabs
"; fi
    printf '%s' "$live"
    echo sessions
  fi
  if [ $((tick % 15)) -eq 0 ]; then
    printf 'stats %s%s\n' "$({ load; df -Pk "$HOME"; usage; battery; } | awk -v roots="$pids" '
      function seconds(t, n, p, i, s) {
        n = split(t, p, /[-:]/)
        for (i = 1; i <= n; i++) s = s * (i == 2 && t ~ /-/ ? 24 : 60) + p[i]
        return s
      }
      /^Now drawing from / { discharging = /Battery Power/; next }
      /^ -InternalBattery/ { split($0, b, /\t|; /); battery = b[2] + 0; next }
      /^\/sys\/class\/power_supply\// { split($0, f, /[\/:]/); supply[f[5], f[6]] = f[7]; if (f[6] == "type") type[f[5]] = f[7]; next }
      NR == 1 { cores = $1 }
      NR == 2 { load = $1 }
      NR == 3 { total = $1; used = $1 - $2 }
      NR == 4 { swap = $1 " " $2; booted = $3; pressure = $4 }
      NR == 6 { disk = sprintf("%.0f %.0f", $3 / 1024, $2 / 1024) }
      NR > 6 { parent[$1] = $2; cpu[$1] = seconds($3); rss[$1] = $4 }
      END {
        for (s in type) {
          if (type[s] == "Battery" && supply[s, "scope"] != "Device") { battery = supply[s, "capacity"] + 0; discharging = supply[s, "status"] == "Discharging" }
        }
        n = split(roots, r, " ")
        for (i = 1; i <= n; i++) root[r[i]] = 1
        for (p in parent) {
          q = p
          while (q > 0 && !(q in root)) q = (q in parent) ? parent[q] : 0
          if (q in root) { c[q] += cpu[p]; m[q] += rss[p] }
        }
        printf "%s %s %.0f %.0f %s %s %s", cores, load, used / 1048576, total / 1048576, disk, swap, booted
        if (pressure != "") printf " pressure=%s", pressure
        if (battery != "") printf " battery=%d:%d", battery, discharging
        for (i = 1; i <= n; i++) printf " %s:%.2f:%.0f", r[i], c[r[i]], m[r[i]] / 1024
        print ""
      }')" "$(sample)"
  fi
  tick=$((tick + 1))
  sleep 2
done
