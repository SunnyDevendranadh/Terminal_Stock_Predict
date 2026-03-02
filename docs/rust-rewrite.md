# Rust VPS1 Serving Rewrite

Rust gateway implementation now exists in `rust/vps1-serving` with:

- `tonic` gRPC server (`PredictionGateway`)
- mTLS transport configuration (`ServerTlsConfig` + client CA)
- certificate common-name allowlist validation interceptor
- HTTP backend adapter to current Python serving API
- server-streaming `SubscribeSignals` pipeline
- Rust-owned audit writer with trait-based adapters:
  - Postgres audit event repository
  - S3 object-store manifest/checkpoint archive
- parity tests for `Predict`, `GetOpsStatus`, and `SetKillSwitch` field contracts

## Files

- `rust/vps1-serving/src/main.rs`: gateway bootstrap and TLS wiring
- `rust/vps1-serving/src/gateway.rs`: gRPC service implementation
- `rust/vps1-serving/src/backend.rs`: Python backend adapter and error mapping
- `rust/vps1-serving/src/audit.rs`: Postgres/S3 audit adapters and writer traits
- `rust/vps1-serving/src/mtls.rs`: client certificate CN extraction/validation
- `rust/vps1-serving/tests/parity.rs`: contract parity tests
- `proto/quant_platform.proto`: canonical gRPC contract

## Build and test

From repo root:

```bash
cd rust/vps1-serving
cargo test
cargo build --release
```

## End-to-end pipeline verification

Use `rust/terminal-client/src/bin/stream_probe.rs` for a headless stream check.

Reference runbook:

- `docs/terminal-client.md`

## Runtime configuration

Environment variables:

- `RUST_GATEWAY_HOST`, `RUST_GATEWAY_PORT`
- `PYTHON_BACKEND_URL`
- `RUST_REQUIRE_MTLS` (`true` in production)
- `RUST_TLS_SERVER_CERT_PATH`
- `RUST_TLS_SERVER_KEY_PATH`
- `RUST_TLS_CLIENT_CA_CERT_PATH`
- `RUST_TLS_CLIENT_CN_ALLOWLIST` (comma-separated)
- `RUST_AUDIT_ENABLED`
- `RUST_AUDIT_FLUSH_INTERVAL_SECONDS`
- `RUST_AUDIT_SIGNING_KEY`
- `RUST_AUDIT_POSTGRES_DSN`
- `RUST_AUDIT_S3_BUCKET`
- `RUST_AUDIT_S3_REGION`
- `RUST_AUDIT_S3_ENDPOINT` (optional)
- `RUST_AUDIT_S3_PREFIX`

Template:

- `deploy/env/vps1-serving-rust.env.example`

## Systemd deployment

Artifacts:

- `deploy/systemd/vps1-serving-rust.service`
- `deploy/scripts/build_rust_gateway.sh`
- `deploy/scripts/start_rust_gateway.sh`

Recommended flow on VPS1:

```bash
sudo cp deploy/env/vps1-serving-rust.env.example /etc/quant-platform/vps1-serving-rust.env
# edit /etc/quant-platform/vps1-serving-rust.env for TLS paths and allowlist
./deploy/scripts/build_rust_gateway.sh
sudo cp deploy/systemd/vps1-serving-rust.service /etc/systemd/system/vps1-serving-rust.service
sudo systemctl daemon-reload
sudo systemctl enable --now vps1-serving-rust
```

## Operational notes

- `Predict` and `SetKillSwitch` now fail-closed if Rust audit persistence fails.
- `GetOpsStatus` reports Rust audit chain verification status when audit is enabled.
- Ensure IAM permissions allow `s3:PutObject` on the configured audit prefix.
