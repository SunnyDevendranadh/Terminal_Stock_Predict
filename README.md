# Quant Research Platform v1.2 (Control-Complete Skeleton)

This repository implements the provided `PLAN.md` as a runnable API service focused on institutional controls.

## Quickstart

```bash
# Clone and set up Python backend
pip install -e .
PYTHONPATH=src python3 -m quant_platform.main --host 127.0.0.1 --port 8080

# Build Rust gateway (optional)
cd rust/vps1-serving && cargo build --release
cd ../../rust/terminal-client && cargo build --release

# Run tests
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

## Project Structure

```
Terminal_Stock_Predict/
├── src/quant_platform/        # Python API backend
│   ├── main.py                # HTTP server entry point
│   ├── service.py             # Core platform service
│   ├── config.py              # Configuration loading
│   ├── audit.py               # Hash-chained audit logging
│   ├── alerts.py              # Alerting channel routing
│   ├── canary.py              # Automated drift detection
│   ├── kill_switch.py         # Emergency circuit breakers
│   ├── market_data.py         # Market data ingestion
│   ├── ml_decision_support.py # ML-backed decision support
│   ├── models.py              # Data models
│   ├── news_feed.py           # News feed ingestion
│   └── news_intel.py          # News intelligence analysis
├── rust/                       # Rust microservices
│   ├── vps1-serving/          # gRPC + mTLS serving layer
│   └── terminal-client/       # ratatui C2 terminal interface
├── proto/                     # Protocol buffer definitions
├── deploy/                    # systemd service files + scripts
├── tests/                     # Python test suite
├── scripts/                   # Build & utility scripts
└── docs/                      # Architecture and deployment docs
```

## What is implemented

- Audit durability controls:
  - Append-only `audit_events` transactional stream (SQLite in this local build)
  - Dual-write batch append to object-store style NDJSON stream
  - Hash-chained audit events with signed event and manifest digests
  - Checkpoint digest persisted separately (`data/checkpoints/off_provider_checkpoint.txt`)
- Alerting channel hardening:
  - Primary channel abstraction (`matrix_webhook_gateway`)
  - Fallback encrypted-email route
- Jurisdiction policy encoding:
  - Primary: Iceland
  - Backup: Switzerland
  - Versioned policy exposed in API headers and compliance endpoint
- Credential rotation safety:
  - Active/standby slot model with overlap window
  - Validation before cutover
  - Rotation events written to immutable audit stream
- Automated canary gates:
  - Mann-Whitney U drift gate
  - Calibration and risk-band shift thresholds
  - Outcome actions: pass / extend / rollback+incident
- Emergency kill switch:
  - Soft kill: replay last-known-good predictions
  - Hard kill: block prediction endpoints
  - RBAC + MFA + signed reason checks
  - Two-person approval to clear hard kill

## Endpoints

- `POST /v1/predict`
- `GET /v1/ops/audit`
- `GET /v1/ops/credentials`
- `POST /v1/ops/credentials/rotate`
- `POST /v1/ops/kill-switch`
- `POST /v1/ops/canary/evaluate`
- `GET /v1/ops/compliance`
- `GET /v1/ops/status`

## Run locally

```bash
PYTHONPATH=src python3 -m quant_platform.main --host 127.0.0.1 --port 8080
```

## Test

```bash
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

## Deployment package

For immediate VPS testing with `systemd`, use the deploy bundle in `deploy/`.

See:

- `docs/deployment.md`

## Rust rewrite

- `rust/vps1-serving`
- `rust/terminal-client`
- `proto/quant_platform.proto`
- `docs/rust-rewrite.md`
- `docs/terminal-client.md`

Current Rust gateway includes:

- gRPC + mTLS serving layer (`tonic`)
- Python-backend forwarding adapter
- Rust-owned Postgres + S3 audit adapters (trait-based)
- server-streaming signal pipeline (`SubscribeSignals`)

Terminal/API milestone client includes:

- ratatui C2 terminal interface with Radar, Tactical, Signal Detail, and Threat Board views
- tonic mTLS client to `vps1-serving` with live stream subscription (+ polling fallback)
- SQLite outage cache + degraded banner mode
