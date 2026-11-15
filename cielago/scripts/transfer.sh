#!/bin/sh
set -e
command=$1 dir=$2
shift 2

busybox() { tar --help 2>&1 | grep -q BusyBox; }

case $command in
  pack)
    cd -- "$dir"
    for e; do set -- "$@" --exclude "$e"; shift; done
    if busybox; then
      tr '\0' '\n' | tar "$@" -T - -cf -
      exit 0
    fi
    if tar --version 2>/dev/null | grep -q bsdtar; then set -- --no-mac-metadata "$@"; fi
    COPYFILE_DISABLE=1 tar --no-xattrs --no-acls "$@" --null -T - -cf -
    ;;
  unpack)
    keep=$1
    mkdir -p -- "$dir"
    if [ "$keep" != 1 ]; then
      tar -xpf - -C "$dir"
    elif ! busybox; then
      tar --keep-newer-files -xpf - -C "$dir"
    else
      t=$(mktemp -d)
      trap 'rm -rf "$t"' EXIT
      tar -xpf - -C "$t"
      (cd "$t" && find . ! -type d) | while IFS= read -r f; do
        if [ ! -e "$dir/$f" ] || [ -z "$(find "$dir/$f" -prune -newer "$t/$f")" ]; then
          mkdir -p "$(dirname "$dir/$f")"
          mv -f "$t/$f" "$dir/$f"
        fi
      done
    fi
    ;;
esac
