# Terminal Client (Local Laptop Runbook)

## Prerequisites (macOS)

```bash
brew install rust protobuf
```

## Build

```bash
cd rust/vps1-serving && cargo build --release
cd ../terminal-client && cargo build --release
```

## Environment

Copy and edit:

- `deploy/env/vps1-serving-rust.env.example`
- `deploy/env/terminal-client.env.example`

For local non-mTLS smoke testing:

- `RUST_REQUIRE_MTLS=false`
- `RUST_AUDIT_ENABLED=false`
- `TC_TLS_ENABLED=false`
- `TC_GRPC_ENDPOINT=http://127.0.0.1:50071`

## End-to-end smoke (headless)

```bash
PYTHONPATH=src python3 -m quant_platform.main --host 127.0.0.1 --port 18080
```

In a second terminal:

```bash
RUST_REQUIRE_MTLS=false RUST_AUDIT_ENABLED=false RUST_GATEWAY_HOST=127.0.0.1 RUST_GATEWAY_PORT=50071 PYTHON_BACKEND_URL=http://127.0.0.1:18080 ./rust/vps1-serving/target/release/vps1-serving
```

In a third terminal:

```bash
TC_TLS_ENABLED=false TC_GRPC_ENDPOINT=http://127.0.0.1:50071 ./rust/terminal-client/target/release/stream_probe
```

Expected output:

```text
ok symbols=... kill_switch=... chain=... captured_at=...
```

## One-command local control panel

Use the new helper script:

```bash
cd /Users/sunny/Terminal_Project_1
./scripts/control_panel.sh start
./scripts/control_panel.sh status
./scripts/control_panel.sh client
```

Other commands:

```bash
./scripts/control_panel.sh doctor
./scripts/control_panel.sh restart
./scripts/control_panel.sh stop
```

## Launch TUI

```bash
TC_TLS_ENABLED=false TC_GRPC_ENDPOINT=http://127.0.0.1:50071 ./rust/terminal-client/target/release/terminal-client
```

Controls:

- `1-9`: switch views
- `Tab`: cycle views
- `Up/Down`: move selection in active table/list
- `f`: open Focus for selected symbol
- `i`: open Intel Board
- `e`: open Event Detail from Intel Board selection
- `p`: open Portfolio Risk (ML-backed)
- `r`: refresh active view (`Focus` also refreshes ML strip)
- `v`: cycle Intel severity filter
- `x`: toggle Intel contradiction filter
- `+/-`: raise/lower Intel confidence floor
- `n` / `b`: next / first Intel page
- `?`: keymap overlay
- `q`: quit

## Focus + ML status

`Focus` now shows a live ML decision-support status strip for the selected ticker:

- model version and gating status
- regime and action band
- confidence and calibration ECE

The strip auto-refreshes when:

- entering Focus (`f`)
- switching focus symbols (`[` and `]`)
- changing timeframe (`t`)
- pressing refresh (`r`) in Focus

## Portfolio Risk view

`Portfolio Risk` is sourced from the ML engine via gRPC (`GetMlDecisionSupport` + `GetMlCalibrationStatus`) and includes:

- model lineage and feature-contract status
- regime/momentum/volatility/liquidity snapshot
- horizon-level probabilities and action bands
- sizing constraints and stop-review flags
- calibration metrics (ECE/Brier/hit-rate/drift)
