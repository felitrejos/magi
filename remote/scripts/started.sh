#!/bin/sh
started() {
  if [ -r "/proc/$1/stat" ]; then
    sed 's/.*) //' "/proc/$1/stat" | cut -d ' ' -f 20
  else
    LC_ALL=C TZ=UTC ps -o lstart= -p "$1" 2>/dev/null | tr -s ' ' | sed 's/^ //;s/ $//'
  fi
}

args_of() {
  if [ -r "/proc/$1/cmdline" ]; then
    tr '\0' ' ' < "/proc/$1/cmdline"
  else
    ps -o args= -p "$1"
  fi
}

processes() {
  if [ -d /proc/self ]; then
    for dir in /proc/[0-9]*; do
      args=$(args_of "${dir#/proc/}" 2>/dev/null)
      if [ -n "$args" ]; then printf '%s %s\n' "${dir#/proc/}" "$args"; fi
    done
  else
    ps -axo pid=,args=
  fi
}

program_is() {
  awk -v want="$1" '{
    program = $2
    if (program ~ /(^|\/)(sh|bash|dash|zsh|node)$/) program = $3
    sub(/.*\//, "", program)
    if (program == want) print
  }'
}
