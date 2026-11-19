#!/bin/sh
set -eu
base=${MAGI_NIGHTLY_DIR:-$HOME/.local/share/magi-nightly}
repo=$base/magi
bin=$base/bin
tested=$repo/remote/tested.json
go_version=$(sed -n 's/^go //p' "$repo/remote/go.mod")
go_root=$base/toolchains/go$go_version/go
claude_releases=https://downloads.claude.ai/claude-code-releases
mkdir -p "$bin" "$base/logs"

newer() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    split(a, x, "."); split(b, y, ".")
    for (i = 1; i <= 3; i++) { if (x[i] + 0 > y[i] + 0) exit 0; if (x[i] + 0 < y[i] + 0) exit 1 }
    exit 1
  }'
}

if [ ! -x "$go_root/bin/go" ]; then
  archive=go$go_version.linux-amd64.tar.gz
  sum=$(curl -fsSL 'https://go.dev/dl/?mode=json&include=all' | jq -r --arg f "$archive" '.[].files[] | select(.filename == $f) | .sha256')
  curl -fsSL -o "$base/$archive" "https://go.dev/dl/$archive"
  echo "$sum  $base/$archive" | sha256sum -c - >/dev/null
  mkdir -p "${go_root%/go}"
  tar -xzf "$base/$archive" -C "${go_root%/go}"
  rm "$base/$archive"
fi

claude_version=$(curl -fsSL "$claude_releases/latest")
if [ ! -x "$bin/claude-$claude_version" ]; then
  sum=$(curl -fsSL "$claude_releases/$claude_version/manifest.json" | jq -r '.platforms["linux-x64"].checksum')
  curl -fsSL -o "$bin/claude.download" "$claude_releases/$claude_version/linux-x64/claude"
  echo "$sum  $bin/claude.download" | sha256sum -c - >/dev/null
  chmod 755 "$bin/claude.download"
  mv "$bin/claude.download" "$bin/claude-$claude_version"
fi
ln -sf "claude-$claude_version" "$bin/claude"

codex_release=$(curl -fsSL https://api.github.com/repos/openai/codex/releases/latest)
codex_tag=$(printf '%s' "$codex_release" | jq -r .tag_name)
codex_version=${codex_tag#rust-v}
if [ ! -x "$bin/codex-$codex_version" ]; then
  asset=codex-x86_64-unknown-linux-musl.tar.gz
  sum=$(printf '%s' "$codex_release" | jq -r --arg a "$asset" '.assets[] | select(.name == $a) | .digest' | sed 's/^sha256://')
  curl -fsSL -o "$bin/codex.tar.gz" "https://github.com/openai/codex/releases/download/$codex_tag/$asset"
  echo "$sum  $bin/codex.tar.gz" | sha256sum -c - >/dev/null
  tar -xzf "$bin/codex.tar.gz" -C "$bin"
  mv "$bin/codex-x86_64-unknown-linux-musl" "$bin/codex-$codex_version"
  rm "$bin/codex.tar.gz"
fi
ln -sf "codex-$codex_version" "$bin/codex"

passed=
for tool in claude codex; do
  version=$claude_version pattern=^TestContractClaude
  if [ "$tool" = codex ]; then version=$codex_version pattern=^TestContractCodex; fi
  log=$base/logs/$(date +%F)-$tool-$version.log
  if (cd "$repo/remote" && PATH="$bin:$go_root/bin:$PATH" DISABLE_AUTOUPDATER=1 go test -count=1 -tags contract -run "$pattern" -v ./...) > "$log" 2>&1 &&
    grep -q -- '--- PASS' "$log" && ! grep -q -- '--- SKIP' "$log"; then
    echo "$tool $version passed"
    if newer "$version" "$(jq -r ".$tool.max" "$tested")"; then
      jq --arg v "$version" ".$tool.max = \$v" "$tested" > "$tested.new"
      mv "$tested.new" "$tested"
      passed="$passed $tool $version,"
    fi
  else
    echo "$tool $version failed, see $log" >&2
  fi
done

if [ -n "$passed" ]; then
  git -C "$repo" commit -q -m "Nightly: contract tests pass with${passed%,}" -- remote/tested.json
  git -C "$repo" -c credential.helper= -c 'credential.helper=!gh auth git-credential' push -q origin HEAD:main
fi
