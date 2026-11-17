#!/bin/sh
set -e
command=$1 dir=$2
case $command in
  list)
    if [ -d "$dir" ]; then
      cd -- "$dir"
      for f in * .[!.]*; do
        if [ -e "$f" ]; then printf '%s\0' "$f"; fi
      done
    fi
    ;;
  read)
    if [ -f "$dir/MEMORY.md" ]; then cat "$dir/MEMORY.md"; fi
    ;;
  write)
    mkdir -p -- "$dir"
    t=$(mktemp "$dir/.MEMORY.XXXXXX")
    cat > "$t"
    mv -f "$t" "$dir/MEMORY.md"
    ;;
esac
