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
    keep=$1
    mkdir -p -- "$dir"
    t=$(mktemp -d "$dir/.magi-XXXXXX")
    trap 'chmod -R u+w "$t"; rm -rf "$t"' EXIT
    tar -xpf - -C "$t"
    relay() {
      if busybox; then
        (cd "$t" && find . -mindepth 1 -maxdepth 1 | tar -T - -cf -)
        return
      fi
      set --
      if tar --version 2>/dev/null | grep -q bsdtar; then set -- --no-mac-metadata; fi
      (cd "$t" && find . -mindepth 1 -maxdepth 1 -print0 | COPYFILE_DISABLE=1 tar --no-xattrs --no-acls "$@" --null -T - -cf -)
    }
    if [ "$keep" != 1 ]; then
      relay | tar -xpf - -C "$dir"
    elif ! busybox; then
      relay | tar --keep-newer-files -xpf - -C "$dir"
    else
      (cd "$t" && find . ! -type d) | while IFS= read -r f; do
        if [ ! -e "$dir/$f" ] || [ -z "$(find "$dir/$f" -prune -newer "$t/$f")" ]; then
          mkdir -p "$(dirname "$dir/$f")"
          mv -f "$t/$f" "$dir/$f"
        fi
      done
    fi
    ;;
esac
