#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

VERSION="$(awk -F ' *= *' '
  /^\[package\]/ { in_package=1; next }
  /^\[/ && $0 !~ /^\[package\]/ { in_package=0 }
  in_package && $1=="version" {
    gsub(/"/, "", $2)
    print $2
    exit
  }
' "$ROOT_DIR/Cargo.toml")"

if [[ -z "$VERSION" ]]; then
  echo "Error: failed to read version from Cargo.toml"
  exit 1
fi

cargo build --release

mkdir -p "$ROOT_DIR/release"
mkdir -p "$ROOT_DIR/release/old_version"

OUTPUT_NAME="mylittlebotty-v$VERSION"
OUTPUT_PATH="$ROOT_DIR/release/$OUTPUT_NAME"

# Keep only current version in release root; archive older versions.
find "$ROOT_DIR/release" -maxdepth 1 -type f -name 'mylittlebotty-v*' ! -name "$OUTPUT_NAME" -print0 | while IFS= read -r -d '' f; do
  base="$(basename "$f")"
  archive="$ROOT_DIR/release/old_version/$base"
  if [[ -f "$archive" ]]; then
    archive="$ROOT_DIR/release/old_version/${base}-$(date +%Y%m%d%H%M%S)"
  fi
  mv "$f" "$archive"
  echo "Archived previous build: $archive"
done

# Backward-compatible migration for legacy unversioned artifact.
if [[ -f "$ROOT_DIR/release/mylittlebotty" ]]; then
  legacy_archive="$ROOT_DIR/release/old_version/mylittlebotty-legacy-$(date +%Y%m%d%H%M%S)"
  mv "$ROOT_DIR/release/mylittlebotty" "$legacy_archive"
  echo "Archived legacy build: $legacy_archive"
fi

cp "$ROOT_DIR/target/release/mylittlebotty" "$OUTPUT_PATH"

chmod +x "$OUTPUT_PATH"
echo "Built: $OUTPUT_PATH"
