use std::fs;

use anyhow::{Context, Result};
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tracing::{info, warn};

use vps1_serving::audit::build_audit_writer;
use vps1_serving::backend::HttpBackend;
use vps1_serving::config::GatewayConfig;
use vps1_serving::gateway::PredictionGatewayService;
use vps1_serving::mtls::ClientCertificateInterceptor;
use vps1_serving::pb::prediction_gateway_server::PredictionGatewayServer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = GatewayConfig::from_env()?;
    let addr = config.socket_addr()?;

    let backend = HttpBackend::new(config.backend_base_url.clone())
        .map_err(|err| anyhow::anyhow!("backend init failed: {err}"))?;
    let audit = build_audit_writer(&config.audit)
        .await
        .map_err(|err| anyhow::anyhow!("audit init failed: {err}"))?;
    let gateway = PredictionGatewayService::new(backend, audit, config.backend_base_url.clone());

    let mut builder = Server::builder();
    let require_mtls = config.tls.require_mtls;
    if config.tls.require_mtls {
        let cert_path = config
            .tls
            .server_cert_path
            .as_ref()
            .context("server cert path missing")?;
        let key_path = config
            .tls
            .server_key_path
            .as_ref()
            .context("server key path missing")?;
        let client_ca_path = config
            .tls
            .client_ca_cert_path
            .as_ref()
            .context("client CA cert path missing")?;

        let server_cert = fs::read(cert_path)
            .with_context(|| format!("failed reading server cert at {}", cert_path.display()))?;
        let server_key = fs::read(key_path)
            .with_context(|| format!("failed reading server key at {}", key_path.display()))?;
        let client_ca_cert = fs::read(client_ca_path)
            .with_context(|| format!("failed reading client CA cert at {}", client_ca_path.display()))?;

        let identity = Identity::from_pem(server_cert, server_key);
        let client_ca = Certificate::from_pem(client_ca_cert);

        builder = builder
            .tls_config(
                ServerTlsConfig::new()
                    .identity(identity)
                    .client_ca_root(client_ca),
            )
            .context("failed applying mTLS server config")?;

        info!("mTLS enabled with client CA validation");
    } else {
        warn!("mTLS disabled (RUST_REQUIRE_MTLS=false); do not use in production");
    }

    info!(
        host = %config.host,
        port = %config.port,
        backend_url = %config.backend_base_url,
        "starting vps1-serving gRPC gateway"
        );

    if require_mtls {
        let interceptor = ClientCertificateInterceptor::new(config.tls.client_cn_allowlist.clone());
        let service = PredictionGatewayServer::with_interceptor(gateway, interceptor);
        builder.add_service(service).serve(addr).await.context("gRPC server failed")
    } else {
        let service = PredictionGatewayServer::new(gateway);
        builder.add_service(service).serve(addr).await.context("gRPC server failed")
    }
}
