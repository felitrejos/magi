#!/bin/sh
set -e
command=$1 dir=$2
shift 2

busybox() { tar --help 2>&1 | grep -q BusyBox; }

case $command in
  pack)
    cd -- "$dir"
    follow=$1
    shift
    for e; do set -- "$@" --exclude "$e"; shift; done
    if [ "$follow" = 1 ]; then
      if tar --version 2>/dev/null | grep -q bsdtar; then set -- -H "$@"; else set -- -h "$@"; fi
    fi
    if busybox; then
      tr '\0' '\n' | tar "$@" -T - -cf -
      exit 0
    fi
    if tar --version 2>/dev/null | grep -q bsdtar; then set -- --no-mac-metadata "$@"; fi
    COPYFILE_DISABLE=1 tar --no-xattrs --no-acls "$@" --null -T - -cf -
    ;;
  unpack)
    mkdir -p -- "$dir"
    dir=$(cd -- "$dir" && pwd)
    t=$(mktemp -d "$(dirname "$dir")/.magi-XXXXXX")
    trap 'rm -rf "$t"' EXIT
    mkdir "$t/x"
    tar -xpf - -C "$t/x"
    cd "$t/x"
    find . -depth -type d -print0 > "$t/dirs"
    # shellcheck disable=SC2016
    keep=$1 dir=$dir xargs -0 sh -c '
      set -e
      for p; do
        [ -d "$dir/$p" ] && [ ! -L "$dir/$p" ] || continue
        set --
        for f in "$p"/* "$p"/.[!.]* "$p"/..?*; do
          [ -e "$f" ] || [ -L "$f" ] || continue
          d=$dir/$f
          [ -e "$d" ] || [ -L "$d" ] || continue
          if [ -d "$f" ] && [ ! -L "$f" ] && [ -d "$d" ] && [ ! -L "$d" ]; then
            set -- "$@" "$f"
          elif [ "$keep" = 1 ] && [ -n "$(find "$d" -prune -newer "$f")" ]; then
            rm -rf "$f"
          elif [ -d "$d" ] && [ ! -L "$d" ]; then
            rmdir "$d"
          elif [ -L "$d" ] || [ -d "$f" ]; then
            rm -f "$d"
          fi
        done
        if [ $# -gt 0 ]; then rmdir "$@"; fi
        for g in "*" ".[!.]*" "..?*"; do
          set -- "$p"/$g
          if [ -e "$1" ] || [ -L "$1" ]; then mv -f "$@" "$dir/$p"; fi
        done
      done' sh < "$t/dirs"
    ;;
esac
