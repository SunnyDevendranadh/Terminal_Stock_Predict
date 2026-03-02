use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct TlsConfig {
    pub require_mtls: bool,
    pub server_cert_path: Option<PathBuf>,
    pub server_key_path: Option<PathBuf>,
    pub client_ca_cert_path: Option<PathBuf>,
    pub client_cn_allowlist: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    pub backend_base_url: String,
    pub tls: TlsConfig,
    pub audit: AuditConfig,
}

#[derive(Debug, Clone)]
pub struct AuditConfig {
    pub enabled: bool,
    pub flush_interval_seconds: i64,
    pub signing_key: Option<String>,
    pub postgres_dsn: Option<String>,
    pub s3_bucket: Option<String>,
    pub s3_region: String,
    pub s3_endpoint: Option<String>,
    pub s3_prefix: String,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
        let host = env::var("RUST_GATEWAY_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = env::var("RUST_GATEWAY_PORT")
            .unwrap_or_else(|_| "50051".to_string())
            .parse::<u16>()
            .context("invalid RUST_GATEWAY_PORT")?;
        let backend_base_url = env::var("PYTHON_BACKEND_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18080".to_string());

        let require_mtls = parse_bool_env("RUST_REQUIRE_MTLS", true);
        let server_cert_path = env::var("RUST_TLS_SERVER_CERT_PATH").ok().map(PathBuf::from);
        let server_key_path = env::var("RUST_TLS_SERVER_KEY_PATH").ok().map(PathBuf::from);
        let client_ca_cert_path = env::var("RUST_TLS_CLIENT_CA_CERT_PATH").ok().map(PathBuf::from);
        let client_cn_allowlist = env::var("RUST_TLS_CLIENT_CN_ALLOWLIST")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect::<HashSet<_>>();

        if require_mtls {
            if server_cert_path.is_none() {
                return Err(anyhow!("RUST_TLS_SERVER_CERT_PATH is required when RUST_REQUIRE_MTLS=true"));
            }
            if server_key_path.is_none() {
                return Err(anyhow!("RUST_TLS_SERVER_KEY_PATH is required when RUST_REQUIRE_MTLS=true"));
            }
            if client_ca_cert_path.is_none() {
                return Err(anyhow!("RUST_TLS_CLIENT_CA_CERT_PATH is required when RUST_REQUIRE_MTLS=true"));
            }
        }

        let audit_enabled = parse_bool_env("RUST_AUDIT_ENABLED", false);
        let audit_flush_interval_seconds = env::var("RUST_AUDIT_FLUSH_INTERVAL_SECONDS")
            .unwrap_or_else(|_| "60".to_string())
            .parse::<i64>()
            .context("invalid RUST_AUDIT_FLUSH_INTERVAL_SECONDS")?;
        let audit_signing_key = env::var("RUST_AUDIT_SIGNING_KEY").ok();
        let audit_postgres_dsn = env::var("RUST_AUDIT_POSTGRES_DSN").ok();
        let audit_s3_bucket = env::var("RUST_AUDIT_S3_BUCKET").ok();
        let audit_s3_region = env::var("RUST_AUDIT_S3_REGION").unwrap_or_else(|_| "us-east-1".to_string());
        let audit_s3_endpoint = env::var("RUST_AUDIT_S3_ENDPOINT").ok();
        let audit_s3_prefix = env::var("RUST_AUDIT_S3_PREFIX").unwrap_or_else(|_| "quant-platform/audit".to_string());

        Ok(Self {
            host,
            port,
            backend_base_url,
            tls: TlsConfig {
                require_mtls,
                server_cert_path,
                server_key_path,
                client_ca_cert_path,
                client_cn_allowlist,
            },
            audit: AuditConfig {
                enabled: audit_enabled,
                flush_interval_seconds: audit_flush_interval_seconds,
                signing_key: audit_signing_key,
                postgres_dsn: audit_postgres_dsn,
                s3_bucket: audit_s3_bucket,
                s3_region: audit_s3_region,
                s3_endpoint: audit_s3_endpoint,
                s3_prefix: audit_s3_prefix,
            },
        })
    }

    pub fn socket_addr(&self) -> Result<SocketAddr> {
        format!("{}:{}", self.host, self.port)
            .parse::<SocketAddr>()
            .context("invalid gateway bind address")
    }
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    let raw = env::var(name).unwrap_or_else(|_| default.to_string());
    matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}
