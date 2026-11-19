#!/bin/sh
set -eu

main() {
  repo="felitrejos/magi"
  dir="${MAGI_INSTALL_DIR:-$HOME/.local/bin}"

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
  install -m 755 "$work/magi" "$dir/magi.new"
  mv "$dir/magi.new" "$dir/magi"
  echo "Installed magi to $dir"

  remove_overview

  case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Add $dir to your PATH" ;;
  esac

  printf '\nNext:\n'
  printf '  magi init              on each machine, to move sessions and send work\n'
  printf '  magi watch install     on your laptop, so sessions can be handed back to it\n'
}

remove_overview() {
  plist="$HOME/Library/LaunchAgents/magi-agent.plist"
  unit="$HOME/.config/systemd/user/magi-agent.service"
  if [ -e "$plist" ]; then
    launchctl bootout "gui/$(id -u)/magi-agent" 2>/dev/null || :
    rm -f "$plist"
    echo "Removed the magi-overview service"
  fi
  if [ -e "$unit" ]; then
    systemctl --user disable --now magi-agent.service 2>/dev/null || :
    rm -f "$unit"
    systemctl --user daemon-reload 2>/dev/null || :
    echo "Removed the magi-overview service"
  fi
  rm -f "$dir/magi-overview" "$dir/magi-remote"
  if grep -q 'magi-overview.* statusline' "$HOME/.claude/settings.json" 2>/dev/null; then
    echo "Your Claude Code status line still runs magi-overview, which is gone: remove the statusLine entry from ~/.claude/settings.json"
  fi
}

main "$@"
