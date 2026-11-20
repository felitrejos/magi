#!/bin/sh
set -eu
tool=$1 version=$2 bin=$3
case $(uname -s)-$(uname -m) in
  Linux-x86_64) claude_platform=linux-x64 codex_target=x86_64-unknown-linux-musl ;;
  Darwin-arm64) claude_platform=darwin-arm64 codex_target=aarch64-apple-darwin ;;
  *) echo "no $tool build for $(uname -sm)" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$bin"
case $tool in
  claude)
    release=https://downloads.claude.ai/claude-code-releases/$version
    sum=$(curl -fsSL "$release/manifest.json" | jq -r --arg p "$claude_platform" '.platforms[$p].checksum')
    curl -fsSL -o "$tmp/claude" "$release/$claude_platform/claude"
    echo "$sum  $tmp/claude" | shasum -a 256 -c - >/dev/null
    install -m 755 "$tmp/claude" "$bin/claude"
    ;;
  codex)
    asset=codex-$codex_target.tar.gz
    sum=$(gh api "repos/openai/codex/releases/tags/rust-v$version" --jq ".assets[] | select(.name == \"$asset\") | .digest")
    curl -fsSL -o "$tmp/$asset" "https://github.com/openai/codex/releases/download/rust-v$version/$asset"
    echo "${sum#sha256:}  $tmp/$asset" | shasum -a 256 -c - >/dev/null
    tar -xzf "$tmp/$asset" -C "$tmp"
    install -m 755 "$tmp/codex-$codex_target" "$bin/codex"
    ;;
  *) echo "usage: $0 claude|codex VERSION BIN_DIR" >&2; exit 2 ;;
esac
