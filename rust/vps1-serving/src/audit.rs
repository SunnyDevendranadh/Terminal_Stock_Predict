use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use chrono::{Datelike, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

use crate::config::AuditConfig;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
pub enum AuditError {
    Config(String),
    Postgres(String),
    ObjectStore(String),
    Serialization(String),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "config error: {msg}"),
            Self::Postgres(msg) => write!(f, "postgres error: {msg}"),
            Self::ObjectStore(msg) => write!(f, "object store error: {msg}"),
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
        }
    }
}

impl std::error::Error for AuditError {}

#[derive(Debug, Clone)]
pub struct PersistedAuditEvent {
    pub event_type: String,
    pub actor: String,
    pub payload: Value,
    pub created_at_iso: String,
    pub event_hash: String,
    pub prev_hash: String,
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct AuditStatus {
    pub chain_verification_status: String,
    pub audit_rpo_seconds: i32,
    pub last_hash: String,
}

#[derive(Debug, Clone)]
pub struct ChainEventRow {
    pub event_type: String,
    pub actor: String,
    pub payload: Value,
    pub created_at_iso: String,
    pub event_hash: String,
    pub prev_hash: String,
    pub signature: String,
}

#[async_trait]
pub trait AuditRepository: Send + Sync + 'static {
    async fn init_schema(&self) -> Result<(), AuditError>;
    async fn last_event_hash(&self) -> Result<Option<String>, AuditError>;
    async fn insert_event(&self, event: &PersistedAuditEvent) -> Result<(), AuditError>;
    async fn chain_rows(&self) -> Result<Vec<ChainEventRow>, AuditError>;
}

#[async_trait]
pub trait AuditArchiveStore: Send + Sync + 'static {
    async fn put_json(&self, key: &str, payload: &Value) -> Result<(), AuditError>;
}

#[async_trait]
pub trait AuditWriter: Send + Sync + 'static {
    async fn write_event(&self, event_type: &str, actor: &str, payload: Value) -> Result<String, AuditError>;
    async fn status(&self) -> Result<AuditStatus, AuditError>;
}

#[derive(Clone)]
pub struct NoopAuditWriter {
    audit_rpo_seconds: i32,
}

impl NoopAuditWriter {
    pub fn new(audit_rpo_seconds: i32) -> Self {
        Self { audit_rpo_seconds }
    }
}

#[async_trait]
impl AuditWriter for NoopAuditWriter {
    async fn write_event(&self, _event_type: &str, _actor: &str, _payload: Value) -> Result<String, AuditError> {
        Ok("audit-disabled".to_string())
    }

    async fn status(&self) -> Result<AuditStatus, AuditError> {
        Ok(AuditStatus {
            chain_verification_status: "disabled".to_string(),
            audit_rpo_seconds: self.audit_rpo_seconds,
            last_hash: "GENESIS".to_string(),
        })
    }
}

pub struct PostgresAuditRepository {
    client: tokio_postgres::Client,
}

impl PostgresAuditRepository {
    pub async fn connect(dsn: &str) -> Result<Self, AuditError> {
        let (client, connection) = tokio_postgres::connect(dsn, NoTls)
            .await
            .map_err(|err| AuditError::Postgres(err.to_string()))?;

        tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::error!(error = %err, "postgres connection task ended");
            }
        });

        Ok(Self { client })
    }
}

#[async_trait]
impl AuditRepository for PostgresAuditRepository {
    async fn init_schema(&self) -> Result<(), AuditError> {
        self.client
            .batch_execute(
                "
                CREATE TABLE IF NOT EXISTS audit_events (
                    id BIGSERIAL PRIMARY KEY,
                    event_type TEXT NOT NULL,
                    actor TEXT NOT NULL,
                    payload JSONB NOT NULL,
                    created_at_iso TEXT NOT NULL,
                    event_hash TEXT NOT NULL UNIQUE,
                    prev_hash TEXT NOT NULL,
                    signature TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS audit_events_created_idx ON audit_events(id);
                ",
            )
            .await
            .map_err(|err| AuditError::Postgres(err.to_string()))
    }

    async fn last_event_hash(&self) -> Result<Option<String>, AuditError> {
        let row = self
            .client
            .query_opt(
                "SELECT event_hash FROM audit_events ORDER BY id DESC LIMIT 1",
                &[],
            )
            .await
            .map_err(|err| AuditError::Postgres(err.to_string()))?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }

    async fn insert_event(&self, event: &PersistedAuditEvent) -> Result<(), AuditError> {
        self.client
            .execute(
                "
                INSERT INTO audit_events (
                    event_type,
                    actor,
                    payload,
                    created_at_iso,
                    event_hash,
                    prev_hash,
                    signature
                ) VALUES ($1, $2, $3, $4, $5, $6, $7)
                ",
                &[
                    &event.event_type,
                    &event.actor,
                    &event.payload,
                    &event.created_at_iso,
                    &event.event_hash,
                    &event.prev_hash,
                    &event.signature,
                ],
            )
            .await
            .map_err(|err| AuditError::Postgres(err.to_string()))?;
        Ok(())
    }

    async fn chain_rows(&self) -> Result<Vec<ChainEventRow>, AuditError> {
        let rows = self
            .client
            .query(
                "
                SELECT
                    event_type,
                    actor,
                    payload,
                    created_at_iso,
                    event_hash,
                    prev_hash,
                    signature
                FROM audit_events
                ORDER BY id ASC
                ",
                &[],
            )
            .await
            .map_err(|err| AuditError::Postgres(err.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| ChainEventRow {
                event_type: row.get("event_type"),
                actor: row.get("actor"),
                payload: row.get("payload"),
                created_at_iso: row.get("created_at_iso"),
                event_hash: row.get("event_hash"),
                prev_hash: row.get("prev_hash"),
                signature: row.get("signature"),
            })
            .collect())
    }
}

#[derive(Clone)]
pub struct S3AuditArchive {
    client: aws_sdk_s3::Client,
    bucket: String,
    prefix: String,
}

impl S3AuditArchive {
    pub async fn new(
        bucket: String,
        region: String,
        endpoint: Option<String>,
        prefix: String,
    ) -> Result<Self, AuditError> {
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region))
            .load()
            .await;

        let mut builder = aws_sdk_s3::config::Builder::from(&shared_config);
        if let Some(endpoint_url) = endpoint {
            builder = builder.endpoint_url(endpoint_url);
        }

        let client = aws_sdk_s3::Client::from_conf(builder.build());
        Ok(Self {
            client,
            bucket,
            prefix: prefix.trim_matches('/').to_string(),
        })
    }

    fn full_key(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.prefix, key.trim_start_matches('/'))
        }
    }
}

#[async_trait]
impl AuditArchiveStore for S3AuditArchive {
    async fn put_json(&self, key: &str, payload: &Value) -> Result<(), AuditError> {
        let bytes = serde_json::to_vec(payload)
            .map_err(|err| AuditError::Serialization(err.to_string()))?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .content_type("application/json")
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|err| AuditError::ObjectStore(err.to_string()))?;

        Ok(())
    }
}

#[derive(Debug)]
struct AuditState {
    last_hash: String,
    buffered: Vec<PersistedAuditEvent>,
    last_flush_unix: i64,
}

pub struct PostgresS3AuditWriter<R: AuditRepository, S: AuditArchiveStore> {
    repo: R,
    archive: S,
    signing_key: Vec<u8>,
    flush_interval_seconds: i64,
    state: Mutex<AuditState>,
}

impl<R: AuditRepository, S: AuditArchiveStore> PostgresS3AuditWriter<R, S> {
    pub async fn new(
        repo: R,
        archive: S,
        signing_key: String,
        flush_interval_seconds: i64,
    ) -> Result<Self, AuditError> {
        repo.init_schema().await?;
        let last_hash = repo
            .last_event_hash()
            .await?
            .unwrap_or_else(|| "GENESIS".to_string());

        Ok(Self {
            repo,
            archive,
            signing_key: signing_key.into_bytes(),
            flush_interval_seconds,
            state: Mutex::new(AuditState {
                last_hash,
                buffered: Vec::new(),
                last_flush_unix: Utc::now().timestamp(),
            }),
        })
    }

    async fn flush_buffer_locked(&self, state: &mut AuditState) -> Result<(), AuditError> {
        if state.buffered.is_empty() {
            return Ok(());
        }

        let now = Utc::now();
        let manifest_events = state
            .buffered
            .iter()
            .map(|event| {
                json!({
                    "event_type": event.event_type,
                    "actor": event.actor,
                    "payload": event.payload,
                    "created_at_iso": event.created_at_iso,
                    "event_hash": event.event_hash,
                    "prev_hash": event.prev_hash,
                    "signature": event.signature,
                })
            })
            .collect::<Vec<_>>();

        let start_hash = state
            .buffered
            .first()
            .map(|e| e.prev_hash.clone())
            .unwrap_or_else(|| "GENESIS".to_string());
        let end_hash = state
            .buffered
            .last()
            .map(|e| e.event_hash.clone())
            .unwrap_or_else(|| "GENESIS".to_string());
        let aggregate = sha256_hex(&state.buffered.iter().map(|e| e.event_hash.clone()).collect::<Vec<_>>().join("|"));

        let mut manifest = json!({
            "manifest_created_at": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            "start_hash": start_hash,
            "end_hash": end_hash,
            "count": state.buffered.len(),
            "aggregate_hash": aggregate,
            "events": manifest_events,
        });

        let manifest_sig = sign_json(&self.signing_key, &manifest)?;
        manifest["manifest_signature"] = Value::String(manifest_sig);

        let hash_prefix = &end_hash[0..std::cmp::min(12, end_hash.len())];
        let manifest_key = format!(
            "manifests/{}/{:02}/{:02}/manifest-{}-{}.json",
            now.year(),
            now.month(),
            now.day(),
            now.format("%Y%m%dT%H%M%S%.3fZ"),
            hash_prefix,
        );

        self.archive.put_json(&manifest_key, &manifest).await?;

        let mut checkpoint = json!({
            "created_at": now.to_rfc3339_opts(SecondsFormat::Millis, true),
            "checkpoint_hash": end_hash,
            "count": state.buffered.len(),
        });
        let checkpoint_sig = sign_json(&self.signing_key, &checkpoint)?;
        checkpoint["signature"] = Value::String(checkpoint_sig);

        self.archive.put_json("checkpoints/latest.json", &checkpoint).await?;
        self.archive
            .put_json(
                &format!("checkpoints/{}.json", now.format("%Y%m%dT%H%M%S%.3fZ")),
                &checkpoint,
            )
            .await?;

        state.buffered.clear();
        state.last_flush_unix = now.timestamp();

        Ok(())
    }

    fn verify_chain_rows(&self, rows: &[ChainEventRow]) -> Result<bool, AuditError> {
        let mut expected_prev = "GENESIS".to_string();
        for row in rows {
            if row.prev_hash != expected_prev {
                return Ok(false);
            }

            let payload_text = serde_json::to_string(&row.payload)
                .map_err(|err| AuditError::Serialization(err.to_string()))?;
            let canonical = format!(
                "{}|{}|{}|{}|{}",
                row.event_type, row.actor, row.created_at_iso, payload_text, row.prev_hash
            );
            let expected_hash = sha256_hex(&canonical);
            if row.event_hash != expected_hash {
                return Ok(false);
            }

            let expected_sig = hmac_sha256_hex(&self.signing_key, &row.event_hash)?;
            if row.signature != expected_sig {
                return Ok(false);
            }

            expected_prev = row.event_hash.clone();
        }
        Ok(true)
    }
}

#[async_trait]
impl<R: AuditRepository, S: AuditArchiveStore> AuditWriter for PostgresS3AuditWriter<R, S> {
    async fn write_event(&self, event_type: &str, actor: &str, payload: Value) -> Result<String, AuditError> {
        let mut state = self.state.lock().await;
        let created = Utc::now();
        let created_iso = created.to_rfc3339_opts(SecondsFormat::Millis, true);
        let payload_text = serde_json::to_string(&payload)
            .map_err(|err| AuditError::Serialization(err.to_string()))?;

        let canonical = format!(
            "{}|{}|{}|{}|{}",
            event_type, actor, created_iso, payload_text, state.last_hash
        );
        let event_hash = sha256_hex(&canonical);
        let signature = hmac_sha256_hex(&self.signing_key, &event_hash)?;

        let event = PersistedAuditEvent {
            event_type: event_type.to_string(),
            actor: actor.to_string(),
            payload,
            created_at_iso: created_iso,
            event_hash: event_hash.clone(),
            prev_hash: state.last_hash.clone(),
            signature,
        };

        self.repo.insert_event(&event).await?;
        state.last_hash = event_hash.clone();
        state.buffered.push(event);

        if created.timestamp() - state.last_flush_unix >= self.flush_interval_seconds {
            self.flush_buffer_locked(&mut state).await?;
        }

        Ok(event_hash)
    }

    async fn status(&self) -> Result<AuditStatus, AuditError> {
        let rows = self.repo.chain_rows().await?;
        let valid = self.verify_chain_rows(&rows)?;
        let last_hash = self
            .repo
            .last_event_hash()
            .await?
            .unwrap_or_else(|| "GENESIS".to_string());
        Ok(AuditStatus {
            chain_verification_status: if valid { "valid".to_string() } else { "invalid".to_string() },
            audit_rpo_seconds: self.flush_interval_seconds as i32,
            last_hash,
        })
    }
}

pub async fn build_audit_writer(config: &AuditConfig) -> Result<Arc<dyn AuditWriter>, AuditError> {
    if !config.enabled {
        return Ok(Arc::new(NoopAuditWriter::new(config.flush_interval_seconds as i32)));
    }

    let postgres_dsn = config
        .postgres_dsn
        .as_ref()
        .ok_or_else(|| AuditError::Config("RUST_AUDIT_POSTGRES_DSN is required when audit is enabled".to_string()))?;
    let s3_bucket = config
        .s3_bucket
        .as_ref()
        .ok_or_else(|| AuditError::Config("RUST_AUDIT_S3_BUCKET is required when audit is enabled".to_string()))?;
    let signing_key = config
        .signing_key
        .as_ref()
        .ok_or_else(|| AuditError::Config("RUST_AUDIT_SIGNING_KEY is required when audit is enabled".to_string()))?;

    let repo = PostgresAuditRepository::connect(postgres_dsn).await?;
    let archive = S3AuditArchive::new(
        s3_bucket.clone(),
        config.s3_region.clone(),
        config.s3_endpoint.clone(),
        config.s3_prefix.clone(),
    )
    .await?;

    let writer = PostgresS3AuditWriter::new(repo, archive, signing_key.clone(), config.flush_interval_seconds)
        .await?;
    Ok(Arc::new(writer))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn hmac_sha256_hex(key: &[u8], value: &str) -> Result<String, AuditError> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|err| AuditError::Serialization(err.to_string()))?;
    mac.update(value.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn sign_json(key: &[u8], value: &Value) -> Result<String, AuditError> {
    let encoded = serde_json::to_string(value)
        .map_err(|err| AuditError::Serialization(err.to_string()))?;
    hmac_sha256_hex(key, &encoded)
}

#[cfg(test)]
mod tests {
    use super::{hmac_sha256_hex, sha256_hex};

    #[test]
    fn sha256_hash_is_deterministic() {
        let a = sha256_hex("audit-event");
        let b = sha256_hex("audit-event");
        assert_eq!(a, b);
    }

    #[test]
    fn hmac_signature_is_deterministic() {
        let key = b"test-key";
        let a = hmac_sha256_hex(key, "payload").expect("hmac");
        let b = hmac_sha256_hex(key, "payload").expect("hmac");
        assert_eq!(a, b);
    }
}
