from __future__ import annotations

import math
from dataclasses import dataclass


@dataclass
class CanaryThresholds:
    alpha: float = 0.01
    fail_calibration_delta: float = 0.07
    borderline_calibration_delta: float = 0.04
    fail_risk_shift: float = 0.12
    borderline_risk_shift: float = 0.06
    risk_cutoff: float = 0.8


@dataclass
class CanaryResult:
    outcome: str
    action: str
    p_value: float
    calibration_delta: float
    risk_band_shift: float


def _rank(values: list[tuple[float, int]]) -> list[float]:
    ranks = [0.0] * len(values)
    i = 0
    while i < len(values):
        j = i
        while j + 1 < len(values) and values[j + 1][0] == values[i][0]:
            j += 1
        avg_rank = (i + j + 2) / 2.0
        for k in range(i, j + 1):
            ranks[k] = avg_rank
        i = j + 1
    return ranks


def mann_whitney_u_pvalue(sample_a: list[float], sample_b: list[float]) -> float:
    if not sample_a or not sample_b:
        return 1.0
    pooled = [(v, 0) for v in sample_a] + [(v, 1) for v in sample_b]
    pooled.sort(key=lambda x: x[0])
    ranks = _rank(pooled)

    n1 = len(sample_a)
    n2 = len(sample_b)
    rank_sum_a = sum(rank for rank, (_, group) in zip(ranks, pooled) if group == 0)
    u1 = rank_sum_a - n1 * (n1 + 1) / 2.0

    mean_u = n1 * n2 / 2.0

    tie_counts: dict[float, int] = {}
    for value, _ in pooled:
        tie_counts[value] = tie_counts.get(value, 0) + 1
    tie_term = sum(count**3 - count for count in tie_counts.values())
    n = n1 + n2
    if n <= 1:
        return 1.0
    variance_u = n1 * n2 / 12.0 * ((n + 1) - tie_term / (n * (n - 1)))
    if variance_u <= 0:
        return 1.0

    z = (u1 - mean_u) / math.sqrt(variance_u)
    p_two_sided = math.erfc(abs(z) / math.sqrt(2.0))
    return max(0.0, min(1.0, p_two_sided))


class CanaryGate:
    def __init__(self, thresholds: CanaryThresholds | None = None) -> None:
        self._t = thresholds or CanaryThresholds()

    def evaluate(self, current_scores: list[float], candidate_scores: list[float]) -> CanaryResult:
        p_value = mann_whitney_u_pvalue(current_scores, candidate_scores)
        current_mean = sum(current_scores) / max(1, len(current_scores))
        candidate_mean = sum(candidate_scores) / max(1, len(candidate_scores))
        calibration_delta = abs(candidate_mean - current_mean)

        current_risk = sum(1 for x in current_scores if x >= self._t.risk_cutoff) / max(1, len(current_scores))
        candidate_risk = sum(1 for x in candidate_scores if x >= self._t.risk_cutoff) / max(1, len(candidate_scores))
        risk_shift = abs(candidate_risk - current_risk)

        drift_fail = p_value < self._t.alpha
        cal_fail = calibration_delta >= self._t.fail_calibration_delta
        risk_fail = risk_shift >= self._t.fail_risk_shift
        cal_border = calibration_delta >= self._t.borderline_calibration_delta
        risk_border = risk_shift >= self._t.borderline_risk_shift

        if drift_fail and (cal_fail or risk_fail):
            return CanaryResult(
                outcome="fail",
                action="automatic_rollback_and_incident",
                p_value=p_value,
                calibration_delta=calibration_delta,
                risk_band_shift=risk_shift,
            )
        if drift_fail or cal_border or risk_border:
            return CanaryResult(
                outcome="borderline",
                action="extend_canary_window",
                p_value=p_value,
                calibration_delta=calibration_delta,
                risk_band_shift=risk_shift,
            )
        return CanaryResult(
            outcome="pass",
            action="continue_canary_ramp",
            p_value=p_value,
            calibration_delta=calibration_delta,
            risk_band_shift=risk_shift,
        )
