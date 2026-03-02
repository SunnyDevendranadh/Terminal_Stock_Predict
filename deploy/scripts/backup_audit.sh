#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="${1:-/var/lib/quant-platform}"
BACKUP_DIR="${2:-/var/backups/quant-platform}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$BACKUP_DIR/audit-backup-$STAMP.tar.gz"

mkdir -p "$BACKUP_DIR"

tar -czf "$OUT" -C "$STATE_DIR" audit checkpoints

echo "Created $OUT"
