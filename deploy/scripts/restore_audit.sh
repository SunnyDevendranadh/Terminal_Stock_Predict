#!/usr/bin/env bash
set -euo pipefail

ARCHIVE_PATH="${1:?Usage: restore_audit.sh <archive-path> [state-dir]}"
STATE_DIR="${2:-/var/lib/quant-platform}"

if [[ ! -f "$ARCHIVE_PATH" ]]; then
  echo "Archive not found: $ARCHIVE_PATH" >&2
  exit 1
fi

mkdir -p "$STATE_DIR"
tar -xzf "$ARCHIVE_PATH" -C "$STATE_DIR"

echo "Restored audit artifacts into $STATE_DIR"
