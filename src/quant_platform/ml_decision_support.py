from __future__ import annotations

import math
from collections import deque
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from statistics import mean

from .canary import CanaryGate

UTC = timezone.utc


def _clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


@dataclass
class HorizonDecision:
    horizon: str
    expected_return: float
    downside_risk: float
    confidence: float
    prob_up: float
    prob_flat: float
    prob_down: float
    action_band: str


@dataclass
class RegimeSnapshot:
    regime: str
    momentum: float
    realized_vol_proxy: float
    liquidity_proxy: float


@dataclass
class PortfolioRiskAdvisory:
    exposure_band: str
    concentration_risk: str
    suggested_sizing_band: str
    max_single_position_pct: float
    stop_review_required: bool


@dataclass
class DecisionSupportSnapshot:
    model_version: str
    model_lineage: str
    confidence_floor: float
    confidence_gated: bool
    calibration_error: float
    confidence_drift: float
    regime: RegimeSnapshot
    horizons: list[HorizonDecision]
    portfolio_risk_advisory: PortfolioRiskAdvisory
    generated_at: str


@dataclass
class CalibrationStatus:
    sample_count: int
    ece: float
    brier_score: float
    hit_rate: float
    confidence_drift: float
    updated_at: str


@dataclass
class WalkForwardFold:
    fold: int
    train_start: int
    train_end: int
    test_start: int
    test_end: int
    hit_rate: float
    brier_score: float


@dataclass
class WalkForwardReport:
    folds: list[WalkForwardFold]
    aggregate_hit_rate: float
    aggregate_brier_score: float
    purge: int
    embargo: int
    fold_size: int
    generated_at: str


@dataclass
class ChallengerEvaluation:
    outcome: str
    recommendation: str
    canary_action: str
    p_value: float
    calibration_delta: float
    risk_band_shift: float
    expected_cost_delta_bps: float
    promoted_model: str
    evaluated_at: str


class _RegimeClassifier:
    def classify(self, features: dict[str, float]) -> RegimeSnapshot:
        open_px = features.get("open", 0.5)
        high_px = features.get("high", 0.5)
        low_px = features.get("low", 0.5)
        close_px = features.get("close", 0.5)
        vol = features.get("volume", 0.5)

        momentum = close_px - open_px
        realized_vol_proxy = abs(high_px - low_px)
        liquidity_proxy = vol

        if realized_vol_proxy >= 0.35 and abs(momentum) <= 0.08:
            regime = "volatile-range"
        elif momentum >= 0.08 and close_px >= 0.55:
            regime = "trend-up"
        elif momentum <= -0.08 and close_px <= 0.45:
            regime = "trend-down"
        elif liquidity_proxy < 0.25:
            regime = "thin-liquidity"
        else:
            regime = "mean-reversion"

        return RegimeSnapshot(
            regime=regime,
            momentum=round(momentum, 4),
            realized_vol_proxy=round(realized_vol_proxy, 4),
            liquidity_proxy=round(liquidity_proxy, 4),
        )


class _CalibrationMonitor:
    def __init__(self, max_samples: int = 1500) -> None:
        self._pairs: deque[tuple[float, int, datetime]] = deque(maxlen=max_samples)

    def update(self, prob_up: float, realized_return: float) -> CalibrationStatus:
        label = 1 if realized_return > 0 else 0
        self._pairs.append((_clamp(prob_up, 0.0, 1.0), label, datetime.now(tz=UTC)))
        return self.status()

    def status(self) -> CalibrationStatus:
        now = datetime.now(tz=UTC)
        if not self._pairs:
            return CalibrationStatus(
                sample_count=0,
                ece=0.0,
                brier_score=0.0,
                hit_rate=0.0,
                confidence_drift=0.0,
                updated_at=now.isoformat(),
            )

        probs = [p for p, _, _ in self._pairs]
        labels = [y for _, y, _ in self._pairs]
        sample_count = len(probs)

        # Expected Calibration Error (10 bins).
        bins = 10
        ece = 0.0
        for idx in range(bins):
            lo = idx / bins
            hi = (idx + 1) / bins
            members = [i for i, p in enumerate(probs) if (lo <= p < hi) or (idx == bins - 1 and p == 1.0)]
            if not members:
                continue
            avg_prob = mean(probs[i] for i in members)
            avg_label = mean(labels[i] for i in members)
            ece += (len(members) / sample_count) * abs(avg_prob - avg_label)

        brier = mean((p - y) ** 2 for p, y in zip(probs, labels))
        hit_rate = mean(1.0 if (p >= 0.5) == bool(y) else 0.0 for p, y in zip(probs, labels))
        confidence_drift = abs(mean(probs) - mean(labels))

        return CalibrationStatus(
            sample_count=sample_count,
            ece=round(ece, 6),
            brier_score=round(brier, 6),
            hit_rate=round(hit_rate, 6),
            confidence_drift=round(confidence_drift, 6),
            updated_at=now.isoformat(),
        )


class _WalkForwardValidator:
    def evaluate(
        self,
        predictions: list[float],
        realized_returns: list[float],
        fold_size: int = 30,
        purge: int = 2,
        embargo: int = 2,
    ) -> WalkForwardReport:
        if len(predictions) != len(realized_returns):
            raise ValueError("predictions and realized_returns must have equal length")
        if fold_size < 5:
            raise ValueError("fold_size must be >= 5")

        n = len(predictions)
        folds: list[WalkForwardFold] = []
        idx = fold_size
        fold = 1
        while idx < n:
            test_start = idx
            test_end = min(n, idx + fold_size)
            train_end = max(0, test_start - purge)
            train_start = 0
            # embargo preserved conceptually by skipping next window start.
            test_probs = predictions[test_start:test_end]
            test_labels = [1 if x > 0 else 0 for x in realized_returns[test_start:test_end]]
            if test_probs:
                fold_hit = mean(1.0 if (p >= 0.5) == bool(y) else 0.0 for p, y in zip(test_probs, test_labels))
                fold_brier = mean((p - y) ** 2 for p, y in zip(test_probs, test_labels))
            else:
                fold_hit = 0.0
                fold_brier = 0.0
            folds.append(
                WalkForwardFold(
                    fold=fold,
                    train_start=train_start,
                    train_end=train_end,
                    test_start=test_start,
                    test_end=test_end,
                    hit_rate=round(fold_hit, 6),
                    brier_score=round(fold_brier, 6),
                )
            )
            fold += 1
            idx = test_end + embargo

        if folds:
            agg_hit = mean(f.hit_rate for f in folds)
            agg_brier = mean(f.brier_score for f in folds)
        else:
            agg_hit = 0.0
            agg_brier = 0.0

        return WalkForwardReport(
            folds=folds,
            aggregate_hit_rate=round(agg_hit, 6),
            aggregate_brier_score=round(agg_brier, 6),
            purge=purge,
            embargo=embargo,
            fold_size=fold_size,
            generated_at=datetime.now(tz=UTC).isoformat(),
        )


class _ChampionChallenger:
    def __init__(self) -> None:
        self._canary = CanaryGate()
        self._active_model = "champion-v1"
        self._last_evaluation: ChallengerEvaluation | None = None
        self._lineage = {
            "champion-v1": "ml-stack/champion-v1",
            "challenger-v2": "ml-stack/challenger-v2",
        }

    def evaluate(
        self,
        champion_scores: list[float],
        challenger_scores: list[float],
        expected_cost_delta_bps: float,
    ) -> ChallengerEvaluation:
        gate = self._canary.evaluate(champion_scores, challenger_scores)
        outcome = "hold"
        recommendation = "keep_champion"
        promoted = self._active_model

        cost_penalty = expected_cost_delta_bps > 6.0
        if gate.outcome == "pass" and not cost_penalty:
            outcome = "promote"
            recommendation = "promote_challenger"
            promoted = "challenger-v2"
            self._active_model = promoted
        elif gate.outcome == "borderline":
            outcome = "watch"
            recommendation = "extend_shadow_evaluation"
        elif gate.outcome == "fail":
            outcome = "reject"
            recommendation = "rollback_or_keep_champion"

        self._last_evaluation = ChallengerEvaluation(
            outcome=outcome,
            recommendation=recommendation,
            canary_action=gate.action,
            p_value=round(gate.p_value, 6),
            calibration_delta=round(gate.calibration_delta, 6),
            risk_band_shift=round(gate.risk_band_shift, 6),
            expected_cost_delta_bps=round(expected_cost_delta_bps, 4),
            promoted_model=promoted,
            evaluated_at=datetime.now(tz=UTC).isoformat(),
        )
        return self._last_evaluation

    def status(self) -> dict[str, object]:
        return {
            "active_model": self._active_model,
            "active_lineage": self._lineage.get(self._active_model, "unknown"),
            "last_evaluation": asdict(self._last_evaluation) if self._last_evaluation else None,
        }

    def lineage_for(self, model_version: str) -> str:
        return self._lineage.get(model_version, f"ml-stack/{model_version}")


class DecisionSupportEngine:
    def __init__(self) -> None:
        self._regime = _RegimeClassifier()
        self._calibration = _CalibrationMonitor()
        self._walk_forward = _WalkForwardValidator()
        self._cc = _ChampionChallenger()

    def _probability_triplet(self, base_score: float, regime: str) -> tuple[float, float, float]:
        up = _clamp(0.25 + 0.75 * base_score, 0.02, 0.95)
        down = _clamp(0.25 + 0.75 * (1.0 - base_score), 0.02, 0.95)
        flat = 1.0 - abs(base_score - 0.5) * 1.3
        if regime == "trend-up":
            up += 0.08
            down -= 0.05
        elif regime == "trend-down":
            down += 0.08
            up -= 0.05
        elif regime == "volatile-range":
            flat += 0.07
        elif regime == "thin-liquidity":
            flat += 0.05
            down += 0.03
        up = _clamp(up, 0.01, 0.96)
        flat = _clamp(flat, 0.01, 0.96)
        down = _clamp(down, 0.01, 0.96)
        total = up + flat + down
        return up / total, flat / total, down / total

    def _action_band(self, expected_return: float, confidence: float, downside_risk: float) -> str:
        if confidence < 0.45:
            return "hold-fire"
        if expected_return > 0.009 and downside_risk < 0.015:
            return "lean-long"
        if expected_return < -0.009 and downside_risk > 0.015:
            return "lean-short"
        return "watch"

    def _sizing_band(self, confidence: float, downside_risk: float) -> str:
        if confidence < 0.45 or downside_risk > 0.025:
            return "0.20x-0.40x"
        if confidence < 0.65:
            return "0.45x-0.70x"
        return "0.75x-1.00x"

    def generate(
        self,
        features: dict[str, float],
        model_version: str,
        confidence_floor: float = 0.55,
    ) -> DecisionSupportSnapshot:
        base = _clamp(mean(features.values()) if features else 0.5, 0.0, 1.0)
        regime = self._regime.classify(features)
        calib = self._calibration.status()
        prob_up, prob_flat, prob_down = self._probability_triplet(base, regime.regime)

        horizons = [("intraday", 1.0), ("swing", 0.72), ("position", 0.54)]
        out: list[HorizonDecision] = []
        for horizon, scale in horizons:
            expected = ((base - 0.5) * 0.045) * scale
            downside = (0.012 + abs(min(0.0, expected))) * (1.0 + regime.realized_vol_proxy * 0.8)
            confidence = _clamp(
                0.58
                + (abs(base - 0.5) * 0.55)
                - (regime.realized_vol_proxy * 0.30)
                - (calib.ece * 0.65),
                0.05,
                0.98,
            )
            out.append(
                HorizonDecision(
                    horizon=horizon,
                    expected_return=round(expected, 6),
                    downside_risk=round(downside, 6),
                    confidence=round(confidence, 6),
                    prob_up=round(prob_up, 6),
                    prob_flat=round(prob_flat, 6),
                    prob_down=round(prob_down, 6),
                    action_band=self._action_band(expected, confidence, downside),
                )
            )

        avg_conf = mean(h.confidence for h in out)
        avg_down = mean(h.downside_risk for h in out)
        confidence_gated = avg_conf < confidence_floor

        concentration_risk = "high" if prob_down > 0.52 and avg_down > 0.018 else "normal"
        advisory = PortfolioRiskAdvisory(
            exposure_band="defensive" if confidence_gated else "balanced",
            concentration_risk=concentration_risk,
            suggested_sizing_band=self._sizing_band(avg_conf, avg_down),
            max_single_position_pct=0.08 if concentration_risk == "high" else 0.12,
            stop_review_required=prob_down > 0.57,
        )

        return DecisionSupportSnapshot(
            model_version=model_version,
            model_lineage=self._cc.lineage_for(model_version),
            confidence_floor=round(confidence_floor, 6),
            confidence_gated=confidence_gated,
            calibration_error=calib.ece,
            confidence_drift=calib.confidence_drift,
            regime=regime,
            horizons=out,
            portfolio_risk_advisory=advisory,
            generated_at=datetime.now(tz=UTC).isoformat(),
        )

    def register_realized_outcome(self, prob_up: float, realized_return: float) -> CalibrationStatus:
        return self._calibration.update(prob_up=prob_up, realized_return=realized_return)

    def calibration_status(self) -> CalibrationStatus:
        return self._calibration.status()

    def walk_forward_validate(
        self,
        predictions: list[float],
        realized_returns: list[float],
        fold_size: int,
        purge: int,
        embargo: int,
    ) -> WalkForwardReport:
        return self._walk_forward.evaluate(
            predictions=predictions,
            realized_returns=realized_returns,
            fold_size=fold_size,
            purge=max(0, purge),
            embargo=max(0, embargo),
        )

    def evaluate_challenger(
        self,
        champion_scores: list[float],
        challenger_scores: list[float],
        expected_cost_delta_bps: float,
    ) -> ChallengerEvaluation:
        return self._cc.evaluate(
            champion_scores=champion_scores,
            challenger_scores=challenger_scores,
            expected_cost_delta_bps=expected_cost_delta_bps,
        )

    def challenger_status(self) -> dict[str, object]:
        return self._cc.status()
