from __future__ import annotations

import hashlib
import hmac
import json
from datetime import datetime, timezone
from typing import Any

from .alerts import AlertRouter
from .audit import AuditStore
from .canary import CanaryGate
from .config import AppConfig
from .credentials import CredentialManager
from .kill_switch import KillLevel, KillSwitch
import dataclasses

from .market_data import MarketDataProvider, YFinanceProvider, fetch_batch, fetch_history
from .ml_decision_support import DecisionSupportEngine
from .news_feed import fetch_news, fetch_news_digest
from .news_intel import NewsIntelEngine

UTC = timezone.utc


class PlatformService:
    def __init__(self, config: AppConfig, market_data_provider: MarketDataProvider | None = None) -> None:
        self.config = config
        self.audit = AuditStore(config.audit, signing_key=config.security.model_signing_key)
        self.alerts = AlertRouter()
        self.credentials = CredentialManager(config.credentials)
        self.canary = CanaryGate()
        self.kill_switch = KillSwitch()
        self._feature_manifest_updated_at = datetime.now(tz=UTC)
        self._last_known_good: dict[str, Any] | None = None
        self._disclaimer_hash = hashlib.sha256(
            config.disclaimer.encode("utf-8")
        ).hexdigest()
        self._market_data_provider: MarketDataProvider = (
            market_data_provider if market_data_provider is not None else YFinanceProvider()
        )
        self.news_intel = NewsIntelEngine(timeout_secs=config.news.timeout_secs)
        self.ml_decision_support = DecisionSupportEngine()

    def _feature_contract_status(self, features: dict[str, float]) -> str:
        age = datetime.now(tz=UTC) - self._feature_manifest_updated_at
        if age > self.config.features.stale_after:
            return "stale"
        incoming = set(features.keys())
        expected = set(self.config.features.expected_features)
        if incoming == expected:
            return "valid"
        return "mismatch"

    def _sign_prediction(self, payload: dict[str, Any]) -> str:
        text = json.dumps(payload, sort_keys=True, separators=(",", ":"))
        return hmac.new(
            self.config.security.model_signing_key.encode("utf-8"),
            text.encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()

    def _audit_checkpoint(self) -> str:
        checkpoint = self.audit.create_checkpoint()
        return str(checkpoint["checkpoint_hash"])

    def _compute_score(self, features: dict[str, float]) -> float:
        if not features:
            return 0.0
        values = list(features.values())
        raw = sum(values) / len(values)
        return max(0.0, min(1.0, raw))

    def predict(self, features: dict[str, float], model_version: str, actor: str) -> dict[str, Any]:
        status = self._feature_contract_status(features)
        if status != "valid":
            self.audit.append(
                event_type="prediction_rejected",
                actor=actor,
                payload={"reason": "feature_contract", "status": status},
            )
            raise ValueError(f"feature_contract_status={status}")

        level = self.kill_switch.state.level
        if level == KillLevel.HARD:
            self.audit.append(
                event_type="prediction_blocked",
                actor=actor,
                payload={"reason": "hard_kill_switch"},
            )
            raise PermissionError("hard_kill_switch_active")

        if level == KillLevel.SOFT and self._last_known_good:
            replay = dict(self._last_known_good)
            replay["kill_switch_active"] = True
            self.audit.append(
                event_type="prediction_soft_kill_replay",
                actor=actor,
                payload={"model_version": replay.get("model_version", "unknown")},
            )
            return replay

        payload = {
            "score": self._compute_score(features),
            "model_version": model_version,
            "disclaimer_hash": self._disclaimer_hash,
            "jurisdiction_restriction": "decision-support-only",
            "kill_switch_active": self.kill_switch.active,
            "audit_chain_checkpoint": self._audit_checkpoint(),
            "jurisdiction_primary": self.config.jurisdiction.primary,
            "jurisdiction_backup": self.config.jurisdiction.backup,
            "feature_contract_status": status,
        }

        ml = self.ml_decision_support.generate(features=features, model_version=model_version, confidence_floor=0.55)
        intraday = next((h for h in ml.horizons if h.horizon == "intraday"), ml.horizons[0])
        payload.update(
            {
                "expected_return": intraday.expected_return,
                "downside_risk": intraday.downside_risk,
                "confidence": intraday.confidence,
                "prob_up": intraday.prob_up,
                "prob_flat": intraday.prob_flat,
                "prob_down": intraday.prob_down,
                "regime": dataclasses.asdict(ml.regime),
                "horizon_outputs": [dataclasses.asdict(h) for h in ml.horizons],
                "confidence_floor": ml.confidence_floor,
                "confidence_gated": ml.confidence_gated,
                "calibration_error": ml.calibration_error,
                "confidence_drift": ml.confidence_drift,
                "model_lineage": ml.model_lineage,
                "portfolio_risk_advisory": dataclasses.asdict(ml.portfolio_risk_advisory),
                "decision_support_generated_at": ml.generated_at,
            }
        )
        payload["prediction_signature"] = self._sign_prediction(payload)
        self._last_known_good = dict(payload)
        self.audit.append(
            event_type="prediction_served",
            actor=actor,
            payload={
                "model_version": model_version,
                "score": payload["score"],
                "confidence": payload["confidence"],
                "regime": payload["regime"]["regime"],
            },
        )
        return payload

    def ops_audit(self) -> dict[str, Any]:
        self.audit.flush_object_store()
        self.audit.create_checkpoint()
        return self.audit.ops_summary()

    def ops_credentials(self) -> dict[str, object]:
        return self.credentials.status()

    def rotate_credentials(self, provider: str, new_key_id: str, new_key_secret: str, actor: str) -> dict[str, Any]:
        result = self.credentials.rotate(provider, new_key_id, new_key_secret)
        self.audit.append(
            event_type="credential_rotated",
            actor=actor,
            payload={
                "provider": provider,
                "active_key_id": result["active_key_id"],
                "checksum": result["checksum"],
            },
        )
        return result

    def _verify_kill_reason_signature(self, reason: str, reason_signature: str) -> bool:
        expected = hmac.new(
            self.config.security.kill_switch_reason_secret.encode("utf-8"),
            reason.encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()
        return hmac.compare_digest(expected, reason_signature)

    def set_kill_switch(
        self,
        action: str,
        reason: str,
        reason_signature: str,
        approvers: list[str],
        actor: str,
        role: str,
        mfa_token: str,
    ) -> dict[str, Any]:
        if role != "ops-admin":
            raise PermissionError("rbac_denied")
        if len(mfa_token.strip()) < 6:
            raise PermissionError("mfa_required")
        if not self._verify_kill_reason_signature(reason, reason_signature):
            raise PermissionError("invalid_reason_signature")

        if action == "soft_on":
            state = self.kill_switch.activate_soft(reason)
        elif action == "soft_off":
            state = self.kill_switch.clear_soft(reason)
        elif action == "hard_on":
            state = self.kill_switch.activate_hard(reason)
        elif action == "hard_off":
            state = self.kill_switch.deactivate_hard(reason, approvers)
        else:
            raise ValueError(f"unsupported_action={action}")

        self.audit.append(
            event_type="kill_switch_changed",
            actor=actor,
            payload={
                "action": action,
                "reason": reason,
                "result_level": state.level.value,
                "approvers": approvers,
            },
        )
        return {
            "level": state.level.value,
            "updated_at": state.updated_at.isoformat(),
            "reason": state.reason,
        }

    def evaluate_canary(self, current_scores: list[float], candidate_scores: list[float], actor: str) -> dict[str, Any]:
        result = self.canary.evaluate(current_scores=current_scores, candidate_scores=candidate_scores)
        result_dict = {
            "outcome": result.outcome,
            "action": result.action,
            "p_value": result.p_value,
            "calibration_delta": result.calibration_delta,
            "risk_band_shift": result.risk_band_shift,
        }
        self.audit.append(
            event_type="canary_evaluated",
            actor=actor,
            payload=result_dict,
        )
        if result.outcome == "fail":
            self.alerts.send(
                severity="critical",
                message="Canary gate failed; automatic rollback requested.",
            )
        return result_dict

    def compliance_summary(self) -> dict[str, Any]:
        return {
            "jurisdiction_policy_version": self.config.jurisdiction.version,
            "jurisdiction_primary": self.config.jurisdiction.primary,
            "jurisdiction_backup": self.config.jurisdiction.backup,
            "data_residency_map": self.config.jurisdiction.residency_map,
            "kill_switch_level": self.kill_switch.state.level.value,
            "decision_support_only": True,
        }

    def market_data_snapshot(self, symbols: list[str]) -> dict[str, Any]:
        """Fetch market data for given symbols; return {symbol: {price, change_pct, features}}."""
        batch = fetch_batch(symbols, self._market_data_provider)
        return {
            sym: {
                "price": result.price,
                "change_pct": result.change_pct,
                "features": result.features,
                "fetched_at": result.fetched_at.isoformat(),
            }
            for sym, result in batch.items()
        }

    def market_data_history_snapshot(self, symbol: str, range: str, interval: str) -> dict[str, Any]:
        """Fetch OHLCV history for a symbol; returns error dict if fetch fails."""
        result = fetch_history(symbol, range, interval)
        if result is None:
            return {"error": "fetch failed", "symbol": symbol}
        return dataclasses.asdict(result)

    def news_digest_snapshot(self, symbol: str, max_items: int = 20) -> dict[str, Any]:
        """Fetch enriched news digest for a symbol (or all news if symbol is empty)."""
        syms = [symbol] if symbol else []
        items = fetch_news_digest(syms, max_items)
        return {"items": [dataclasses.asdict(i) for i in items]}

    def news_events_snapshot(
        self,
        symbols: list[str],
        severity: str = "",
        sentiment: str = "",
        window: str = "24h",
        limit: int = 25,
        cursor: str = "",
        actor: str = "system",
    ) -> dict[str, Any]:
        result = self.news_intel.list_events(
            symbols=symbols,
            severity=severity,
            sentiment=sentiment,
            window=window,
            limit=max(1, min(100, limit)),
            cursor=cursor,
        )
        self.audit.append(
            event_type="news_events_listed",
            actor=actor,
            payload={
                "symbols": symbols,
                "severity": severity,
                "sentiment": sentiment,
                "window": window,
                "count": result.get("count", 0),
            },
        )
        return result

    def news_event_snapshot(self, event_id: str, symbols: list[str], actor: str = "system") -> dict[str, Any]:
        event = self.news_intel.get_event(event_id=event_id, symbols=symbols, window="7d")
        if event is None:
            return {"detail": "event_not_found", "event_id": event_id}
        self.audit.append(
            event_type="news_event_read",
            actor=actor,
            payload={"event_id": event_id, "symbol": event.event.symbol},
        )
        return {
            "event": dataclasses.asdict(event.event),
            "claims": [dataclasses.asdict(c) for c in event.claims],
            "impacts": [dataclasses.asdict(i) for i in event.impacts],
            "timeline": [dataclasses.asdict(t) for t in event.timeline],
            "citations": event.citations,
            "what_changed_since_last_update": event.what_changed_since_last_update,
        }

    def news_event_claims_snapshot(self, event_id: str, symbols: list[str]) -> dict[str, Any]:
        event = self.news_intel.get_event(event_id=event_id, symbols=symbols, window="7d")
        if event is None:
            return {"items": [], "event_id": event_id}
        return {"items": [dataclasses.asdict(c) for c in event.claims], "event_id": event_id}

    def news_event_impact_snapshot(self, event_id: str, symbols: list[str]) -> dict[str, Any]:
        event = self.news_intel.get_event(event_id=event_id, symbols=symbols, window="7d")
        if event is None:
            return {"items": [], "event_id": event_id}
        return {"items": [dataclasses.asdict(i) for i in event.impacts], "event_id": event_id}

    def news_event_timeline_snapshot(self, event_id: str, symbols: list[str]) -> dict[str, Any]:
        event = self.news_intel.get_event(event_id=event_id, symbols=symbols, window="7d")
        if event is None:
            return {"items": [], "event_id": event_id}
        return {"items": [dataclasses.asdict(t) for t in event.timeline], "event_id": event_id}

    def focus_bundle_snapshot(self, symbol: str, timeframe: str, actor: str = "system") -> dict[str, Any]:
        history = self.market_data_history_snapshot(symbol=symbol, range=timeframe, interval="1h")
        digest = self.news_digest_snapshot(symbol=symbol, max_items=20).get("items", [])
        overlays = self.news_intel.focus_overlay(symbol=symbol, timeframe=timeframe)
        output = {
            "symbol": symbol,
            "timeframe": timeframe,
            "history": history,
            "digest": digest,
            "event_markers": overlays["event_markers"],
            "confidence_band": overlays["confidence_band"],
            "impact_probabilities": overlays["impact_probabilities"],
        }
        self.audit.append(
            event_type="focus_bundle_requested",
            actor=actor,
            payload={"symbol": symbol, "timeframe": timeframe, "digest_count": len(digest)},
        )
        return output

    def ml_decision_support_snapshot(
        self,
        features: dict[str, float],
        model_version: str,
        confidence_floor: float,
        actor: str = "system",
    ) -> dict[str, Any]:
        status = self._feature_contract_status(features)
        if status != "valid":
            raise ValueError(f"feature_contract_status={status}")
        snapshot = self.ml_decision_support.generate(
            features=features,
            model_version=model_version,
            confidence_floor=_safe_confidence_floor(confidence_floor),
        )
        payload = dataclasses.asdict(snapshot)
        payload["feature_contract_status"] = status
        self.audit.append(
            event_type="ml_decision_support_served",
            actor=actor,
            payload={
                "model_version": model_version,
                "confidence_gated": snapshot.confidence_gated,
                "regime": snapshot.regime.regime,
            },
        )
        return payload

    def ml_calibration_update(self, prob_up: float, realized_return: float, actor: str = "system") -> dict[str, Any]:
        status = self.ml_decision_support.register_realized_outcome(prob_up=prob_up, realized_return=realized_return)
        payload = dataclasses.asdict(status)
        self.audit.append(
            event_type="ml_calibration_updated",
            actor=actor,
            payload={
                "prob_up": round(prob_up, 6),
                "realized_return": round(realized_return, 6),
                "sample_count": status.sample_count,
            },
        )
        return payload

    def ml_calibration_status(self) -> dict[str, Any]:
        return dataclasses.asdict(self.ml_decision_support.calibration_status())

    def ml_walk_forward_snapshot(
        self,
        predictions: list[float],
        realized_returns: list[float],
        fold_size: int,
        purge: int,
        embargo: int,
        actor: str = "system",
    ) -> dict[str, Any]:
        report = self.ml_decision_support.walk_forward_validate(
            predictions=predictions,
            realized_returns=realized_returns,
            fold_size=max(5, fold_size),
            purge=max(0, purge),
            embargo=max(0, embargo),
        )
        payload = dataclasses.asdict(report)
        self.audit.append(
            event_type="ml_walk_forward_evaluated",
            actor=actor,
            payload={
                "folds": len(report.folds),
                "aggregate_hit_rate": report.aggregate_hit_rate,
                "aggregate_brier_score": report.aggregate_brier_score,
                "purge": report.purge,
                "embargo": report.embargo,
            },
        )
        return payload

    def ml_challenger_evaluate(
        self,
        champion_scores: list[float],
        challenger_scores: list[float],
        expected_cost_delta_bps: float,
        actor: str = "system",
    ) -> dict[str, Any]:
        if not champion_scores or not challenger_scores:
            raise ValueError("champion_scores and challenger_scores are required")
        evaluation = self.ml_decision_support.evaluate_challenger(
            champion_scores=champion_scores,
            challenger_scores=challenger_scores,
            expected_cost_delta_bps=expected_cost_delta_bps,
        )
        payload = dataclasses.asdict(evaluation)
        self.audit.append(
            event_type="ml_challenger_evaluated",
            actor=actor,
            payload={
                "outcome": evaluation.outcome,
                "recommendation": evaluation.recommendation,
                "promoted_model": evaluation.promoted_model,
            },
        )
        return payload

    def ml_challenger_status(self) -> dict[str, Any]:
        return self.ml_decision_support.challenger_status()

    def news_snapshot(self, symbols: list[str], max_items: int = 50) -> dict[str, Any]:
        """Fetch latest news items and match to symbols; return {"items": [...]}."""
        news_cfg = self.config.news
        items = fetch_news(
            symbols=symbols,
            max_items=max_items,
            timeout_secs=news_cfg.timeout_secs,
        )
        return {
            "items": [
                {
                    "headline": item.headline,
                    "source": item.source,
                    "url": item.url,
                    "published_at": item.published_at,
                    "symbol": item.symbol,
                    "sentiment": item.sentiment,
                }
                for item in items
            ]
        }


def _safe_confidence_floor(value: float) -> float:
    return max(0.0, min(0.95, value))

# PlatformService: central orchestrator wiring together audit, alerts, market data, and ML
