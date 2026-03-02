# Deployment Package (VPS1 Immediate Testing)

This package deploys the Python serving layer as a hardened `systemd` unit.

## Included assets

- `deploy/systemd/quant-platform.service`
- `deploy/env/quant-platform.env.example`
- `deploy/scripts/start_server.sh`
- `deploy/scripts/install_service.sh`
- `deploy/scripts/healthcheck.sh`
- `deploy/scripts/smoke_test.sh`
- `deploy/scripts/backup_audit.sh`
- `deploy/scripts/restore_audit.sh`

## One-time host preparation

1. Create service account:

```bash
sudo useradd --system --create-home --shell /usr/sbin/nologin quant
```

2. Ensure required tools:

```bash
sudo apt-get update
sudo apt-get install -y python3 rsync curl
```

## Install

From the repo root:

```bash
./deploy/scripts/install_service.sh
```

This script will:

- sync repo into `/opt/quant-platform`
- create `/etc/quant-platform/quant-platform.env` (first install)
- install and enable `quant-platform.service`
- restart the service

## Configure secrets and policy

Edit:

- `/etc/quant-platform/quant-platform.env`

At minimum replace:

- `MODEL_SIGNING_KEY`
- `KILL_SWITCH_REASON_SECRET`

Then reload:

```bash
sudo systemctl restart quant-platform
```

## Validate

```bash
./deploy/scripts/healthcheck.sh 127.0.0.1 8080
./deploy/scripts/smoke_test.sh 127.0.0.1 8080
sudo systemctl status quant-platform
```

## Backup / restore audit artifacts

Backup:

```bash
sudo ./deploy/scripts/backup_audit.sh /var/lib/quant-platform /var/backups/quant-platform
```

Restore:

```bash
sudo ./deploy/scripts/restore_audit.sh /var/backups/quant-platform/<archive>.tar.gz /var/lib/quant-platform
sudo systemctl restart quant-platform
```

## Notes

- Service writes mutable state under `/var/lib/quant-platform`.
- Service unit uses `ProtectSystem=strict` and `ReadWritePaths=/var/lib/quant-platform`.
- The app remains decision-support only and exposes jurisdiction headers by policy config.
