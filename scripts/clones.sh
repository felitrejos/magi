#!/bin/sh
export LC_ALL=C
for root; do
  if [ "${root#\~/}" != "$root" ]; then root=$HOME/${root#\~/}; fi
  find "$root" -maxdepth 3 -name .git -type d -prune -print 2>/dev/null | while IFS= read -r g; do
    d=$(cd "${g%/.git}" && pwd -P)
    printf '%s\t%s\t%s\n' "$d" "$(git -C "$d" remote get-url origin 2>/dev/null)" "$(git -C "$d" rev-list --max-parents=0 HEAD 2>/dev/null | sort | head -n 1)"
  done
done
