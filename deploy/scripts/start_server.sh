#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

if [[ -f /etc/quant-platform/quant-platform.env ]]; then
  set -a
  # shellcheck disable=SC1091
  source /etc/quant-platform/quant-platform.env
  set +a
fi

: "${QP_HOST:=127.0.0.1}"
: "${QP_PORT:=8080}"

mkdir -p "$(dirname "${AUDIT_SQLITE_PATH:-data/audit/audit_events.sqlite3}")"
mkdir -p "$(dirname "${AUDIT_OBJECT_STORE_PATH:-data/audit/object_store.ndjson}")"
mkdir -p "$(dirname "${AUDIT_CHECKPOINT_PATH:-data/checkpoints/off_provider_checkpoint.txt}")"

exec env PYTHONPATH=src python3 -m quant_platform.main --host "$QP_HOST" --port "$QP_PORT"
