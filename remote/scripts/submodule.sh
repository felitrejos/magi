#!/bin/sh
set -e
repo=$1 path=$2 origin=$3
[ ! -e "$repo/$path/.git" ] || exit 0
git init --quiet "$repo/$path"
if [ -n "$origin" ]; then git -C "$repo/$path" remote add origin "$origin"; fi
cd -- "$repo"
if [ -f .gitmodules ]; then git config -f .gitmodules --null --get-regexp '\.path$' || [ $? -eq 1 ]; fi
