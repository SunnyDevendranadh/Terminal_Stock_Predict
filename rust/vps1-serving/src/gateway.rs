use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::audit::{AuditError, AuditWriter};
use crate::backend::{
    map_backend_error, BackendClient, BackendKillSwitchRequest, BackendOpsStatus, BackendPredictRequest,
    DigestResponse, EventImpactListResponse, EventImpactRaw, EventIntelResponseRaw, EventSummaryRaw,
    FocusBundleHttpResponse, HistoryResponse, MarketDataEntry, MarketDataResponse, MlCalibrationStatusRaw,
    MlDecisionSupportResponseRaw, NewsEventsResponse, NewsItemRaw, NewsResponse,
};
use crate::pb::prediction_gateway_server::PredictionGateway;
use crate::pb::{
    Candle, ConfidenceBandPoint, EventAlert, EventAlertSubscriptionRequest, EventClaim, EventEvidence,
    EventImpact, EventImpactRequest, EventImpactResponse, EventIntelRequest, EventIntelResponse, EventMarker,
    EventSummary, EventTimelineEntry, FeatureContribution, FocusBundleRequest, FocusBundleResponse,
    ImpactProbability, KillSwitchRequest, KillSwitchResponse, MlCalibrationStatusRequest,
    MlCalibrationStatusResponse, MlDecisionSupportRequest, MlDecisionSupportResponse, MlHorizonDecision,
    MlPortfolioRiskAdvisory, MlRegimeSnapshot, NewsDigestItem as PbNewsDigestItem, NewsIntelBoardRequest,
    NewsIntelBoardResponse, NewsItem, OpsStatusRequest, OpsStatusResponse, PredictRequest, PredictResponse,
    PricePoint, SignalPoint, SignalSnapshot, SignalSubscriptionRequest, ThreatSnapshot,
};

type SignalStream = Pin<Box<dyn Stream<Item = Result<SignalSnapshot, Status>> + Send + 'static>>;
type EventAlertStream = Pin<Box<dyn Stream<Item = Result<EventAlert, Status>> + Send + 'static>>;

pub struct PredictionGatewayService<B: BackendClient> {
    backend: Arc<B>,
    audit: Arc<dyn AuditWriter>,
    http_client: reqwest::Client,
    backend_url: String,
}

impl<B: BackendClient> Clone for PredictionGatewayService<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            audit: Arc::clone(&self.audit),
            http_client: self.http_client.clone(),
            backend_url: self.backend_url.clone(),
        }
    }
}

impl<B: BackendClient> PredictionGatewayService<B> {
    pub fn new(backend: B, audit: Arc<dyn AuditWriter>, backend_url: String) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            backend: Arc::new(backend),
            audit,
            http_client,
            backend_url: backend_url.trim_end_matches('/').to_string(),
        }
    }

    fn map_ops_status(status: BackendOpsStatus) -> OpsStatusResponse {
        OpsStatusResponse {
            kill_switch_active: status.kill_switch_active,
            kill_switch_level: status.kill_switch_level,
            chain_verification_status: status.chain_verification_status,
            audit_rpo_seconds: status.audit_rpo_seconds,
            api_quota_status: String::new(),
        }
    }

    fn severity_score(severity: &str) -> i32 {
        match severity {
            "critical" => 4,
            "high" => 3,
            "medium" => 2,
            "low" => 1,
            _ => 0,
        }
    }

    fn map_event_summary(raw: EventSummaryRaw) -> EventSummary {
        EventSummary {
            event_id: raw.event_id,
            symbol: raw.symbol,
            title: raw.title,
            severity: raw.severity,
            novelty_score: raw.novelty_score,
            contradiction_score: raw.contradiction_score,
            confidence: raw.confidence,
            sentiment: raw.sentiment,
            published_at: raw.published_at,
            article_count: raw.article_count,
            stale: raw.stale,
            why_it_matters: raw.why_it_matters,
        }
    }

    fn map_event_impact(raw: EventImpactRaw) -> EventImpact {
        EventImpact {
            event_id: raw.event_id,
            horizon: raw.horizon,
            prob_up: raw.prob_up,
            prob_flat: raw.prob_flat,
            prob_down: raw.prob_down,
            expected_return: raw.expected_return,
            downside_risk: raw.downside_risk,
            confidence: raw.confidence,
        }
    }

    fn synth_features_for_symbol(symbol: &str) -> HashMap<String, f64> {
        synth_features(symbol, Utc::now().timestamp())
    }

    async fn record_audit(&self, event_type: &str, actor: &str, payload: serde_json::Value) -> Result<String, Status> {
        self.audit
            .write_event(event_type, actor, payload)
            .await
            .map_err(map_audit_error)
    }

    async fn build_signal_snapshot(
        &self,
        symbols: &[String],
        refresh_seconds: i32,
        actor: &str,
        model_version: &str,
    ) -> Result<SignalSnapshot, Status> {
        let captured_at = Utc::now();
        let ops = self
            .backend
            .ops_status()
            .await
            .map_err(map_backend_error)?;

        // Fetch real market data; fall back to synth on any error.
        let market_data: HashMap<String, MarketDataEntry> =
            fetch_market_data(&self.http_client, &self.backend_url, symbols)
                .await
                .unwrap_or_default();

        // Fetch news items; on error return empty list.
        let news_raw: Vec<NewsItemRaw> =
            fetch_news_items(&self.http_client, &self.backend_url, symbols, 50)
                .await
                .unwrap_or_default();

        let mut signals = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            // Use real features when available, otherwise fall back to synth.
            let features = match market_data.get(symbol) {
                Some(entry) if !entry.features.is_empty() => entry.features.clone(),
                _ => synth_features(symbol, captured_at.timestamp()),
            };

            let response = self
                .backend
                .predict(BackendPredictRequest {
                    features: features.clone(),
                    model_version: model_version.to_string(),
                    actor: actor.to_string(),
                })
                .await
                .map_err(map_backend_error)?;

            let alpha_score = (response.score * 100.0).round().clamp(0.0, 100.0) as i32;
            let bias = (response.score - 0.5) * 100.0;

            let (price, price_change_pct) = match market_data.get(symbol) {
                Some(entry) => (entry.price, entry.change_pct),
                None => (0.0, 0.0),
            };

            signals.push(SignalPoint {
                symbol: symbol.clone(),
                alpha_score,
                raw_score: response.score,
                model_version: response.model_version,
                model_provenance: format!(
                    "gateway:{} -> model:vps1-serving -> contract:{}",
                    actor, response.feature_contract_status
                ),
                feature_contract_status: response.feature_contract_status,
                audit_hash: response.audit_chain_checkpoint,
                bucket_1_5d: bias / 5.0,
                bucket_20_60d: bias / 8.5,
                feature_contributions: feature_contributions(&features),
                price,
                price_change_pct,
            });
        }

        signals.sort_by(|a, b| b.alpha_score.cmp(&a.alpha_score));

        let stream_audit_hash = self
            .record_audit(
                "signal_stream_snapshot",
                actor,
                json!({
                    "symbol_count": symbols.len(),
                    "top_symbol": signals.first().map(|s| s.symbol.clone()).unwrap_or_default(),
                    "refresh_seconds": refresh_seconds,
                }),
            )
            .await?;

        for signal in &mut signals {
            if !stream_audit_hash.is_empty() && stream_audit_hash != "audit-disabled" {
                signal.audit_hash = stream_audit_hash.clone();
            }
        }

        let audit_status = self.audit.status().await.map_err(map_audit_error)?;
        let use_rust_audit = audit_status.chain_verification_status != "disabled";

        // Map raw news items to proto NewsItem.
        let news: Vec<NewsItem> = news_raw
            .into_iter()
            .map(|n| NewsItem {
                headline: n.headline,
                source: n.source,
                url: n.url,
                published_at: n.published_at,
                symbol: n.symbol,
                sentiment: n.sentiment,
            })
            .collect();

        Ok(SignalSnapshot {
            captured_at: captured_at.to_rfc3339(),
            signals,
            threat: Some(ThreatSnapshot {
                kill_switch_active: ops.kill_switch_active,
                kill_switch_level: ops.kill_switch_level,
                chain_verification_status: if use_rust_audit {
                    audit_status.chain_verification_status
                } else {
                    ops.chain_verification_status
                },
                audit_rpo_seconds: if use_rust_audit {
                    audit_status.audit_rpo_seconds
                } else {
                    ops.audit_rpo_seconds
                },
                api_quota_status: format!("STREAM/{:02}s budget mode", refresh_seconds),
            }),
            news,
        })
    }
}

#[tonic::async_trait]
impl<B: BackendClient> PredictionGateway for PredictionGatewayService<B> {
    type SubscribeSignalsStream = SignalStream;
    type SubscribeEventAlertsStream = EventAlertStream;

    async fn get_focus_bundle(
        &self,
        request: Request<FocusBundleRequest>,
    ) -> Result<Response<FocusBundleResponse>, Status> {
        let req = request.into_inner();
        let symbol = req.symbol.clone();
        let timeframe = if req.timeframe.is_empty() { "5d".to_string() } else { req.timeframe.clone() };
        if let Ok(bundle) = fetch_focus_bundle_http(&self.http_client, &self.backend_url, &symbol, &timeframe).await {
            let trend_label = bundle.history.meta.get("trend_label")
                .and_then(|v| v.as_str())
                .unwrap_or("Range")
                .to_string();
            let change_pct = bundle.history.meta.get("change_pct")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let line_series: Vec<PricePoint> = bundle.history.points.into_iter().map(|p| PricePoint {
                ts: p.ts,
                price: p.price,
                volume: p.volume,
            }).collect();

            let candles: Vec<Candle> = bundle.history.candles.into_iter().map(|c| Candle {
                ts: c.ts,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
                volume: c.volume,
            }).collect();

            let digest: Vec<PbNewsDigestItem> = bundle.digest.into_iter().map(|d| PbNewsDigestItem {
                headline: d.headline,
                source: d.source,
                url: d.url,
                published_at: d.published_at,
                symbol: d.symbol,
                sentiment: d.sentiment,
                summary: d.summary,
                why_it_matters: d.why_it_matters,
                glossary_terms: d.glossary_terms,
            }).collect();

            let event_markers = bundle.event_markers.into_iter().map(|m| EventMarker {
                event_id: m.event_id,
                ts: m.ts,
                severity: m.severity,
                headline: m.headline,
            }).collect();
            let confidence_band = bundle.confidence_band.into_iter().map(|c| ConfidenceBandPoint {
                ts: c.ts,
                lower: c.lower,
                upper: c.upper,
            }).collect();
            let impact_probabilities = bundle.impact_probabilities.into_iter().map(|p| ImpactProbability {
                horizon: p.horizon,
                prob_up: p.prob_up,
                prob_flat: p.prob_flat,
                prob_down: p.prob_down,
            }).collect();

            return Ok(Response::new(FocusBundleResponse {
                symbol: bundle.symbol,
                timeframe: bundle.timeframe,
                line_series,
                candles,
                digest,
                trend_label,
                change_pct,
                captured_at: Utc::now().to_rfc3339(),
                event_markers,
                confidence_band,
                impact_probabilities,
            }));
        }

        // Backward-compatible fallback if /v1/focus/bundle is unavailable.
        let (hist_result, digest_result) = tokio::join!(
            fetch_history(&self.http_client, &self.backend_url, &symbol, &timeframe),
            fetch_digest(&self.http_client, &self.backend_url, &symbol),
        );

        let hist = hist_result.unwrap_or(HistoryResponse {
            symbol: symbol.clone(),
            timeframe: timeframe.clone(),
            points: vec![],
            candles: vec![],
            meta: serde_json::Value::Object(Default::default()),
        });

        let digest_items = digest_result.map(|d| d.items).unwrap_or_default();
        let trend_label = hist.meta.get("trend_label").and_then(|v| v.as_str()).unwrap_or("Range").to_string();
        let change_pct = hist.meta.get("change_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let line_series: Vec<PricePoint> = hist.points.into_iter().map(|p| PricePoint {
            ts: p.ts,
            price: p.price,
            volume: p.volume,
        }).collect();
        let candles: Vec<Candle> = hist.candles.into_iter().map(|c| Candle {
            ts: c.ts,
            open: c.open,
            high: c.high,
            low: c.low,
            close: c.close,
            volume: c.volume,
        }).collect();
        let digest: Vec<PbNewsDigestItem> = digest_items.into_iter().map(|d| PbNewsDigestItem {
            headline: d.headline,
            source: d.source,
            url: d.url,
            published_at: d.published_at,
            symbol: d.symbol,
            sentiment: d.sentiment,
            summary: d.summary,
            why_it_matters: d.why_it_matters,
            glossary_terms: d.glossary_terms,
        }).collect();

        Ok(Response::new(FocusBundleResponse {
            symbol,
            timeframe,
            line_series,
            candles,
            digest,
            trend_label,
            change_pct,
            captured_at: Utc::now().to_rfc3339(),
            event_markers: vec![],
            confidence_band: vec![],
            impact_probabilities: vec![],
        }))
    }

    async fn get_news_intel_board(
        &self,
        request: Request<NewsIntelBoardRequest>,
    ) -> Result<Response<NewsIntelBoardResponse>, Status> {
        let req = request.into_inner();
        let actor = if req.actor.trim().is_empty() {
            "system".to_string()
        } else {
            req.actor.clone()
        };
        let limit = req.limit.clamp(1, 100);
        let data = fetch_news_events(
            &self.http_client,
            &self.backend_url,
            &req.symbols,
            &req.severity,
            &req.sentiment,
            if req.window.is_empty() { "24h" } else { &req.window },
            limit,
            &req.cursor,
        )
        .await
        .map_err(|err| Status::unavailable(format!("news intel board fetch failed: {err}")))?;

        let events = data
            .items
            .into_iter()
            .map(Self::map_event_summary)
            .collect::<Vec<_>>();

        self.record_audit(
            "news_intel_board_served",
            &actor,
            json!({
                "count": events.len(),
                "severity": req.severity,
                "sentiment": req.sentiment,
                "window": req.window,
                "cursor": req.cursor,
            }),
        )
        .await?;

        Ok(Response::new(NewsIntelBoardResponse {
            events,
            next_cursor: data.next_cursor,
            count: data.count,
            total: data.total,
            stale: false,
            generated_at: Utc::now().to_rfc3339(),
        }))
    }

    async fn get_event_intel(
        &self,
        request: Request<EventIntelRequest>,
    ) -> Result<Response<EventIntelResponse>, Status> {
        let req = request.into_inner();
        let actor = if req.actor.trim().is_empty() {
            "system".to_string()
        } else {
            req.actor.clone()
        };
        let event = fetch_event_intel(&self.http_client, &self.backend_url, &req.event_id, &req.symbols)
            .await
            .map_err(|err| Status::not_found(format!("event intel unavailable: {err}")))?;

        let claims = event
            .claims
            .into_iter()
            .map(|c| EventClaim {
                claim_id: c.claim_id,
                event_id: c.event_id,
                claim_type: c.claim_type,
                claim_text: c.claim_text,
                verification_status: c.verification_status,
                confidence: c.confidence,
                evidence: c
                    .evidence
                    .into_iter()
                    .map(|e| EventEvidence {
                        source: e.source,
                        url: e.url,
                        published_at: e.published_at,
                        quote: e.quote,
                        reliability: e.reliability,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();

        let impacts = event
            .impacts
            .into_iter()
            .map(Self::map_event_impact)
            .collect::<Vec<_>>();

        let timeline = event
            .timeline
            .into_iter()
            .map(|t| EventTimelineEntry {
                event_id: t.event_id,
                timestamp: t.timestamp,
                status: t.status,
                change_note: t.change_note,
            })
            .collect::<Vec<_>>();

        self.record_audit(
            "event_intel_served",
            &actor,
            json!({"event_id": req.event_id, "claim_count": claims.len(), "impact_count": impacts.len()}),
        )
        .await?;

        Ok(Response::new(EventIntelResponse {
            event: Some(Self::map_event_summary(event.event)),
            claims,
            impacts,
            timeline,
            citations: event.citations,
            what_changed_since_last_update: event.what_changed_since_last_update,
        }))
    }

    async fn get_event_impact(
        &self,
        request: Request<EventImpactRequest>,
    ) -> Result<Response<EventImpactResponse>, Status> {
        let req = request.into_inner();
        let actor = if req.actor.trim().is_empty() {
            "system".to_string()
        } else {
            req.actor.clone()
        };
        let data = fetch_event_impact(&self.http_client, &self.backend_url, &req.event_id, &req.symbols)
            .await
            .map_err(|err| Status::not_found(format!("event impact unavailable: {err}")))?;

        let impacts = data.items.into_iter().map(Self::map_event_impact).collect::<Vec<_>>();
        self.record_audit(
            "event_impact_served",
            &actor,
            json!({"event_id": req.event_id, "impact_count": impacts.len()}),
        )
        .await?;

        Ok(Response::new(EventImpactResponse {
            event_id: data.event_id,
            impacts,
            stale: false,
            generated_at: Utc::now().to_rfc3339(),
        }))
    }

    async fn get_ml_decision_support(
        &self,
        request: Request<MlDecisionSupportRequest>,
    ) -> Result<Response<MlDecisionSupportResponse>, Status> {
        let req = request.into_inner();
        let actor = if req.actor.trim().is_empty() {
            "system".to_string()
        } else {
            req.actor.clone()
        };
        let model_version = if req.model_version.trim().is_empty() {
            "current".to_string()
        } else {
            req.model_version.clone()
        };
        let confidence_floor = req.confidence_floor.clamp(0.0, 0.95);

        let mut features = req.features;
        if features.is_empty() {
            if !req.symbol.trim().is_empty() {
                let syms = vec![req.symbol.clone()];
                if let Ok(market) = fetch_market_data(&self.http_client, &self.backend_url, &syms).await {
                    if let Some(entry) = market.get(&req.symbol) {
                        if !entry.features.is_empty() {
                            features = entry.features.clone();
                        }
                    }
                }
                if features.is_empty() {
                    features = Self::synth_features_for_symbol(&req.symbol);
                }
            } else {
                features = Self::synth_features_for_symbol("SPY");
            }
        }

        let ds = fetch_ml_decision_support(
            &self.http_client,
            &self.backend_url,
            &features,
            &model_version,
            confidence_floor,
            &actor,
        )
        .await
        .map_err(|err| Status::unavailable(format!("ml decision support fetch failed: {err}")))?;

        self.record_audit(
            "ml_decision_support_served",
            &actor,
            json!({
                "model_version": model_version,
                "symbol": req.symbol,
                "confidence_floor": confidence_floor,
                "confidence_gated": ds.confidence_gated,
            }),
        )
        .await?;

        let regime = MlRegimeSnapshot {
            regime: ds.regime.regime,
            momentum: ds.regime.momentum,
            realized_vol_proxy: ds.regime.realized_vol_proxy,
            liquidity_proxy: ds.regime.liquidity_proxy,
        };
        let horizons = ds
            .horizons
            .into_iter()
            .map(|h| MlHorizonDecision {
                horizon: h.horizon,
                expected_return: h.expected_return,
                downside_risk: h.downside_risk,
                confidence: h.confidence,
                prob_up: h.prob_up,
                prob_flat: h.prob_flat,
                prob_down: h.prob_down,
                action_band: h.action_band,
            })
            .collect::<Vec<_>>();
        let advisory = MlPortfolioRiskAdvisory {
            exposure_band: ds.portfolio_risk_advisory.exposure_band,
            concentration_risk: ds.portfolio_risk_advisory.concentration_risk,
            suggested_sizing_band: ds.portfolio_risk_advisory.suggested_sizing_band,
            max_single_position_pct: ds.portfolio_risk_advisory.max_single_position_pct,
            stop_review_required: ds.portfolio_risk_advisory.stop_review_required,
        };

        Ok(Response::new(MlDecisionSupportResponse {
            model_version: ds.model_version,
            model_lineage: ds.model_lineage,
            feature_contract_status: ds.feature_contract_status,
            confidence_floor: ds.confidence_floor,
            confidence_gated: ds.confidence_gated,
            calibration_error: ds.calibration_error,
            confidence_drift: ds.confidence_drift,
            regime: Some(regime),
            horizons,
            portfolio_risk_advisory: Some(advisory),
            generated_at: ds.generated_at,
        }))
    }

    async fn get_ml_calibration_status(
        &self,
        request: Request<MlCalibrationStatusRequest>,
    ) -> Result<Response<MlCalibrationStatusResponse>, Status> {
        let req = request.into_inner();
        let actor = if req.actor.trim().is_empty() {
            "system".to_string()
        } else {
            req.actor.clone()
        };
        let status = fetch_ml_calibration_status(&self.http_client, &self.backend_url)
            .await
            .map_err(|err| Status::unavailable(format!("ml calibration status fetch failed: {err}")))?;

        self.record_audit(
            "ml_calibration_status_served",
            &actor,
            json!({
                "sample_count": status.sample_count,
                "ece": status.ece,
                "brier_score": status.brier_score,
            }),
        )
        .await?;

        Ok(Response::new(MlCalibrationStatusResponse {
            sample_count: status.sample_count,
            ece: status.ece,
            brier_score: status.brier_score,
            hit_rate: status.hit_rate,
            confidence_drift: status.confidence_drift,
            updated_at: status.updated_at,
        }))
    }

    async fn predict(
        &self,
        request: Request<PredictRequest>,
    ) -> Result<Response<PredictResponse>, Status> {
        let req = request.into_inner();
        let actor = req.actor.clone();
        let model_version = req.model_version.clone();
        let feature_count = req.features.len() as i32;

        let backend_req = BackendPredictRequest {
            features: req.features,
            model_version: req.model_version,
            actor: req.actor,
        };
        let data = self
            .backend
            .predict(backend_req)
            .await
            .map_err(map_backend_error)?;

        let audit_hash = self
            .record_audit(
                "predict_forwarded",
                &actor,
                json!({
                    "model_version": model_version,
                    "score": data.score,
                    "feature_count": feature_count,
                    "kill_switch_active": data.kill_switch_active,
                }),
            )
            .await?;

        let checkpoint = if audit_hash == "audit-disabled" {
            data.audit_chain_checkpoint
        } else {
            audit_hash
        };

        Ok(Response::new(PredictResponse {
            score: data.score,
            model_version: data.model_version,
            disclaimer_hash: data.disclaimer_hash,
            prediction_signature: data.prediction_signature,
            jurisdiction_restriction: data.jurisdiction_restriction,
            kill_switch_active: data.kill_switch_active,
            audit_chain_checkpoint: checkpoint,
            jurisdiction_primary: data.jurisdiction_primary,
            jurisdiction_backup: data.jurisdiction_backup,
            feature_contract_status: data.feature_contract_status,
        }))
    }

    async fn get_ops_status(
        &self,
        _request: Request<OpsStatusRequest>,
    ) -> Result<Response<OpsStatusResponse>, Status> {
        let status = self
            .backend
            .ops_status()
            .await
            .map_err(map_backend_error)?;
        let mut response = Self::map_ops_status(status);

        let audit_status = self.audit.status().await.map_err(map_audit_error)?;
        if audit_status.chain_verification_status != "disabled" {
            response.chain_verification_status = audit_status.chain_verification_status;
            response.audit_rpo_seconds = audit_status.audit_rpo_seconds;
        }

        Ok(Response::new(response))
    }

    async fn set_kill_switch(
        &self,
        request: Request<KillSwitchRequest>,
    ) -> Result<Response<KillSwitchResponse>, Status> {
        let req = request.into_inner();
        let actor = req.actor.clone();
        let action = req.action.clone();
        let reason = req.reason.clone();
        let approver_count = req.approvers.len() as i32;

        let backend_req = BackendKillSwitchRequest {
            action: req.action,
            reason: req.reason,
            reason_signature: req.reason_signature,
            approvers: req.approvers,
            actor: req.actor,
            role: req.role,
            mfa_token: req.mfa_token,
        };

        let data = self
            .backend
            .set_kill_switch(backend_req)
            .await
            .map_err(map_backend_error)?;

        self.record_audit(
            "kill_switch_forwarded",
            &actor,
            json!({
                "action": action,
                "reason": reason,
                "result_level": data.level.clone(),
                "approver_count": approver_count,
            }),
        )
        .await?;

        Ok(Response::new(KillSwitchResponse {
            level: data.level,
            updated_at: data.updated_at,
            reason: data.reason,
        }))
    }

    async fn subscribe_signals(
        &self,
        request: Request<SignalSubscriptionRequest>,
    ) -> Result<Response<Self::SubscribeSignalsStream>, Status> {
        let req = request.into_inner();
        let mut symbols = req
            .symbols
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>();
        if symbols.is_empty() {
            symbols = vec![
                "BTCUSD".to_string(),
                "ETHUSD".to_string(),
                "SPY".to_string(),
                "QQQ".to_string(),
                "NVDA".to_string(),
                "TSLA".to_string(),
            ];
        }

        let refresh_seconds = req.refresh_seconds.clamp(15, 300);
        let actor = if req.actor.trim().is_empty() {
            "signal-stream-client".to_string()
        } else {
            req.actor
        };
        let model_version = if req.model_version.trim().is_empty() {
            "current".to_string()
        } else {
            req.model_version
        };

        let (tx, rx) = mpsc::channel::<Result<SignalSnapshot, Status>>(8);
        let service = (*self).clone();

        tokio::spawn(async move {
            loop {
                let snapshot = service
                    .build_signal_snapshot(&symbols, refresh_seconds, &actor, &model_version)
                    .await;

                match snapshot {
                    Ok(data) => {
                        if tx.send(Ok(data)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                }

                sleep(Duration::from_secs(refresh_seconds as u64)).await;
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx)) as Self::SubscribeSignalsStream))
    }

    async fn subscribe_event_alerts(
        &self,
        request: Request<EventAlertSubscriptionRequest>,
    ) -> Result<Response<Self::SubscribeEventAlertsStream>, Status> {
        let req = request.into_inner();
        let mut symbols = req
            .symbols
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>();
        if symbols.is_empty() {
            symbols = vec!["SPY".to_string(), "QQQ".to_string(), "AAPL".to_string()];
        }
        let refresh_seconds = req.refresh_seconds.clamp(15, 300);
        let actor = if req.actor.trim().is_empty() {
            "event-alert-client".to_string()
        } else {
            req.actor
        };
        let severity_floor = req.severity_floor;
        let confidence_floor = req.confidence_floor.clamp(0.0, 1.0);
        let contradiction_only = req.contradiction_only;
        let service = (*self).clone();

        let (tx, rx) = mpsc::channel::<Result<EventAlert, Status>>(32);
        tokio::spawn(async move {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            loop {
                let board = fetch_news_events(
                    &service.http_client,
                    &service.backend_url,
                    &symbols,
                    "",
                    "",
                    "24h",
                    50,
                    "",
                )
                .await;

                match board {
                    Ok(data) => {
                        for ev in data.items {
                            let sev_ok = if severity_floor.is_empty() {
                                true
                            } else {
                                Self::severity_score(&ev.severity) >= Self::severity_score(&severity_floor)
                            };
                            let conf_ok = ev.confidence >= confidence_floor;
                            let contra_ok = !contradiction_only || ev.contradiction_score >= 0.35;
                            let unseen = !seen.contains(&ev.event_id);
                            if sev_ok && conf_ok && contra_ok && unseen {
                                let event_id = ev.event_id.clone();
                                let reason = format!(
                                    "severity={} confidence={:.2} contradiction={:.2}",
                                    ev.severity, ev.confidence, ev.contradiction_score
                                );
                                let impact = fetch_event_impact(
                                    &service.http_client,
                                    &service.backend_url,
                                    &event_id,
                                    &symbols,
                                )
                                .await
                                .ok()
                                .and_then(|d| d.items.into_iter().next())
                                .map(Self::map_event_impact);

                                let alert = EventAlert {
                                    event: Some(Self::map_event_summary(ev)),
                                    impact,
                                    emitted_at: Utc::now().to_rfc3339(),
                                    reason,
                                };
                                if tx.send(Ok(alert)).await.is_err() {
                                    return;
                                }
                                seen.insert(event_id);
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx
                            .send(Err(Status::unavailable(format!("event alert stream failed: {err}"))))
                            .await;
                        return;
                    }
                }

                let _ = service
                    .record_audit(
                        "event_alert_tick",
                        &actor,
                        json!({"symbols": symbols, "refresh_seconds": refresh_seconds}),
                    )
                    .await;

                sleep(Duration::from_secs(refresh_seconds as u64)).await;
            }
        });

        Ok(Response::new(
            Box::pin(ReceiverStream::new(rx)) as Self::SubscribeEventAlertsStream
        ))
    }
}

async fn fetch_market_data(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbols: &[String],
) -> Result<HashMap<String, MarketDataEntry>, reqwest::Error> {
    let syms = symbols.join(",");
    let url = format!("{}/v1/market-data?symbols={}", backend_url, syms);
    let data: MarketDataResponse = http_client.get(&url).send().await?.json().await?;
    Ok(data)
}

async fn fetch_news_items(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbols: &[String],
    max: usize,
) -> Result<Vec<NewsItemRaw>, reqwest::Error> {
    let syms = symbols.join(",");
    let url = format!("{}/v1/news?symbols={}&max={}", backend_url, syms, max);
    let data: NewsResponse = http_client.get(&url).send().await?.json().await?;
    Ok(data.items)
}

async fn fetch_history(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbol: &str,
    timeframe: &str,
) -> Result<HistoryResponse, reqwest::Error> {
    let url = format!(
        "{}/v1/market-data/history?symbol={}&range={}&interval=1h",
        backend_url, symbol, timeframe
    );
    http_client.get(&url).send().await?.json::<HistoryResponse>().await
}

async fn fetch_digest(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbol: &str,
) -> Result<DigestResponse, reqwest::Error> {
    let url = format!("{}/v1/news/digest?symbol={}&max=20", backend_url, symbol);
    http_client.get(&url).send().await?.json::<DigestResponse>().await
}

async fn fetch_focus_bundle_http(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbol: &str,
    timeframe: &str,
) -> Result<FocusBundleHttpResponse, reqwest::Error> {
    let url = format!("{}/v1/focus/bundle?symbol={}&timeframe={}", backend_url, symbol, timeframe);
    http_client
        .get(&url)
        .send()
        .await?
        .json::<FocusBundleHttpResponse>()
        .await
}

async fn fetch_news_events(
    http_client: &reqwest::Client,
    backend_url: &str,
    symbols: &[String],
    severity: &str,
    sentiment: &str,
    window: &str,
    limit: i32,
    cursor: &str,
) -> Result<NewsEventsResponse, reqwest::Error> {
    let syms = symbols.join(",");
    let url = format!(
        "{}/v1/news/events?symbols={}&severity={}&sentiment={}&window={}&limit={}&cursor={}",
        backend_url, syms, severity, sentiment, window, limit, cursor
    );
    http_client.get(&url).send().await?.json::<NewsEventsResponse>().await
}

async fn fetch_event_intel(
    http_client: &reqwest::Client,
    backend_url: &str,
    event_id: &str,
    symbols: &[String],
) -> Result<EventIntelResponseRaw, reqwest::Error> {
    let syms = symbols.join(",");
    let url = format!("{}/v1/news/events/{}?symbols={}", backend_url, event_id, syms);
    http_client
        .get(&url)
        .send()
        .await?
        .json::<EventIntelResponseRaw>()
        .await
}

async fn fetch_event_impact(
    http_client: &reqwest::Client,
    backend_url: &str,
    event_id: &str,
    symbols: &[String],
) -> Result<EventImpactListResponse, reqwest::Error> {
    let syms = symbols.join(",");
    let url = format!("{}/v1/news/events/{}/impact?symbols={}", backend_url, event_id, syms);
    http_client
        .get(&url)
        .send()
        .await?
        .json::<EventImpactListResponse>()
        .await
}

#[derive(serde::Serialize)]
struct MlDecisionSupportPayload {
    features: HashMap<String, f64>,
    model_version: String,
    confidence_floor: f64,
}

async fn fetch_ml_decision_support(
    http_client: &reqwest::Client,
    backend_url: &str,
    features: &HashMap<String, f64>,
    model_version: &str,
    confidence_floor: f64,
    actor: &str,
) -> Result<MlDecisionSupportResponseRaw, reqwest::Error> {
    let url = format!("{}/v1/ml/decision-support", backend_url);
    let payload = MlDecisionSupportPayload {
        features: features.clone(),
        model_version: model_version.to_string(),
        confidence_floor,
    };
    http_client
        .post(&url)
        .header("x-actor", actor)
        .json(&payload)
        .send()
        .await?
        .json::<MlDecisionSupportResponseRaw>()
        .await
}

async fn fetch_ml_calibration_status(
    http_client: &reqwest::Client,
    backend_url: &str,
) -> Result<MlCalibrationStatusRaw, reqwest::Error> {
    let url = format!("{}/v1/ml/calibration/status", backend_url);
    http_client
        .get(&url)
        .send()
        .await?
        .json::<MlCalibrationStatusRaw>()
        .await
}

fn map_audit_error(err: AuditError) -> Status {
    match err {
        AuditError::Config(message) => Status::failed_precondition(message),
        AuditError::Postgres(message) => Status::unavailable(message),
        AuditError::ObjectStore(message) => Status::unavailable(message),
        AuditError::Serialization(message) => Status::internal(message),
    }
}

fn synth_features(symbol: &str, timestamp_seconds: i64) -> HashMap<String, f64> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    symbol.hash(&mut hasher);
    let base = hasher.finish();
    let bucket = (timestamp_seconds / 30).unsigned_abs();

    let jitter = |salt: u64| {
        let mixed = base ^ (bucket.wrapping_mul(37 + salt));
        (mixed % 100) as f64 / 100.0
    };

    let mut features = HashMap::with_capacity(5);
    features.insert("open".to_string(), jitter(1));
    features.insert("high".to_string(), jitter(2).max(0.1));
    features.insert("low".to_string(), jitter(3).min(0.9));
    features.insert("close".to_string(), jitter(4));
    features.insert("volume".to_string(), jitter(5));
    features
}

fn feature_contributions(features: &HashMap<String, f64>) -> Vec<FeatureContribution> {
    let mut out = features
        .iter()
        .map(|(name, value)| FeatureContribution {
            name: name.clone(),
            score: ((value - 0.5) * 2.0 * 100.0).round() / 100.0,
        })
        .collect::<Vec<_>>();

    out.sort_by(|a, b| {
        b.score
            .abs()
            .partial_cmp(&a.score.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    out
}
