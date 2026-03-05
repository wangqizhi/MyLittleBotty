#!/usr/bin/env bash
set -euo pipefail

# Download latest release binary into current directory and make it executable.
REPO="${REPO:-wangqizhi/MyLittleBotty}"
ASSET_NAME="${ASSET_NAME:-mylittlebotty}"
OUTPUT_PATH="${OUTPUT_PATH:-./$ASSET_NAME}"

URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"

echo "Downloading latest release asset..."
echo "Repo:   $REPO"
echo "Asset:  $ASSET_NAME"
echo "Output: $OUTPUT_PATH"

curl -fL --retry 3 --retry-delay 1 -o "$OUTPUT_PATH" "$URL"

if [[ ! -s "$OUTPUT_PATH" ]]; then
  echo "Error: downloaded file is empty: $OUTPUT_PATH"
  exit 1
fi

chmod +x "$OUTPUT_PATH"

echo "Done."
ls -lh "$OUTPUT_PATH"
file "$OUTPUT_PATH" || true
