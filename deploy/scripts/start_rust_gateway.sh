#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT/rust/vps1-serving"

if [[ -f /etc/quant-platform/vps1-serving-rust.env ]]; then
  set -a
  # shellcheck disable=SC1091
  source /etc/quant-platform/vps1-serving-rust.env
  set +a
fi

if [[ ! -x "$REPO_ROOT/rust/vps1-serving/target/release/vps1-serving" ]]; then
  echo "Rust gateway binary missing; build with deploy/scripts/build_rust_gateway.sh" >&2
  exit 1
fi

exec "$REPO_ROOT/rust/vps1-serving/target/release/vps1-serving"
