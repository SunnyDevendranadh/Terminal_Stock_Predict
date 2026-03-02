use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_stream::StreamExt;
use tonic::Request;

use vps1_serving::audit::{AuditError, AuditStatus, AuditWriter};
use vps1_serving::backend::{
    BackendClient, BackendError, BackendKillSwitchRequest, BackendKillSwitchResponse, BackendOpsStatus,
    BackendPredictRequest, BackendPredictResponse,
};
use vps1_serving::gateway::PredictionGatewayService;
use vps1_serving::pb::prediction_gateway_server::PredictionGateway;
use vps1_serving::pb::{KillSwitchRequest, OpsStatusRequest, PredictRequest, SignalSubscriptionRequest};

#[derive(Clone)]
struct MockBackend;

#[derive(Clone)]
struct MockAudit;

#[async_trait]
impl BackendClient for MockBackend {
    async fn predict(&self, req: BackendPredictRequest) -> Result<BackendPredictResponse, BackendError> {
        assert_eq!(req.model_version, "current");
        assert_eq!(req.actor, "ops-analyst");
        assert!(req.features.contains_key("open"));

        Ok(BackendPredictResponse {
            score: 0.42,
            model_version: "current".to_string(),
            disclaimer_hash: "disc_hash_v1".to_string(),
            prediction_signature: "pred_sig_v1".to_string(),
            jurisdiction_restriction: "decision-support-only".to_string(),
            kill_switch_active: false,
            audit_chain_checkpoint: "chk_001".to_string(),
            jurisdiction_primary: "Iceland".to_string(),
            jurisdiction_backup: "Switzerland".to_string(),
            feature_contract_status: "valid".to_string(),
        })
    }

    async fn ops_status(&self) -> Result<BackendOpsStatus, BackendError> {
        Ok(BackendOpsStatus {
            kill_switch_active: true,
            kill_switch_level: "soft".to_string(),
            chain_verification_status: "valid".to_string(),
            audit_rpo_seconds: 60,
        })
    }

    async fn set_kill_switch(
        &self,
        req: BackendKillSwitchRequest,
    ) -> Result<BackendKillSwitchResponse, BackendError> {
        assert_eq!(req.action, "hard_on");
        assert_eq!(req.reason, "integrity breach");
        assert_eq!(req.role, "ops-admin");
        assert_eq!(req.mfa_token, "123456");
        assert_eq!(req.approvers.len(), 2);

        Ok(BackendKillSwitchResponse {
            level: "hard".to_string(),
            updated_at: "2026-02-28T20:00:00Z".to_string(),
            reason: req.reason,
        })
    }
}

#[async_trait]
impl AuditWriter for MockAudit {
    async fn write_event(&self, _event_type: &str, _actor: &str, _payload: Value) -> Result<String, AuditError> {
        Ok("rust-audit-hash-001".to_string())
    }

    async fn status(&self) -> Result<AuditStatus, AuditError> {
        Ok(AuditStatus {
            chain_verification_status: "valid".to_string(),
            audit_rpo_seconds: 60,
            last_hash: "rust-audit-hash-001".to_string(),
        })
    }
}

#[tokio::test]
async fn predict_contract_parity_with_python_fields() {
    let service = PredictionGatewayService::new(
        MockBackend,
        Arc::new(MockAudit),
        "http://127.0.0.1:18080".to_string(),
    );
    let mut features = HashMap::new();
    features.insert("open".to_string(), 0.2);
    features.insert("high".to_string(), 0.3);
    features.insert("low".to_string(), 0.1);
    features.insert("close".to_string(), 0.4);
    features.insert("volume".to_string(), 0.5);

    let response = service
        .predict(Request::new(PredictRequest {
            features,
            model_version: "current".to_string(),
            actor: "ops-analyst".to_string(),
        }))
        .await
        .expect("predict response")
        .into_inner();

    assert_eq!(response.model_version, "current");
    assert_eq!(response.disclaimer_hash, "disc_hash_v1");
    assert_eq!(response.prediction_signature, "pred_sig_v1");
    assert_eq!(response.jurisdiction_restriction, "decision-support-only");
    assert!(!response.kill_switch_active);
    assert_eq!(response.audit_chain_checkpoint, "rust-audit-hash-001");
    assert_eq!(response.jurisdiction_primary, "Iceland");
    assert_eq!(response.jurisdiction_backup, "Switzerland");
    assert_eq!(response.feature_contract_status, "valid");
}

#[tokio::test]
async fn ops_status_contract_parity_with_python_fields() {
    let service = PredictionGatewayService::new(
        MockBackend,
        Arc::new(MockAudit),
        "http://127.0.0.1:18080".to_string(),
    );

    let response = service
        .get_ops_status(Request::new(OpsStatusRequest {}))
        .await
        .expect("ops status response")
        .into_inner();

    assert!(response.kill_switch_active);
    assert_eq!(response.kill_switch_level, "soft");
    assert_eq!(response.chain_verification_status, "valid");
    assert_eq!(response.audit_rpo_seconds, 60);
}

#[tokio::test]
async fn kill_switch_contract_parity_with_python_fields() {
    let service = PredictionGatewayService::new(
        MockBackend,
        Arc::new(MockAudit),
        "http://127.0.0.1:18080".to_string(),
    );

    let response = service
        .set_kill_switch(Request::new(KillSwitchRequest {
            action: "hard_on".to_string(),
            reason: "integrity breach".to_string(),
            reason_signature: "sig_001".to_string(),
            approvers: vec!["operator-a".to_string(), "operator-b".to_string()],
            actor: "security-oncall".to_string(),
            role: "ops-admin".to_string(),
            mfa_token: "123456".to_string(),
        }))
        .await
        .expect("kill switch response")
        .into_inner();

    assert_eq!(response.level, "hard");
    assert_eq!(response.updated_at, "2026-02-28T20:00:00Z");
    assert_eq!(response.reason, "integrity breach");
}

#[tokio::test]
async fn subscribe_signals_stream_emits_snapshot() {
    let service = PredictionGatewayService::new(
        MockBackend,
        Arc::new(MockAudit),
        "http://127.0.0.1:18080".to_string(),
    );

    let response = service
        .subscribe_signals(Request::new(SignalSubscriptionRequest {
            symbols: vec!["SPY".to_string(), "QQQ".to_string()],
            refresh_seconds: 15,
            actor: "ops-analyst".to_string(),
            model_version: "current".to_string(),
        }))
        .await
        .expect("subscribe stream response");

    let mut stream = response.into_inner();
    let snapshot = stream
        .next()
        .await
        .expect("stream polling")
        .expect("first snapshot");

    assert!(!snapshot.signals.is_empty());
    assert!(snapshot.threat.is_some());
    assert_eq!(
        snapshot
            .threat
            .as_ref()
            .expect("threat payload")
            .chain_verification_status,
        "valid"
    );
}
