#!/usr/bin/env bash
set -euo pipefail

HOST="${1:-127.0.0.1}"
PORT="${2:-8080}"
URL="http://${HOST}:${PORT}/health"

if command -v curl >/dev/null 2>&1; then
  body="$(curl -fsS "$URL")"
else
  body="$(URL="$URL" python3 -c 'import os,urllib.request;print(urllib.request.urlopen(os.environ["URL"],timeout=5).read().decode("utf-8"))')"
fi

printf '%s\n' "$body"

if [[ "$body" != *'"status": "ok"'* && "$body" != *'"status":"ok"'* ]]; then
  echo "Health check failed" >&2
  exit 1
fi
