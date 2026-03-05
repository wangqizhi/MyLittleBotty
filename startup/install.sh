#!/usr/bin/env bash
set -euo pipefail

# Download latest release binary and install into a user bin directory.
REPO="${REPO:-wangqizhi/MyLittleBotty}"
ASSET_NAME="${ASSET_NAME:-mylittlebotty}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.mylittlebotty/bin}"
OUTPUT_PATH="${OUTPUT_PATH:-$INSTALL_DIR/$ASSET_NAME}"
PATH_EXPORT_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
PATH_MARKER="# mylittlebotty-path"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Error: only macOS is supported for now."
  exit 1
fi

URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"

echo "Downloading latest release asset..."
echo "Repo:   $REPO"
echo "Asset:  $ASSET_NAME"
echo "Output: $OUTPUT_PATH"

mkdir -p "$INSTALL_DIR"
curl -fL --retry 3 --retry-delay 1 -o "$OUTPUT_PATH" "$URL"

if [[ ! -s "$OUTPUT_PATH" ]]; then
  echo "Error: downloaded file is empty: $OUTPUT_PATH"
  exit 1
fi

chmod +x "$OUTPUT_PATH"

add_path_to_profile() {
  local profile="$1"
  if [[ ! -f "$profile" ]]; then
    touch "$profile"
  fi

  if grep -Fq "$PATH_MARKER" "$profile"; then
    return 0
  fi

  {
    echo ""
    echo "$PATH_MARKER"
    echo "$PATH_EXPORT_LINE"
  } >> "$profile"
}

add_path_to_profile "$HOME/.zshrc"
add_path_to_profile "$HOME/.bash_profile"
add_path_to_profile "$HOME/.bashrc"

echo "Done."
ls -lh "$OUTPUT_PATH"
file "$OUTPUT_PATH" || true
echo ""
echo "You can run: $ASSET_NAME"
echo "If command is not found in current shell, run:"
echo "  source ~/.zshrc"
