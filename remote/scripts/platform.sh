#!/bin/sh
PATH="$PATH:$HOME/.local/bin:$HOME/go/bin:/opt/homebrew/bin:/usr/local/bin:/usr/sbin:/sbin"
claude=$1
case $claude in \~/*) claude=$HOME/${claude#\~/} ;; esac
if [ "$(uname -s)" = Darwin ]; then
  echo "macOS $(sw_vers -productVersion)"
else
  # shellcheck source=/dev/null
  name=$(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")
  echo "${name:-$(uname -s)}"
fi
uname -m
for t in cargo go node python3 swift xcodebuild gcc clang make nvidia-smi; do
  command -v "$t" >/dev/null 2>&1 && printf '%s ' "$t"
done
bounded() { if command -v timeout >/dev/null 2>&1; then timeout 3 "$@"; else "$@"; fi; }
for t in docker podman; do
  command -v "$t" >/dev/null 2>&1 && bounded "$t" info >/dev/null 2>&1 && printf '%s ' "$t"
done
echo
if [ "$(uname -s)" = Darwin ]; then
  sysctl -n hw.ncpu
  sysctl -n vm.loadavg | tr -d '{}'
  sysctl -n hw.memsize hw.pagesize vm.page_free_count vm.page_pageable_external_count vm.page_purgeable_count | awk '{ v[NR] = $1 } END { printf "%.0f %.0f\n", v[1], (v[3] + v[4] + v[5]) * v[2] }'
else
  getconf _NPROCESSORS_ONLN
  cut -d ' ' -f 1-3 /proc/loadavg
  awk '/^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 } END { print t * 1024, a * 1024 }' /proc/meminfo
fi
listed=$("$claude" agents --json) || exit
printf '%s\n' "$listed" | awk '$1 == "\"status\":" && $2 ~ /^"busy"/ { n++ } END { print n + 0 }'
