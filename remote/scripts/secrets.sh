#!/bin/sh
set -e
export LC_ALL=C
cd -- "$1"
shift
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
find "$@" > "$d/found" 2>/dev/null || :
if ! git rev-parse --git-dir >/dev/null 2>&1; then cat "$d/found"; exit 0; fi
git check-ignore -z --stdin < "$d/found" || [ $? -eq 1 ]
