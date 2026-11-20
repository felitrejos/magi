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
