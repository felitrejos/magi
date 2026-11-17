#!/bin/sh
set -e
cd -- "$1"
tab=$(printf '\t')
staged=$(mktemp)
trap 'rm -f "$staged"' EXIT
git ls-files -s -z > "$staged"
tr '\0' '\n' < "$staged" | while IFS= read -r line; do
  path=${line#*"$tab"}
  if [ "${line%% *}" = 160000 ] && [ -e "$path/.git" ]; then printf '%s\0' "$path"; fi
done
