#!/bin/sh
set -eu

main() {
  repo="felitrejos/magi"
  dir="${MAGI_INSTALL_DIR:-$HOME/.local/bin}"
  bins="magi magi-remote"

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
  echo "Installed $bins to $dir"

  case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Add $dir to your PATH" ;;
  esac

  printf '\nNext:\n'
  printf '  magi init            on each machine, to move sessions and send work\n'
  printf '  magi agent install   on every machine, for the overview\n'
  printf '  magi                 in a Rex pane\n'
}

main "$@"
