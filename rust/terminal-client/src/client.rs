use std::collections::HashMap;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::Request;

pub mod pb {
    tonic::include_proto!("quant.v1");
}

use pb::prediction_gateway_client::PredictionGatewayClient;
use pb::{
    EventImpactRequest, EventIntelRequest, FocusBundleRequest, MlCalibrationStatusRequest,
    MlDecisionSupportRequest, NewsIntelBoardRequest, OpsStatusRequest, PredictRequest, SignalSubscriptionRequest,
};

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub endpoint: String,
    pub tls_enabled: bool,
    pub tls_domain: String,
    pub ca_cert_path: PathBuf,
    pub client_cert_path: PathBuf,
    pub client_key_path: PathBuf,
    pub refresh_interval: Duration,
    pub actor: String,
    pub model_version: String,
    pub symbols: Vec<String>,
    pub cache_path: PathBuf,
}

impl ClientConfig {
    pub fn from_env() -> Result<Self> {
        let endpoint = env::var("TC_GRPC_ENDPOINT").unwrap_or_else(|_| "https://127.0.0.1:50051".to_string());
        let tls_enabled = env_bool("TC_TLS_ENABLED", true);
        let tls_domain = env::var("TC_TLS_DOMAIN_NAME").unwrap_or_else(|_| "localhost".to_string());

        let ca_cert_path = env_path("TC_TLS_CA_CERT_PATH", "/etc/quant-platform/tls/client-ca.crt");
        let client_cert_path = env_path("TC_TLS_CLIENT_CERT_PATH", "/etc/quant-platform/tls/ops-client.crt");
        let client_key_path = env_path("TC_TLS_CLIENT_KEY_PATH", "/etc/quant-platform/tls/ops-client.key");

        let refresh_interval = Duration::from_secs(env_u64("TC_REFRESH_SECONDS", 30));
        let actor = env::var("TC_ACTOR").unwrap_or_else(|_| "terminal-client".to_string());
        let model_version = env::var("TC_MODEL_VERSION").unwrap_or_else(|_| "current".to_string());

        let symbols = env::var("TC_SYMBOLS")
            .unwrap_or_else(|_| {
                "BTCUSD,ETHUSD,SPY,QQQ,NVDA,TSLA,AAPL,MSFT,GLD,TLT,EURUSD,JPYUSD,CL1,SI1,ES1,NQ1"
                    .to_string()
            })
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        let cache_path = env_path("TC_CACHE_PATH", "data/terminal-client/cache.sqlite3");

        if symbols.is_empty() {
            return Err(anyhow!("TC_SYMBOLS resolved to an empty symbol list"));
        }

        Ok(Self {
            endpoint,
            tls_enabled,
            tls_domain,
            ca_cert_path,
            client_cert_path,
            client_key_path,
            refresh_interval,
            actor,
            model_version,
            symbols,
            cache_path,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureContribution {
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEntry {
    pub symbol: String,
    pub alpha_score: u8,
    pub raw_score: f64,
    pub model_version: String,
    pub model_provenance: String,
    pub feature_contract_status: String,
    pub audit_hash: String,
    pub bucket_1_5d: f64,
    pub bucket_20_60d: f64,
    pub feature_contributions: Vec<FeatureContribution>,
    #[serde(default)]
    pub price: f64,
    #[serde(default)]
    pub price_change_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub headline: String,
    pub source: String,
    pub url: String,
    pub published_at: String,
    pub symbol: String,
    pub sentiment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreatSnapshot {
    pub kill_switch_active: bool,
    pub kill_switch_level: String,
    pub chain_verification_status: String,
    pub audit_rpo_seconds: i32,
    #[serde(default)]
    pub api_quota_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalSnapshot {
    pub captured_at: DateTime<Utc>,
    pub signals: Vec<SignalEntry>,
    pub threat: ThreatSnapshot,
    #[serde(default)]
    pub news: Vec<NewsItem>,
}

// ── Glossary terms (mirroring Python GLOSSARY_MAP) ───────────────────────────

pub static GLOSSARY_TERMS: &[(&str, &str)] = &[
    ("eps",         "Earnings Per Share — profit per share"),
    ("guidance",    "Company's own forecast for future performance"),
    ("downgrade",   "Analyst reduced their rating"),
    ("upgrade",     "Analyst raised their rating"),
    ("layoff",      "Company reducing workforce, often signals cost pressure"),
    ("beat",        "Results exceeded analyst expectations"),
    ("miss",        "Results fell short of expectations"),
    ("rally",       "Rapid price increase over a short period"),
    ("selloff",     "Rapid price decrease driven by high selling volume"),
    ("ipo",         "Initial Public Offering — first time shares sold publicly"),
    ("merger",      "Two companies combining into one entity"),
    ("acquisition", "One company buying another"),
    ("dividend",    "Cash payment from company profits to shareholders"),
    ("default",     "Company failing to repay debt obligations"),
    ("short",       "Bet that a stock will fall in price"),
];

// ── Public focus-bundle types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePoint {
    pub ts: String,
    pub price: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub ts: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsDigestItem {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMarker {
    pub event_id: String,
    pub ts: String,
    pub severity: String,
    pub headline: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceBandPoint {
    pub ts: String,
    pub lower: f64,
    pub upper: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactProbability {
    pub horizon: String,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEvidence {
    pub source: String,
    pub url: String,
    pub published_at: String,
    pub quote: String,
    pub reliability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventClaim {
    pub claim_id: String,
    pub event_id: String,
    pub claim_type: String,
    pub claim_text: String,
    pub verification_status: String,
    pub confidence: f64,
    pub evidence: Vec<EventEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventImpact {
    pub event_id: String,
    pub horizon: String,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
    pub expected_return: f64,
    pub downside_risk: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTimelineEntry {
    pub event_id: String,
    pub timestamp: String,
    pub status: String,
    pub change_note: String,
}

#[derive(Debug, Clone)]
pub struct NewsIntelBoardData {
    pub events: Vec<EventSummary>,
    pub next_cursor: String,
    pub count: i32,
    pub total: i32,
    pub stale: bool,
    pub generated_at: String,
}

#[derive(Debug, Clone)]
pub struct EventIntelData {
    pub event: Option<EventSummary>,
    pub claims: Vec<EventClaim>,
    pub impacts: Vec<EventImpact>,
    pub timeline: Vec<EventTimelineEntry>,
    pub citations: Vec<String>,
    pub what_changed_since_last_update: String,
}

#[derive(Debug, Clone)]
pub struct MlRegimeData {
    pub regime: String,
    pub momentum: f64,
    pub realized_vol_proxy: f64,
    pub liquidity_proxy: f64,
}

#[derive(Debug, Clone)]
pub struct MlHorizonDecisionData {
    pub horizon: String,
    pub expected_return: f64,
    pub downside_risk: f64,
    pub confidence: f64,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
    pub action_band: String,
}

#[derive(Debug, Clone)]
pub struct MlPortfolioRiskAdvisoryData {
    pub exposure_band: String,
    pub concentration_risk: String,
    pub suggested_sizing_band: String,
    pub max_single_position_pct: f64,
    pub stop_review_required: bool,
}

#[derive(Debug, Clone)]
pub struct MlDecisionSupportData {
    pub model_version: String,
    pub model_lineage: String,
    pub feature_contract_status: String,
    pub confidence_floor: f64,
    pub confidence_gated: bool,
    pub calibration_error: f64,
    pub confidence_drift: f64,
    pub regime: MlRegimeData,
    pub horizons: Vec<MlHorizonDecisionData>,
    pub portfolio_risk_advisory: MlPortfolioRiskAdvisoryData,
    pub generated_at: String,
}

#[derive(Debug, Clone)]
pub struct MlCalibrationStatusData {
    pub sample_count: i32,
    pub ece: f64,
    pub brier_score: f64,
    pub hit_rate: f64,
    pub confidence_drift: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct FocusBundleData {
    pub symbol: String,
    pub timeframe: String,
    pub line_series: Vec<PricePoint>,
    pub candles: Vec<Candle>,
    pub digest: Vec<NewsDigestItem>,
    pub trend_label: String,
    pub change_pct: f64,
    pub event_markers: Vec<EventMarker>,
    pub confidence_band: Vec<ConfidenceBandPoint>,
    pub impact_probabilities: Vec<ImpactProbability>,
}

// Private helper types for polling deserialization
#[derive(Deserialize)]
struct MarketDataEntryPoll {
    price: f64,
    change_pct: f64,
}

#[derive(Deserialize)]
struct NewsPollResponse {
    items: Vec<NewsItem>,
}

pub struct GrpcSignalClient {
    inner: PredictionGatewayClient<Channel>,
    actor: String,
    model_version: String,
    symbols: Vec<String>,
    refresh_interval: Duration,
    http_base_url: String,
}

impl GrpcSignalClient {
    pub async fn connect(config: &ClientConfig) -> Result<Self> {
        let mut endpoint = Endpoint::from_shared(config.endpoint.clone())
            .context("invalid TC_GRPC_ENDPOINT")?
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .tcp_keepalive(Some(Duration::from_secs(30)));

        if config.tls_enabled {
            let ca_cert = read_pem_file(&config.ca_cert_path)
                .with_context(|| format!("failed to read CA cert at {}", config.ca_cert_path.display()))?;
            let client_cert = read_pem_file(&config.client_cert_path)
                .with_context(|| format!("failed to read client cert at {}", config.client_cert_path.display()))?;
            let client_key = read_pem_file(&config.client_key_path)
                .with_context(|| format!("failed to read client key at {}", config.client_key_path.display()))?;

            let tls = ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(ca_cert))
                .identity(Identity::from_pem(client_cert, client_key))
                .domain_name(config.tls_domain.clone());

            endpoint = endpoint
                .tls_config(tls)
                .context("failed to apply TLS config")?;
        }

        let channel = endpoint
            .connect()
            .await
            .context("failed connecting to gRPC server")?;

        // Derive http_base_url from the gRPC endpoint.
        // The Python backend always runs on http://127.0.0.1:18080.
        let http_base_url = env::var("TC_HTTP_BASE_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18080".to_string());

        Ok(Self {
            inner: PredictionGatewayClient::new(channel),
            actor: config.actor.clone(),
            model_version: config.model_version.clone(),
            symbols: config.symbols.clone(),
            refresh_interval: config.refresh_interval,
            http_base_url,
        })
    }

    pub async fn subscribe_signals(&mut self) -> Result<tonic::Streaming<pb::SignalSnapshot>> {
        let request = SignalSubscriptionRequest {
            symbols: self.symbols.clone(),
            refresh_seconds: self.refresh_interval.as_secs().clamp(15, 300) as i32,
            actor: self.actor.clone(),
            model_version: self.model_version.clone(),
        };

        let stream = self
            .inner
            .subscribe_signals(Request::new(request))
            .await
            .context("SubscribeSignals failed")?
            .into_inner();

        Ok(stream)
    }

    pub fn map_stream_snapshot(snapshot: pb::SignalSnapshot) -> Result<SignalSnapshot> {
        let captured_at = chrono::DateTime::parse_from_rfc3339(&snapshot.captured_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let signals = snapshot
            .signals
            .into_iter()
            .map(|s| SignalEntry {
                symbol: s.symbol,
                alpha_score: s.alpha_score.clamp(0, 100) as u8,
                raw_score: s.raw_score,
                model_version: s.model_version,
                model_provenance: s.model_provenance,
                feature_contract_status: s.feature_contract_status,
                audit_hash: s.audit_hash,
                bucket_1_5d: s.bucket_1_5d,
                bucket_20_60d: s.bucket_20_60d,
                feature_contributions: s
                    .feature_contributions
                    .into_iter()
                    .map(|f| FeatureContribution {
                        name: f.name,
                        score: f.score,
                    })
                    .collect::<Vec<_>>(),
                price: s.price,
                price_change_pct: s.price_change_pct,
            })
            .collect::<Vec<_>>();

        let threat = snapshot.threat.unwrap_or(pb::ThreatSnapshot {
            kill_switch_active: false,
            kill_switch_level: "unknown".to_string(),
            chain_verification_status: "unknown".to_string(),
            audit_rpo_seconds: 0,
            api_quota_status: format!("STREAM/{:02}s budget mode", 30),
        });

        let news = snapshot
            .news
            .into_iter()
            .map(|n| NewsItem {
                headline: n.headline,
                source: n.source,
                url: n.url,
                published_at: n.published_at,
                symbol: n.symbol,
                sentiment: n.sentiment,
            })
            .collect::<Vec<_>>();

        Ok(SignalSnapshot {
            captured_at,
            signals,
            threat: ThreatSnapshot {
                kill_switch_active: threat.kill_switch_active,
                kill_switch_level: threat.kill_switch_level,
                chain_verification_status: threat.chain_verification_status,
                audit_rpo_seconds: threat.audit_rpo_seconds,
                api_quota_status: threat.api_quota_status,
            },
            news,
        })
    }

    pub async fn fetch_focus_bundle(&mut self, symbol: &str, timeframe: &str) -> Result<FocusBundleData> {
        let req = FocusBundleRequest {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_focus_bundle(Request::new(req))
            .await
            .context("GetFocusBundle RPC failed")?
            .into_inner();

        Ok(FocusBundleData {
            symbol: resp.symbol,
            timeframe: resp.timeframe,
            line_series: resp.line_series.into_iter().map(|p| PricePoint {
                ts: p.ts,
                price: p.price,
                volume: p.volume,
            }).collect(),
            candles: resp.candles.into_iter().map(|c| Candle {
                ts: c.ts,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
            }).collect(),
            digest: resp.digest.into_iter().map(|d| NewsDigestItem {
                headline: d.headline,
                source: d.source,
                url: d.url,
                published_at: d.published_at,
                symbol: d.symbol,
                sentiment: d.sentiment,
                summary: d.summary,
                why_it_matters: d.why_it_matters,
                glossary_terms: d.glossary_terms,
            }).collect(),
            trend_label: resp.trend_label,
            change_pct: resp.change_pct,
            event_markers: resp.event_markers.into_iter().map(|m| EventMarker {
                event_id: m.event_id,
                ts: m.ts,
                severity: m.severity,
                headline: m.headline,
            }).collect(),
            confidence_band: resp.confidence_band.into_iter().map(|c| ConfidenceBandPoint {
                ts: c.ts,
                lower: c.lower,
                upper: c.upper,
            }).collect(),
            impact_probabilities: resp.impact_probabilities.into_iter().map(|p| ImpactProbability {
                horizon: p.horizon,
                prob_up: p.prob_up,
                prob_flat: p.prob_flat,
                prob_down: p.prob_down,
            }).collect(),
        })
    }

    pub async fn fetch_news_intel_board(
        &mut self,
        symbols: &[String],
        severity: &str,
        sentiment: &str,
        window: &str,
        limit: i32,
        cursor: &str,
    ) -> Result<NewsIntelBoardData> {
        let req = NewsIntelBoardRequest {
            symbols: symbols.to_vec(),
            severity: severity.to_string(),
            sentiment: sentiment.to_string(),
            window: window.to_string(),
            limit,
            cursor: cursor.to_string(),
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_news_intel_board(Request::new(req))
            .await
            .context("GetNewsIntelBoard RPC failed")?
            .into_inner();

        Ok(NewsIntelBoardData {
            events: resp.events.into_iter().map(|e| EventSummary {
                event_id: e.event_id,
                symbol: e.symbol,
                title: e.title,
                severity: e.severity,
                novelty_score: e.novelty_score,
                contradiction_score: e.contradiction_score,
                confidence: e.confidence,
                sentiment: e.sentiment,
                published_at: e.published_at,
                article_count: e.article_count,
                stale: e.stale,
                why_it_matters: e.why_it_matters,
            }).collect(),
            next_cursor: resp.next_cursor,
            count: resp.count,
            total: resp.total,
            stale: resp.stale,
            generated_at: resp.generated_at,
        })
    }

    pub async fn fetch_event_intel(&mut self, event_id: &str, symbols: &[String]) -> Result<EventIntelData> {
        let req = EventIntelRequest {
            event_id: event_id.to_string(),
            symbols: symbols.to_vec(),
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_event_intel(Request::new(req))
            .await
            .context("GetEventIntel RPC failed")?
            .into_inner();

        Ok(EventIntelData {
            event: resp.event.map(|e| EventSummary {
                event_id: e.event_id,
                symbol: e.symbol,
                title: e.title,
                severity: e.severity,
                novelty_score: e.novelty_score,
                contradiction_score: e.contradiction_score,
                confidence: e.confidence,
                sentiment: e.sentiment,
                published_at: e.published_at,
                article_count: e.article_count,
                stale: e.stale,
                why_it_matters: e.why_it_matters,
            }),
            claims: resp.claims.into_iter().map(|c| EventClaim {
                claim_id: c.claim_id,
                event_id: c.event_id,
                claim_type: c.claim_type,
                claim_text: c.claim_text,
                verification_status: c.verification_status,
                confidence: c.confidence,
                evidence: c.evidence.into_iter().map(|e| EventEvidence {
                    source: e.source,
                    url: e.url,
                    published_at: e.published_at,
                    quote: e.quote,
                    reliability: e.reliability,
                }).collect(),
            }).collect(),
            impacts: resp.impacts.into_iter().map(|i| EventImpact {
                event_id: i.event_id,
                horizon: i.horizon,
                prob_up: i.prob_up,
                prob_flat: i.prob_flat,
                prob_down: i.prob_down,
                expected_return: i.expected_return,
                downside_risk: i.downside_risk,
                confidence: i.confidence,
            }).collect(),
            timeline: resp.timeline.into_iter().map(|t| EventTimelineEntry {
                event_id: t.event_id,
                timestamp: t.timestamp,
                status: t.status,
                change_note: t.change_note,
            }).collect(),
            citations: resp.citations,
            what_changed_since_last_update: resp.what_changed_since_last_update,
        })
    }

    pub async fn fetch_ml_decision_support(
        &mut self,
        symbol: &str,
        features: &HashMap<String, f64>,
        model_version: &str,
        confidence_floor: f64,
    ) -> Result<MlDecisionSupportData> {
        let req = MlDecisionSupportRequest {
            symbol: symbol.to_string(),
            features: features.clone(),
            model_version: model_version.to_string(),
            confidence_floor,
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_ml_decision_support(Request::new(req))
            .await
            .context("GetMlDecisionSupport RPC failed")?
            .into_inner();

        let regime = resp.regime.unwrap_or(pb::MlRegimeSnapshot {
            regime: "unknown".to_string(),
            momentum: 0.0,
            realized_vol_proxy: 0.0,
            liquidity_proxy: 0.0,
        });
        let advisory = resp.portfolio_risk_advisory.unwrap_or(pb::MlPortfolioRiskAdvisory {
            exposure_band: "unknown".to_string(),
            concentration_risk: "unknown".to_string(),
            suggested_sizing_band: "0.25x-0.50x".to_string(),
            max_single_position_pct: 0.08,
            stop_review_required: false,
        });

        Ok(MlDecisionSupportData {
            model_version: resp.model_version,
            model_lineage: resp.model_lineage,
            feature_contract_status: resp.feature_contract_status,
            confidence_floor: resp.confidence_floor,
            confidence_gated: resp.confidence_gated,
            calibration_error: resp.calibration_error,
            confidence_drift: resp.confidence_drift,
            regime: MlRegimeData {
                regime: regime.regime,
                momentum: regime.momentum,
                realized_vol_proxy: regime.realized_vol_proxy,
                liquidity_proxy: regime.liquidity_proxy,
            },
            horizons: resp
                .horizons
                .into_iter()
                .map(|h| MlHorizonDecisionData {
                    horizon: h.horizon,
                    expected_return: h.expected_return,
                    downside_risk: h.downside_risk,
                    confidence: h.confidence,
                    prob_up: h.prob_up,
                    prob_flat: h.prob_flat,
                    prob_down: h.prob_down,
                    action_band: h.action_band,
                })
                .collect(),
            portfolio_risk_advisory: MlPortfolioRiskAdvisoryData {
                exposure_band: advisory.exposure_band,
                concentration_risk: advisory.concentration_risk,
                suggested_sizing_band: advisory.suggested_sizing_band,
                max_single_position_pct: advisory.max_single_position_pct,
                stop_review_required: advisory.stop_review_required,
            },
            generated_at: resp.generated_at,
        })
    }

    pub async fn fetch_ml_calibration_status(&mut self) -> Result<MlCalibrationStatusData> {
        let req = MlCalibrationStatusRequest {
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_ml_calibration_status(Request::new(req))
            .await
            .context("GetMlCalibrationStatus RPC failed")?
            .into_inner();

        Ok(MlCalibrationStatusData {
            sample_count: resp.sample_count,
            ece: resp.ece,
            brier_score: resp.brier_score,
            hit_rate: resp.hit_rate,
            confidence_drift: resp.confidence_drift,
            updated_at: resp.updated_at,
        })
    }

    #[allow(dead_code)]
    pub async fn fetch_event_impact(&mut self, event_id: &str, symbols: &[String]) -> Result<Vec<EventImpact>> {
        let req = EventImpactRequest {
            event_id: event_id.to_string(),
            symbols: symbols.to_vec(),
            actor: self.actor.clone(),
        };
        let resp = self
            .inner
            .get_event_impact(Request::new(req))
            .await
            .context("GetEventImpact RPC failed")?
            .into_inner();

        Ok(resp.impacts.into_iter().map(|i| EventImpact {
            event_id: i.event_id,
            horizon: i.horizon,
            prob_up: i.prob_up,
            prob_flat: i.prob_flat,
            prob_down: i.prob_down,
            expected_return: i.expected_return,
            downside_risk: i.downside_risk,
            confidence: i.confidence,
        }).collect())
    }

    pub async fn fetch_snapshot_via_polling(&mut self) -> Result<SignalSnapshot> {
        let captured_at = Utc::now();

        let ops = self
            .inner
            .get_ops_status(Request::new(OpsStatusRequest {}))
            .await
            .context("GetOpsStatus failed")?
            .into_inner();

        let mut signals = Vec::with_capacity(self.symbols.len());
        for symbol in &self.symbols {
            let features = synth_features(symbol, captured_at);
            let response = self
                .inner
                .predict(Request::new(PredictRequest {
                    features: features.clone(),
                    model_version: self.model_version.clone(),
                    actor: self.actor.clone(),
                }))
                .await
                .with_context(|| format!("Predict failed for symbol {}", symbol))?
                .into_inner();

            let alpha = (response.score * 100.0).round().clamp(0.0, 100.0) as u8;
            let contributions = build_feature_contributions(&features);
            let bias = (response.score - 0.5) * 100.0;

            signals.push(SignalEntry {
                symbol: symbol.clone(),
                alpha_score: alpha,
                raw_score: response.score,
                model_version: response.model_version,
                model_provenance: format!(
                    "poll:{} -> model:{} -> contract:{}",
                    self.actor, "vps1-serving", response.feature_contract_status
                ),
                feature_contract_status: response.feature_contract_status,
                audit_hash: response.audit_chain_checkpoint,
                bucket_1_5d: bias / 5.0,
                bucket_20_60d: bias / 8.5,
                feature_contributions: contributions,
                price: 0.0,
                price_change_pct: 0.0,
            });
        }

        signals.sort_by(|a, b| b.alpha_score.cmp(&a.alpha_score));

        // Best-effort: fetch market data from the Python HTTP backend.
        let symbols_param = self.symbols.join(",");
        let http_client_opt = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok();

        if let Some(ref http_client) = http_client_opt {
            let market_url = format!("{}/v1/market-data?symbols={}", self.http_base_url, symbols_param);
            if let Ok(resp) = http_client.get(&market_url).send().await {
                if let Ok(map) = resp.json::<HashMap<String, MarketDataEntryPoll>>().await {
                    for (sym, entry) in &map {
                        for signal in signals.iter_mut() {
                            if signal.symbol == *sym {
                                signal.price = entry.price;
                                signal.price_change_pct = entry.change_pct;
                            }
                        }
                    }
                }
            }
        }

        // Best-effort: fetch news from the Python HTTP backend.
        let news = if let Some(ref http_client) = http_client_opt {
            let news_url = format!("{}/v1/news?symbols={}", self.http_base_url, symbols_param);
            if let Ok(resp) = http_client.get(&news_url).send().await {
                resp.json::<NewsPollResponse>().await.map(|r| r.items).unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Ok(SignalSnapshot {
            captured_at,
            signals,
            threat: ThreatSnapshot {
                kill_switch_active: ops.kill_switch_active,
                kill_switch_level: ops.kill_switch_level,
                chain_verification_status: ops.chain_verification_status,
                audit_rpo_seconds: ops.audit_rpo_seconds,
                api_quota_status: format!("POLL/{:02}s budget mode", self.refresh_interval.as_secs()),
            },
            news,
        })
    }
}

fn synth_features(symbol: &str, captured_at: DateTime<Utc>) -> HashMap<String, f64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    symbol.hash(&mut hasher);
    let base = hasher.finish();
    let time_bucket = (captured_at.timestamp() / 30).unsigned_abs();

    let jitter = |salt: u64| -> f64 {
        let mixed = base ^ (time_bucket.wrapping_mul(37 + salt));
        (mixed % 100) as f64 / 100.0
    };

    let mut map = HashMap::with_capacity(5);
    map.insert("open".to_string(), jitter(1));
    map.insert("high".to_string(), jitter(2).max(0.1));
    map.insert("low".to_string(), jitter(3).min(0.9));
    map.insert("close".to_string(), jitter(4));
    map.insert("volume".to_string(), jitter(5));
    map
}

fn build_feature_contributions(features: &HashMap<String, f64>) -> Vec<FeatureContribution> {
    let mut contributions = features
        .iter()
        .map(|(name, value)| FeatureContribution {
            name: name.clone(),
            score: ((value - 0.5) * 2.0 * 100.0).round() / 100.0,
        })
        .collect::<Vec<_>>();

    contributions.sort_by(|a, b| {
        b.score
            .abs()
            .partial_cmp(&a.score.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    contributions
}

fn read_pem_file(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Err(anyhow!("empty PEM file at {}", path.display()));
    }
    Ok(bytes)
}

fn env_path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(env::var(name).unwrap_or_else(|_| default.to_string()))
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(raw) => matches!(raw.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => default,
    }
}
