#!/bin/sh
load() {
  if [ "$(uname -s)" = Darwin ]; then
    sysctl -n hw.ncpu vm.loadavg hw.memsize hw.pagesize vm.page_free_count vm.page_pageable_external_count vm.page_purgeable_count | awk 'NR == 1 { print } NR == 2 { print $2, $3, $4 } { v[NR] = $1 } END { printf "%.0f %.0f\n", v[3], (v[5] + v[6] + v[7]) * v[4] }'
  else
    getconf _NPROCESSORS_ONLN
    cut -d ' ' -f 1-3 /proc/loadavg
    awk '/^MemTotal:/ { t = $2 } /^MemAvailable:/ { a = $2 } END { print t * 1024, a * 1024 }' /proc/meminfo
  fi
}
