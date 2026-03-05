#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo build --release

mkdir -p "$ROOT_DIR/release"
cp "$ROOT_DIR/target/release/mylittlebotty" "$ROOT_DIR/release/mylittlebotty"

chmod +x "$ROOT_DIR/release/mylittlebotty"
echo "Built: $ROOT_DIR/release/mylittlebotty"
