#!/usr/bin/env bash
set -euo pipefail

REPO_SRC="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
INSTALL_ROOT="/opt/quant-platform"
ENV_DIR="/etc/quant-platform"
STATE_DIR="/var/lib/quant-platform"

sudo mkdir -p "$INSTALL_ROOT" "$ENV_DIR" "$STATE_DIR/audit" "$STATE_DIR/checkpoints"
sudo rsync -a --delete \
  --exclude '.git' \
  --exclude '__pycache__' \
  --exclude '*.pyc' \
  "$REPO_SRC/" "$INSTALL_ROOT/"

if [[ ! -f "$ENV_DIR/quant-platform.env" ]]; then
  sudo cp "$INSTALL_ROOT/deploy/env/quant-platform.env.example" "$ENV_DIR/quant-platform.env"
fi

sudo cp "$INSTALL_ROOT/deploy/systemd/quant-platform.service" /etc/systemd/system/quant-platform.service
sudo chown -R quant:quant "$INSTALL_ROOT" "$STATE_DIR"
sudo chmod +x "$INSTALL_ROOT/deploy/scripts/start_server.sh"

sudo systemctl daemon-reload
sudo systemctl enable quant-platform
sudo systemctl restart quant-platform

echo "Installed quant-platform service."
echo "Edit $ENV_DIR/quant-platform.env and restart with: sudo systemctl restart quant-platform"
