#!/usr/bin/env bash
set -euo pipefail

# Download latest release binary and install into a user bin directory.
REPO="${REPO:-wangqizhi/MyLittleBotty}"
ASSET_NAME="${ASSET_NAME:-mylittlebotty}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.mylittlebotty/bin}"
OUTPUT_PATH="${OUTPUT_PATH:-$INSTALL_DIR/$ASSET_NAME}"
PATH_EXPORT_LINE="export PATH=\"$INSTALL_DIR:\$PATH\""
PATH_MARKER="# mylittlebotty-path"
BOSS_PID_FILE="${HOME}/.mylittlebotty/run/boss.pid"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Error: only macOS is supported for now."
  exit 1
fi

find_botty_pids() {
  local boss_pid=""
  if [[ -f "$BOSS_PID_FILE" ]]; then
    boss_pid="$(tr -d '[:space:]' < "$BOSS_PID_FILE" || true)"
  fi

  if [[ -z "$boss_pid" || ! "$boss_pid" =~ ^[0-9]+$ ]]; then
    return 0
  fi

  if ! kill -0 "$boss_pid" 2>/dev/null; then
    rm -f "$BOSS_PID_FILE"
    return 0
  fi

  local -a queue=("$boss_pid")
  local -a all=("$boss_pid")
  local current children child

  while [[ ${#queue[@]} -gt 0 ]]; do
    current="${queue[0]}"
    queue=("${queue[@]:1}")
    children="$(pgrep -P "$current" || true)"
    if [[ -z "$children" ]]; then
      continue
    fi
    while IFS= read -r child; do
      [[ -z "$child" ]] && continue
      all+=("$child")
      queue+=("$child")
    done <<< "$children"
  done

  printf "%s\n" "${all[@]}" | awk '!seen[$0]++'
}

stop_running_botty_if_needed() {
  local pids
  pids="$(find_botty_pids)"
  if [[ -z "$pids" ]]; then
    return 0
  fi

  echo "Detected running Botty process(es):"
  ps -p "$(echo "$pids" | paste -sd, -)" -o pid=,command= || true
  echo ""
  local answer=""
  if [[ "${BOTTY_INSTALL_FORCE:-}" == "1" ]]; then
    answer="y"
  elif [[ -r /dev/tty ]]; then
    read -r -p "Stop these processes and continue install? [y/N]: " answer < /dev/tty
  else
    echo "Installation aborted: no interactive terminal for confirmation."
    echo "Set BOTTY_INSTALL_FORCE=1 to force stop and continue."
    exit 1
  fi
  case "${answer:-}" in
    y|Y|yes|YES)
      kill $pids || true
      sleep 1
      local remaining
      remaining="$(find_botty_pids)"
      if [[ -n "$remaining" ]]; then
        kill -9 $remaining || true
      fi
      remaining="$(find_botty_pids)"
      if [[ -n "$remaining" ]]; then
        echo "Error: failed to stop all Botty processes."
        exit 1
      fi
      rm -f "$BOSS_PID_FILE"
      ;;
    *)
      echo "Installation aborted."
      exit 1
      ;;
  esac
}

stop_running_botty_if_needed

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
