#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-$HOME/.mylittlebotty/bin}"
BASE_DIR="$(dirname "$INSTALL_DIR")"
PATH_MARKER="# mylittlebotty-path"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Error: only macOS is supported for now."
  exit 1
fi

clean_profile() {
  local profile="$1"
  if [[ ! -f "$profile" ]]; then
    return 0
  fi

  local tmp
  tmp="$(mktemp)"
  awk -v marker="$PATH_MARKER" '
    skip_next == 1 { skip_next=0; next }
    $0 == marker { skip_next=1; next }
    { print }
  ' "$profile" > "$tmp"
  mv "$tmp" "$profile"
}

clean_profile "$HOME/.zshrc"
clean_profile "$HOME/.bash_profile"
clean_profile "$HOME/.bashrc"

if [[ -d "$BASE_DIR" ]]; then
  rm -rf "$BASE_DIR"
  echo "Removed: $BASE_DIR"
else
  echo "Not found: $BASE_DIR"
fi

echo "Uninstall completed."
echo "If command still exists in current shell, run:"
echo "  hash -r"
