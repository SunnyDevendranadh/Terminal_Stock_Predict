#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT/rust/vps1-serving"

cargo build --release

echo "Built binary at $REPO_ROOT/rust/vps1-serving/target/release/vps1-serving"
