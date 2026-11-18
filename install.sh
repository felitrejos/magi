#!/bin/sh
set -eu

main() {
  repo="felitrejos/magi"
  dir="${MAGI_INSTALL_DIR:-$HOME/.local/bin}"
  bins="magi"
  agent=""
  for unit in "$HOME/Library/LaunchAgents/magi-agent.plist" "$HOME/.config/systemd/user/magi-agent.service"; do
    [ -e "$unit" ] && agent="$unit"
  done
  if [ "${1:-}" = overview ] || [ -n "$agent" ]; then
    bins="magi magi-overview"
  fi

  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-musl ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
    *) echo "magi has no build for $(uname -s) $(uname -m) yet" >&2; exit 1 ;;
  esac

  archive="magi-$target.tar.gz"
  url="https://github.com/$repo/releases/latest/download/$archive"
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT

  echo "Downloading $archive"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$archive" "$url"
  curl -fsSL --proto '=https' --tlsv1.2 -o "$work/$archive.sha256" "$url.sha256"

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$work" && sha256sum -c "$archive.sha256" >/dev/null)
  else
    (cd "$work" && shasum -a 256 -c "$archive.sha256" >/dev/null)
  fi

  tar -xzf "$work/$archive" -C "$work"
  mkdir -p "$dir"
  for bin in $bins; do
    install -m 755 "$work/$bin" "$dir/$bin.new"
    mv "$dir/$bin.new" "$dir/$bin"
  done
  rm -f "$dir/magi-remote"
  echo "Installed $bins to $dir"

  if [ -n "$agent" ]; then
    "$dir/magi-overview" agent install
    if grep -qE "/magi'? statusline\"" "$HOME/.claude/settings.json" 2>/dev/null; then
      "$dir/magi-overview" statusline install
    fi
  fi

  case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Add $dir to your PATH" ;;
  esac

  printf '\nNext:\n'
  printf '  magi init                      on each machine, to move sessions and send work\n'
  if [ -n "$agent" ] || [ "${1:-}" = overview ]; then
    printf '  magi-overview agent install    on every machine, for the overview\n'
    printf '  magi-overview                  in a Rex pane\n'
  fi
}

main "$@"
