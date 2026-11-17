#!/bin/sh
set -e
cd -- "$1"
git lfs version >/dev/null 2>&1 || exit 0
store=$(git lfs env 2>/dev/null | sed -n 's/^LocalMediaDir=//p')
[ -n "$store" ] || exit 0
mkdir -p "$store"
printf '%s\0' "$store"
tr '\0' '\n' | while IFS= read -r object; do
  if [ ! -f "$store/$object" ]; then printf '%s\0' "$object"; fi
done
