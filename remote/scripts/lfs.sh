#!/bin/sh
set -e
command=$1
cd -- "$2"

case $command in
  missing)
    git lfs version >/dev/null 2>&1 || exit 0
    store=$(git lfs env 2>/dev/null | sed -n 's/^LocalMediaDir=//p')
    [ -n "$store" ] || exit 0
    mkdir -p "$store"
    printf '%s\0' "$store"
    tr '\0' '\n' | while IFS= read -r object; do
      if [ ! -f "$store/$object" ]; then printf '%s\0' "$object"; fi
    done
    ;;
  verify)
    if command -v sha256sum >/dev/null; then set -- sha256sum; else set -- shasum -a 256; fi
    bad=$(xargs -0 "$@" | awk '{ n = split($2, p, "/"); if ($1 != p[n]) print $2 }')
    if [ -n "$bad" ]; then
      printf '%s\n' "$bad" | xargs rm -f
      printf 'these Git LFS objects arrived corrupted and were removed:\n%s\n' "$bad" >&2
      exit 1
    fi
    ;;
esac
