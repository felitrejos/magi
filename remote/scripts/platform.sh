#!/bin/sh
PATH="$PATH:$HOME/.local/bin:$HOME/go/bin:/opt/homebrew/bin:/usr/local/bin:/usr/sbin:/sbin"
claude=$1
if [ "$(uname -s)" = Darwin ]; then
  echo "macOS $(sw_vers -productVersion)"
else
  # shellcheck source=/dev/null
  name=$(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")
  echo "${name:-$(uname -s)}"
fi
uname -m
for t in cargo go node python3 swift xcodebuild gcc clang make nvidia-smi; do
  command -v "$t" >/dev/null 2>&1 && printf '%s ' "$t"
done
bounded() { if command -v timeout >/dev/null 2>&1; then timeout 3 "$@"; else "$@"; fi; }
for t in docker podman; do
  command -v "$t" >/dev/null 2>&1 && bounded "$t" info >/dev/null 2>&1 && printf '%s ' "$t"
done
echo
load
working "$claude"
