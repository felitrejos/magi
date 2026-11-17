#!/bin/sh
role=$1 marker=$2 command=$3
shell=${SHELL:-/bin/sh}
case $role in
  setup)
    trap 'if [ ! -s "$marker" ]; then echo failed > "$marker"; fi; exit 1' HUP TERM
    printf 'magi: setting up: %s\n' "$command"
    if "$shell" -lc "$command"; then echo ok > "$marker"; else echo failed > "$marker"; fi
    trap - HUP TERM
    ;;
  service)
    if [ -n "$marker" ]; then
      printf 'magi: waiting for setup before starting: %s\n' "$command"
      while [ ! -s "$marker" ]; do sleep 1; done
    fi
    if [ -z "$marker" ] || [ "$(cat "$marker")" = ok ]; then
      "$shell" -lc "$command"
    else
      printf 'magi: setup failed, so this did not start: %s\n' "$command"
    fi
    ;;
esac
exec "$shell" -l
