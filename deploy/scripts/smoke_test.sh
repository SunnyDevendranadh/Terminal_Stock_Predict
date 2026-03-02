#!/usr/bin/env bash
set -euo pipefail

HOST="${1:-127.0.0.1}"
PORT="${2:-8080}"
BASE="http://${HOST}:${PORT}"

curl -fsS "$BASE/health"
echo
curl -fsS "$BASE/v1/ops/compliance"
echo
curl -fsS -X POST "$BASE/v1/predict" \
  -H 'Content-Type: application/json' \
  -d '{"features":{"open":0.2,"high":0.3,"low":0.1,"close":0.4,"volume":0.5},"model_version":"current"}'
echo
