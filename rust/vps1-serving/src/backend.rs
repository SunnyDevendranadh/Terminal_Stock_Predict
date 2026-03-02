use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct BackendPredictRequest {
    pub features: HashMap<String, f64>,
    pub model_version: String,
    pub actor: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendPredictResponse {
    pub score: f64,
    pub model_version: String,
    pub disclaimer_hash: String,
    pub prediction_signature: String,
    pub jurisdiction_restriction: String,
    pub kill_switch_active: bool,
    pub audit_chain_checkpoint: String,
    pub jurisdiction_primary: String,
    pub jurisdiction_backup: String,
    pub feature_contract_status: String,
}

#[derive(Debug, Clone)]
pub struct BackendOpsStatus {
    pub kill_switch_active: bool,
    pub kill_switch_level: String,
    pub chain_verification_status: String,
    pub audit_rpo_seconds: i32,
}

#[derive(Debug, Clone)]
pub struct BackendKillSwitchRequest {
    pub action: String,
    pub reason: String,
    pub reason_signature: String,
    pub approvers: Vec<String>,
    pub actor: String,
    pub role: String,
    pub mfa_token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendKillSwitchResponse {
    pub level: String,
    pub updated_at: String,
    pub reason: String,
}

#[derive(Debug)]
pub enum BackendError {
    Transport(String),
    Response { status: u16, detail: String },
    Decode(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
            Self::Response { status, detail } => write!(f, "backend response {status}: {detail}"),
            Self::Decode(msg) => write!(f, "decode error: {msg}"),
        }
    }
}

impl std::error::Error for BackendError {}

#[async_trait]
pub trait BackendClient: Send + Sync + 'static {
    async fn predict(&self, req: BackendPredictRequest) -> Result<BackendPredictResponse, BackendError>;
    async fn ops_status(&self) -> Result<BackendOpsStatus, BackendError>;
    async fn set_kill_switch(
        &self,
        req: BackendKillSwitchRequest,
    ) -> Result<BackendKillSwitchResponse, BackendError>;
}

#[derive(Clone)]
pub struct HttpBackend {
    base_url: String,
    client: reqwest::Client,
}

impl HttpBackend {
    pub fn new(base_url: String) -> Result<Self, BackendError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| BackendError::Transport(err.to_string()))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    async fn parse_error(resp: reqwest::Response) -> BackendError {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_else(|_| "backend error".to_string());
        let detail = serde_json::from_str::<BackendErrorBody>(&text)
            .map(|body| body.detail)
            .unwrap_or(text);
        BackendError::Response { status, detail }
    }
}

#[derive(Debug, Deserialize)]
struct BackendErrorBody {
    detail: String,
}

#[derive(Debug, Deserialize)]
struct BackendOpsStatusEnvelope {
    kill_switch_active: bool,
    kill_switch_level: String,
    audit: BackendAuditSummary,
}

#[derive(Debug, Deserialize)]
struct BackendAuditSummary {
    chain_verification_status: String,
    audit_rpo_seconds: i32,
}

#[derive(Debug, Serialize)]
struct PredictPayload {
    features: HashMap<String, f64>,
    model_version: String,
}

#[derive(Debug, Serialize)]
struct KillSwitchPayload {
    action: String,
    reason: String,
    reason_signature: String,
    approvers: Vec<String>,
}

#[async_trait]
impl BackendClient for HttpBackend {
    async fn predict(&self, req: BackendPredictRequest) -> Result<BackendPredictResponse, BackendError> {
        let url = format!("{}/v1/predict", self.base_url);
        let payload = PredictPayload {
            features: req.features,
            model_version: req.model_version,
        };
        let resp = self
            .client
            .post(url)
            .header("x-actor", req.actor)
            .json(&payload)
            .send()
            .await
            .map_err(|err| BackendError::Transport(err.to_string()))?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        resp.json::<BackendPredictResponse>()
            .await
            .map_err(|err| BackendError::Decode(err.to_string()))
    }

    async fn ops_status(&self) -> Result<BackendOpsStatus, BackendError> {
        let url = format!("{}/v1/ops/status", self.base_url);
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| BackendError::Transport(err.to_string()))?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        let envelope = resp
            .json::<BackendOpsStatusEnvelope>()
            .await
            .map_err(|err| BackendError::Decode(err.to_string()))?;

        Ok(BackendOpsStatus {
            kill_switch_active: envelope.kill_switch_active,
            kill_switch_level: envelope.kill_switch_level,
            chain_verification_status: envelope.audit.chain_verification_status,
            audit_rpo_seconds: envelope.audit.audit_rpo_seconds,
        })
    }

    async fn set_kill_switch(
        &self,
        req: BackendKillSwitchRequest,
    ) -> Result<BackendKillSwitchResponse, BackendError> {
        let url = format!("{}/v1/ops/kill-switch", self.base_url);
        let payload = KillSwitchPayload {
            action: req.action,
            reason: req.reason,
            reason_signature: req.reason_signature,
            approvers: req.approvers,
        };
        let resp = self
            .client
            .post(url)
            .header("x-actor", req.actor)
            .header("x-role", req.role)
            .header("x-mfa-token", req.mfa_token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| BackendError::Transport(err.to_string()))?;

        if !resp.status().is_success() {
            return Err(Self::parse_error(resp).await);
        }

        resp.json::<BackendKillSwitchResponse>()
            .await
            .map_err(|err| BackendError::Decode(err.to_string()))
    }
}

// ── Market-data / news / history / digest response types ─────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct PricePointRaw {
    pub ts: String,
    pub price: f64,
    pub volume: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct CandleRaw {
    pub ts: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct NewsDigestItemRaw {
    pub headline: String,
    pub source: String,
    pub url: String,
    pub published_at: String,
    pub symbol: String,
    pub sentiment: String,
    pub summary: String,
    pub why_it_matters: String,
    pub glossary_terms: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct HistoryResponse {
    pub symbol: String,
    pub timeframe: String,
    pub points: Vec<PricePointRaw>,
    pub candles: Vec<CandleRaw>,
    pub meta: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct DigestResponse {
    pub items: Vec<NewsDigestItemRaw>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MarketDataEntry {
    pub price: f64,
    pub change_pct: f64,
    pub features: std::collections::HashMap<String, f64>,
}

/// The Python `/v1/market-data` response is a flat object keyed by symbol.
/// Deserialize it directly as `HashMap<String, MarketDataEntry>`.
pub type MarketDataResponse = std::collections::HashMap<String, MarketDataEntry>;

#[derive(Debug, serde::Deserialize)]
pub struct NewsItemRaw {
    pub headline: String,
    pub source: String,
    pub url: String,
    pub published_at: String,
    pub symbol: String,
    pub sentiment: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct NewsResponse {
    pub items: Vec<NewsItemRaw>,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventEvidenceRaw {
    pub source: String,
    pub url: String,
    pub published_at: String,
    pub quote: String,
    pub reliability: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventClaimRaw {
    pub claim_id: String,
    pub event_id: String,
    pub claim_type: String,
    pub claim_text: String,
    pub verification_status: String,
    pub confidence: f64,
    pub evidence: Vec<EventEvidenceRaw>,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventImpactRaw {
    pub event_id: String,
    pub horizon: String,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
    pub expected_return: f64,
    pub downside_risk: f64,
    pub confidence: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventTimelineEntryRaw {
    pub event_id: String,
    pub timestamp: String,
    pub status: String,
    pub change_note: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventSummaryRaw {
    pub event_id: String,
    pub symbol: String,
    pub title: String,
    pub severity: String,
    pub novelty_score: f64,
    pub contradiction_score: f64,
    pub confidence: f64,
    pub sentiment: String,
    pub published_at: String,
    pub article_count: i32,
    pub stale: bool,
    pub why_it_matters: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct NewsEventsResponse {
    pub items: Vec<EventSummaryRaw>,
    #[serde(default)]
    pub next_cursor: String,
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub total: i32,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventIntelResponseRaw {
    pub event: EventSummaryRaw,
    pub claims: Vec<EventClaimRaw>,
    pub impacts: Vec<EventImpactRaw>,
    pub timeline: Vec<EventTimelineEntryRaw>,
    #[serde(default)]
    pub citations: Vec<String>,
    #[serde(default)]
    pub what_changed_since_last_update: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventImpactListResponse {
    pub event_id: String,
    pub items: Vec<EventImpactRaw>,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventMarkerRaw {
    pub event_id: String,
    pub ts: String,
    pub severity: String,
    pub headline: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfidenceBandPointRaw {
    pub ts: String,
    pub lower: f64,
    pub upper: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct ImpactProbabilityRaw {
    pub horizon: String,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct FocusBundleHttpResponse {
    pub symbol: String,
    pub timeframe: String,
    pub history: HistoryResponse,
    pub digest: Vec<NewsDigestItemRaw>,
    #[serde(default)]
    pub event_markers: Vec<EventMarkerRaw>,
    #[serde(default)]
    pub confidence_band: Vec<ConfidenceBandPointRaw>,
    #[serde(default)]
    pub impact_probabilities: Vec<ImpactProbabilityRaw>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MlRegimeRaw {
    pub regime: String,
    pub momentum: f64,
    pub realized_vol_proxy: f64,
    pub liquidity_proxy: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct MlHorizonDecisionRaw {
    pub horizon: String,
    pub expected_return: f64,
    pub downside_risk: f64,
    pub confidence: f64,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
    pub action_band: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MlPortfolioRiskAdvisoryRaw {
    pub exposure_band: String,
    pub concentration_risk: String,
    pub suggested_sizing_band: String,
    pub max_single_position_pct: f64,
    pub stop_review_required: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct MlDecisionSupportResponseRaw {
    pub model_version: String,
    pub model_lineage: String,
    pub feature_contract_status: String,
    pub confidence_floor: f64,
    pub confidence_gated: bool,
    pub calibration_error: f64,
    pub confidence_drift: f64,
    pub regime: MlRegimeRaw,
    pub horizons: Vec<MlHorizonDecisionRaw>,
    pub portfolio_risk_advisory: MlPortfolioRiskAdvisoryRaw,
    pub generated_at: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct MlCalibrationStatusRaw {
    pub sample_count: i32,
    pub ece: f64,
    pub brier_score: f64,
    pub hit_rate: f64,
    pub confidence_drift: f64,
    pub updated_at: String,
}

// ─────────────────────────────────────────────────────────────────────────────

pub fn map_backend_error(err: BackendError) -> tonic::Status {
    match err {
        BackendError::Transport(message) => tonic::Status::unavailable(message),
        BackendError::Decode(message) => tonic::Status::internal(message),
        BackendError::Response { status, detail } => match StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR) {
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => tonic::Status::invalid_argument(detail),
            StatusCode::FORBIDDEN => tonic::Status::permission_denied(detail),
            StatusCode::UNAUTHORIZED => tonic::Status::unauthenticated(detail),
            StatusCode::NOT_FOUND => tonic::Status::not_found(detail),
            StatusCode::SERVICE_UNAVAILABLE => tonic::Status::unavailable(detail),
            _ => tonic::Status::internal(detail),
        },
    }
}
