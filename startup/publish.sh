#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
  cat <<'USAGE'
Usage:
  ./startup/publish.sh

Notes:
  - Requires GitHub CLI: gh
  - Requires login: gh auth login
  - Reads version from Cargo.toml and uses tag: v<version>
  - Uses title: MyLittleBotty v<version>
  - Expects existing build artifact: release/mylittlebotty-v<version>
USAGE
}

CARGO_TOML="$ROOT_DIR/Cargo.toml"

if ! command -v gh >/dev/null 2>&1; then
  echo "Error: gh command not found. Install GitHub CLI first."
  exit 1
fi

if ! gh auth status >/dev/null 2>&1; then
  echo "Error: gh is not authenticated. Run: gh auth login"
  exit 1
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Error: not in a git repository."
  exit 1
fi

if [[ ! -f "$CARGO_TOML" ]]; then
  echo "Error: Cargo.toml not found: $CARGO_TOML"
  exit 1
fi

VERSION="$(awk -F ' *= *' '
  /^\[package\]/ { in_package=1; next }
  /^\[/ && $0 !~ /^\[package\]/ { in_package=0 }
  in_package && $1=="version" {
    gsub(/"/, "", $2)
    print $2
    exit
  }
' "$CARGO_TOML")"

if [[ -z "$VERSION" ]]; then
  echo "Error: failed to read version from Cargo.toml"
  exit 1
fi

TAG="v$VERSION"
TITLE="MyLittleBotty v$VERSION"
ASSET_PATH="$ROOT_DIR/release/mylittlebotty-v$VERSION"
UPLOAD_TMP_PATH="/tmp/mylittlebotty"

if [[ ! -f "$ASSET_PATH" ]]; then
  echo "Error: build artifact not found: $ASSET_PATH"
  echo "Please build first: ./startup/build.sh"
  exit 1
fi

echo "[1/3] Ensuring tag exists on origin..."
if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "Local tag exists: $TAG"
else
  git tag "$TAG"
  echo "Created local tag: $TAG"
fi

git push origin "$TAG"

echo "[2/3] Creating or updating GitHub release..."
if gh release view "$TAG" >/dev/null 2>&1; then
  echo "Release already exists, skipping create."
else
  gh release create "$TAG" \
    --title "$TITLE" \
    --generate-notes
  echo "Release created: $TAG"
fi

echo "[3/3] Uploading artifact..."
cp "$ASSET_PATH" "$UPLOAD_TMP_PATH"
chmod +x "$UPLOAD_TMP_PATH"
gh release upload "$TAG" "$UPLOAD_TMP_PATH" --clobber
rm -f "$UPLOAD_TMP_PATH"

echo "Publish completed successfully."
echo "Tag: $TAG"
echo "Local artifact: $ASSET_PATH"
echo "Uploaded asset name: mylittlebotty"
