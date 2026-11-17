#!/bin/sh
set -e
export LC_ALL=C
cd -- "$1"
shift
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
find "$@" > "$d/found" 2>/dev/null || :
git check-ignore -z --stdin < "$d/found" || [ $? -eq 1 ]
