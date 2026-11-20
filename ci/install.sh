#!/bin/sh
set -eu
version=$1 bin=$2
case $(uname -s)-$(uname -m) in
  Linux-x86_64) platform=linux-x64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "no Claude Code build for $(uname -sm)" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$bin"
release=https://downloads.claude.ai/claude-code-releases/$version
sum=$(curl -fsSL "$release/manifest.json" | jq -r --arg p "$platform" '.platforms[$p].checksum')
curl -fsSL -o "$tmp/claude" "$release/$platform/claude"
echo "$sum  $tmp/claude" | shasum -a 256 -c - >/dev/null
install -m 755 "$tmp/claude" "$bin/claude"
