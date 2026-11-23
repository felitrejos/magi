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
tick=0 registry=- placed='' was='' os=$(uname -s) nl='
'
battery() {
  if [ "$os" = Darwin ]; then
    pmset -g batt
  else
    grep -s . /dev/null /sys/class/power_supply/*/type /sys/class/power_supply/*/scope /sys/class/power_supply/*/capacity /sys/class/power_supply/*/status
  fi
}
usage() {
  if [ "$os" = Darwin ]; then
    ps -A -o pid= -o ppid= -o time= -o time= -o rss= -o ucomm=
  else
    cat /proc/[0-9]*/stat 2>/dev/null | awk -v hz="$(getconf CLK_TCK)" -v page="$(getconf PAGESIZE)" '$1 ~ /^[0-9]+$/ && match($0, /.*\)/) {
      split(substr($0, RLENGTH + 2), f, " ")
      printf "%s %s %.2f %.2f %.0f %s\n", $1, f[2], (f[12] + f[13] + f[14] + f[15]) / hz, (f[12] + f[13]) / hz, f[22] * page / 1024, substr($0, index($0, "(") + 1, RLENGTH - index($0, "(") - 1)
    }'
  fi
}
places() {
  {
    if [ "$os" = Darwin ]; then
      tmux list-panes -a -F 'pane #{pane_pid} #{pane_id} #{session_id}' \; list-clients -F 'client #{client_activity} #{client_pid} #{client_tty} #{session_id}' 2>/dev/null
      ps -A -o pid= -o ppid= -o tty=
    else
      tmux list-panes -a -F 'pane #{pane_pid} #{pane_id} #{session_id}' 2>/dev/null
      ps -A -o pid= -o ppid=
      for pid in $1; do
        tr '\0' '\n' 2>/dev/null < "/proc/$pid/environ" | sed -n "s/^REX_SESSION=/rex $pid session /p; s/^REX_BLOCK=/rex $pid block /p"
      done
    fi
  } | awk -v pids="$1" -v os="$os" "$quote"'
    function place(pid, key, value) {
      if (value != "") print "place", pid, key, value
    }
    $1 == "pane" { pane[$2] = $3; sessions[$2] = sessions[$2] " " $4 }
    $1 == "client" && (!($5 in active) || $2 >= active[$5]) { active[$5] = $2 + 0; client[$5] = $3; seat[$5] = $4 }
    $1 == "rex" && $3 == "session" { rs[$2] = $4 }
    $1 == "rex" && $3 == "block" { rb[$2] = $4 }
    $1 ~ /^[0-9]+$/ { parent[$1] = $2; tty[$1] = $3 }
    END {
      n = split(pids, p, " ")
      for (i = 1; i <= n; i++) {
        s = p[i]
        for (q = s; q > 1 && !(q in pane); q = parent[q] + 0) continue
        if (q in pane) {
          place(s, "tmux", pane[q])
          k = split(sessions[q], linked, " ")
          held = ""
          for (j = 1; j <= k; j++) if (linked[j] in active && (held == "" || active[linked[j]] > active[held])) held = linked[j]
          if (held != "") {
            place(s, "tty", seat[held])
            top[s] = client[held]
          }
        } else {
          if (tty[s] ~ /^tty/) place(s, "tty", "/dev/" tty[s])
          if (s in rs && s in rb) place(s, "rex", rs[s] " " rb[s])
          top[s] = s
        }
        for (q = top[s]; q > 1; q = parent[q] + 0) chain[q] = 1
      }
      if (os != "Darwin") exit
      list = ""
      for (q in chain) list = list "," q
      if (list == "") exit
      read = "ps -o pid= -o comm= -p " substr(list, 2)
      while ((read | getline line) > 0) {
        q = line + 0
        sub(/^ *[0-9]+ +/, "", line)
        if ((at = index(line, ".app/")) > 0) bundle[q] = substr(line, 1, at + 3)
      }
      close(read)
      for (i = 1; i <= n; i++) {
        b = ""
        for (q = top[p[i]]; q > 1; q = parent[q] + 0) if (q in bundle) b = bundle[q]
        if (b == "") continue
        if (!(b in id)) {
          read = "plutil -extract CFBundleIdentifier raw -o - " quote(b "/Contents/Info.plist")
          read | getline id[b]
          close(read)
        }
        place(p[i], "app", id[b])
      }
    }'
}
if [ "$os" = Darwin ]; then root=$(diskutil info / | sed -n 's/^ *Volume Name: *//p'); fi
escape='function escape(s, out, c) {
  if (!("%" in code)) for (c = 1; c < 128; c++) if (c <= 32 || c == 37 || c == 58 || c == 127) code[sprintf("%c", c)] = sprintf("%%%02X", c)
  while (match(s, /[\001-\040%:\177]/)) { out = out substr(s, 1, RSTART - 1) code[substr(s, RSTART, 1)]; s = substr(s, RSTART + 1) }
  return out s
}'
quote='function quote(s) { gsub(/\047/, "\047\\\047\047", s); return "\047" s "\047" }'
volumes() {
  if [ "$os" = Darwin ]; then
    mount
  else
    cat /proc/mounts
  fi | awk -v os="$os" -v root="$root" "$escape$quote"'
    os == "Darwin" { mount = $0; sub(/^[^ ]+ on /, "", mount); sub(/ \([^(]*\)$/, "", mount); kept = $1 ~ /^\/dev\// && (mount == "/" || (mount ~ /^\/Volumes\// && !/nobrowse/)) }
    os != "Darwin" {
      mount = ""
      for (rest = $2; match(rest, /\\[0-7][0-7][0-7]/); rest = substr(rest, RSTART + 4)) mount = mount substr(rest, 1, RSTART - 1) sprintf("%c", substr(rest, RSTART + 1, 1) * 64 + substr(rest, RSTART + 2, 1) * 8 + substr(rest, RSTART + 3, 1))
      mount = mount rest
      kept = $1 !~ /@/ && $3 !~ /^(tmpfs|devtmpfs|ramfs|overlay|squashfs|erofs|proc|sysfs|cgroup2?|autofs|efivarfs|nsfs|devpts|mqueue|securityfs|debugfs|tracefs|configfs|fusectl|pstore|bpf|hugetlbfs|binfmt_misc|rpc_pipefs|selinuxfs|nfs4?|nfsd|cifs|smb3|smbfs|ceph|9p|fuse(\..*)?)$/
    }
    kept && (!($1 in at) || length(mount) < length(at[$1])) { at[$1] = mount; order[$1] = NR }
    END {
      for (d in at) if (!(at[d] in top) || order[d] > order[top[at[d]]]) top[at[d]] = d
      for (mount in top) {
        name[mount] = os != "Darwin" ? mount : mount == "/" ? root : substr(mount, 10)
        list = list " " quote(mount)
      }
      if (list == "") exit
      read = "df -Pk" list
      while ((read | getline line) > 0) {
        split(line, f, " ")
        mount = line
        sub(/^[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ +/, "", mount)
        if (mount in name) printf " vol=%.0f:%.0f:%s:%s", (f[2] - f[4]) / 1024, f[2] / 1024, escape(mount), escape(name[mount])
      }
    }'
}
sample() {
  if [ "$os" = Darwin ]; then
    { iostat -n 0 -C -c 2 -w 1; notifyutil -g com.apple.system.thermalpressurelevel; route -n get default 2>/dev/null; netstat -ibn; vm_stat; sysctl -n hw.memsize; } | awk '
      NR == 4 { printf " cpu=%d", $1 + $2 }
      $1 == "com.apple.system.thermalpressurelevel" { printf " thermal=%d", $2 }
      $1 == "interface:" { iface = $2 }
      $1 == iface && $3 ~ /^<Link/ { printf " net=%s:%s:%s", $(NF - 4), $(NF - 1), iface }
      $3 ~ /^<Link/ && $1 !~ /^lo/ && $(NF - 4) + $(NF - 1) > 0 { name = $1; sub(/\*$/, "", name); printf " nic=%s:%s:%s", $(NF - 4), $(NF - 1), name }
      /^Mach Virtual Memory Statistics:/ { page = $(NF - 1) / 1048576 }
      /^Pages free:/ { free = $NF }
      /^Pages purgeable:/ { purgeable = $NF }
      /^File-backed pages:/ { file = $NF }
      /^Anonymous pages:/ { anonymous = $NF }
      /^Pages occupied by compressor:/ { compressor = $NF }
      /^[0-9]+$/ { total = $1 }
      END {
        app = sprintf("%.0f", (anonymous - purgeable) * page)
        compressed = sprintf("%.0f", compressor * page)
        cached = sprintf("%.0f", (file + purgeable) * page)
        free = sprintf("%.0f", free * page)
        printf " part=app:%d part=wired:%d part=compressed:%d part=cached:%d part=free:%d", app, sprintf("%.0f", total / 1048576) - app - compressed - cached - free, compressed, cached, free
      }'
    return
  fi
  { cat /proc/meminfo /proc/net/dev; grep -s . /dev/null /sys/class/hwmon/hwmon*/name /sys/class/hwmon/hwmon*/temp*_input /sys/class/hwmon/hwmon*/temp*_label /sys/class/hwmon/hwmon*/temp*_crit /sys/class/thermal/thermal_zone*/type /sys/class/thermal/thermal_zone*/temp; } | awk "$escape"'
    /^\/sys\/class\/thermal\// {
      at = index($0, ":")
      split(substr($0, 1, at - 1), path, "/")
      zone[path[5], path[6]] = substr($0, at + 1)
      next
    }
    /^\/sys\// {
      at = index($0, ":")
      split(substr($0, 1, at - 1), path, "/")
      split(path[6], file, "_")
      if (path[6] == "name") chip[path[5]] = substr($0, at + 1)
      else { reading[path[5] " " file[1], file[2]] = substr($0, at + 1); hwmon[path[5] " " file[1]] = path[5] }
      next
    }
    /^MemTotal:/ { total = $2 }
    /^MemFree:/ { free = $2 }
    /^MemAvailable:/ { available = $2 }
    (at = index($0, ":")) && split(substr($0, at + 1), f, " ") == 16 {
      name = substr($0, 1, at - 1)
      gsub(/ /, "", name)
      if (name != "lo" && name !~ /^veth/ && f[1] + f[9] > 0) printf " nic=%s:%s:%s", f[1], f[9], name
    }
    END {
      if (available < free) free = available
      total = sprintf("%.0f", total / 1024)
      available = sprintf("%.0f", available / 1024)
      free = sprintf("%.0f", free / 1024)
      printf " part=app:%d part=cached:%d part=free:%d", total - available, available - free, free
      for (d in chip) for (e in chip) if (chip[e] == chip[d]) { shared[d]++; if (substr(e, 6) + 0 < substr(d, 6) + 0) rank[d]++ }
      for (s in hwmon) {
        if (!((s, "input") in reading)) continue
        d = hwmon[s]
        id[d] = shared[d] > 1 ? chip[d] (rank[d] + 0) : chip[d]
        label[s] = (s, "label") in reading ? reading[s, "label"] : id[d]
        labels[label[s]]++
        same[label[s], d]++
      }
      for (s in label) {
        printf " sensor=%.1f:", reading[s, "input"] / 1000
        if ((s, "crit") in reading) printf "%.1f", reading[s, "crit"] / 1000
        channel = substr(s, index(s, " ") + 1)
        if (labels[label[s]] > 1) label[s] = label[s] " (" (same[label[s], hwmon[s]] == labels[label[s]] ? channel : same[label[s], hwmon[s]] > 1 ? id[hwmon[s]] " " channel : id[hwmon[s]]) ")"
        printf ":%s", escape(label[s])
      }
      split("coretemp Package id 0|k10temp Tdie|k10temp Tctl", wanted, "|")
      for (k = 1; k <= 3 && picked == ""; k++) for (s in label) if (((s, "label") in reading) && chip[hwmon[s]] " " reading[s, "label"] == wanted[k]) picked = s
      if (picked != "") {
        printf " temp=%.1f", reading[picked, "input"] / 1000
        if ((picked, "crit") in reading) printf ":%.1f", reading[picked, "crit"] / 1000
        exit
      }
      split("x86_pkg_temp cpu-thermal", wanted, " ")
      for (k = 1; k <= 2; k++) for (z in zone) {
        split(z, key, SUBSEP)
        if (key[2] == "type" && zone[z] == wanted[k] && ((key[1], "temp") in zone)) {
          printf " temp=%.1f", zone[key[1], "temp"] / 1000
          exit
        }
      }
    }'
  read -r _ user nice system idle iowait irq softirq steal _ < /proc/stat
  printf ' ticks=%s:%s' $((user + nice + system + irq + softirq + steal)) $((user + nice + system + idle + iowait + irq + softirq + steal))
  iface=
  while read -r device destination _; do
    case $device in lo | veth* | docker* | br-* | tailscale*) continue ;; esac
    if [ -z "$iface" ] && [ "$destination" = 00000000 ]; then iface=$device; fi
  done < /proc/net/route
  while IFS=': ' read -r device received _ _ _ _ _ _ _ sent _; do
    if [ "$device" = "$iface" ]; then printf ' net=%s:%s:%s' "$received" "$sent" "$iface"; fi
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
  if [ "$live" != "$registry" ] || [ $((tick % 15)) -eq 0 ]; then
    found=$(places "$pids")
    if [ "$live$found" != "$registry$placed" ]; then
      registry=$live placed=$found
      case $live in *'"status":"waiting"'*) live="${live}waiting $("$cli" agents --json | tr -d '\n')
" ;; esac
      if [ -n "$found" ]; then live="$live$found
"; fi
      printf '%s' "$live"
      echo sessions
    fi
  fi
  if [ $((tick % 15)) -eq 0 ]; then
    measured=$({ load; df -Pk "$HOME"; if [ -n "$was" ]; then printf '%s\n' "$was"; fi; usage; battery; } | awk -v roots="$pids" -v os="$os" "$escape$quote"'
      function pick(v, list, k, p, best) {
        for (k = 0; k < 8; k++) {
          best = ""
          for (p in v) if (!((list, p) in got) && (best == "" || v[p] > v[best])) best = p
          if (best == "") return
          got[list, best] = 1
          shown[best] = 1
        }
      }
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
      NR == 6 { disk = sprintf("%.0f %.0f", ($2 - $4) / 1024, $2 / 1024) }
      /^was / { was[$2] = $3; sampled = 1; next }
      NR > 6 && $1 ~ /^[0-9]+$/ {
        parent[$1] = $2; cpu[$1] = seconds($3); own[$1] = seconds($4); rss[$1] = $5
        title = $0
        sub(/^ *[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ +[^ ]+ /, "", title)
        sub(/ +$/, "", title)
        name[$1] = title
      }
      END {
        for (s in type) {
          if (type[s] == "Battery" && supply[s, "scope"] != "Device") { battery = supply[s, "capacity"] + 0; discharging = supply[s, "status"] == "Discharging" }
        }
        n = split(roots, r, " ")
        for (i = 1; i <= n; i++) root[r[i]] = 1
        for (p in parent) {
          q = p
          while (q > 0 && !(q in root)) q = (q in parent) ? parent[q] : 0
          if (q in root) { c[q] += cpu[p]; m[q] += rss[p]; owner[p] = q }
        }
        printf "%s %s %.0f %.0f %s %s %s", cores, load, used / 1048576, total / 1048576, disk, swap, booted
        if (pressure != "") printf " pressure=%s", pressure
        if (battery != "") printf " battery=%d:%d", battery, discharging
        for (i = 1; i <= n; i++) printf " %s:%.2f:%.0f", r[i], c[r[i]], m[r[i]] / 1024
        if (sampled) {
          for (p in own) spent[p] = p in was && own[p] >= was[p] ? own[p] - was[p] : own[p]
          pick(spent, "cpu")
          pick(rss, "memory")
          if (os == "Darwin") {
            list = ""
            for (p in shown) list = list "," p
            read = "ps -o pid= -o comm= -p " substr(list, 2)
            files = ""
            while ((read | getline line) > 0) {
              q = line + 0
              sub(/^ *[0-9]+ /, "", line)
              k = split(line, path, "/")
              if (line !~ /^\// || length(name[q]) != 16 || index(path[k], name[q]) != 1) continue
              if (!(line in candidate)) files = files " " quote(line)
              candidate[line] = candidate[line] " " q
            }
            read = "for f in" files "; do [ -f \"$f\" ] && printf \"%s\\n\" \"$f\"; done"
            if (files != "") while ((read | getline line) > 0) {
              k = split(line, path, "/")
              count = split(candidate[line], pids, " ")
              for (j = 1; j <= count; j++) name[pids[j]] = path[k]
            }
          }
          for (p in shown) printf " proc=%s:%.2f:%.0f:%d:%s", p, spent[p], rss[p] / 1024, owner[p], escape(name[p])
        }
        print ""
        for (p in own) printf "was %s %.2f\n", p, own[p]
      }')
    printf 'stats %s%s%s\n' "${measured%%"$nl"*}" "$(sample)" "$(volumes)"
    was=${measured#*"$nl"}
  fi
  tick=$((tick + 1))
  sleep 2
done
