from __future__ import annotations

import unittest

from quant_platform.ml_decision_support import DecisionSupportEngine


class DecisionSupportEngineTests(unittest.TestCase):
    def setUp(self) -> None:
        self.engine = DecisionSupportEngine()

    def test_generate_contains_horizon_outputs(self) -> None:
        snapshot = self.engine.generate(
            features={"open": 0.49, "high": 0.65, "low": 0.40, "close": 0.61, "volume": 0.58},
            model_version="champion-v1",
            confidence_floor=0.55,
        )
        self.assertEqual(len(snapshot.horizons), 3)
        self.assertIn(snapshot.regime.regime, {"trend-up", "trend-down", "volatile-range", "mean-reversion", "thin-liquidity"})
        probs = snapshot.horizons[0].prob_up + snapshot.horizons[0].prob_flat + snapshot.horizons[0].prob_down
        self.assertAlmostEqual(probs, 1.0, places=5)

    def test_calibration_monitor_updates(self) -> None:
        before = self.engine.calibration_status()
        self.assertEqual(before.sample_count, 0)

        self.engine.register_realized_outcome(prob_up=0.70, realized_return=0.01)
        self.engine.register_realized_outcome(prob_up=0.30, realized_return=-0.02)
        after = self.engine.calibration_status()
        self.assertEqual(after.sample_count, 2)
        self.assertGreaterEqual(after.ece, 0.0)
        self.assertLessEqual(after.ece, 1.0)

    def test_walk_forward_with_embargo_and_purge(self) -> None:
        predictions = [0.55, 0.48, 0.63, 0.52, 0.47, 0.58, 0.62, 0.40, 0.53, 0.51, 0.49, 0.57]
        realized = [0.01, -0.01, 0.03, 0.00, -0.02, 0.01, 0.02, -0.01, 0.01, 0.00, -0.01, 0.02]
        report = self.engine.walk_forward_validate(
            predictions=predictions,
            realized_returns=realized,
            fold_size=5,
            purge=1,
            embargo=1,
        )
        self.assertGreaterEqual(len(report.folds), 1)
        self.assertGreaterEqual(report.aggregate_hit_rate, 0.0)
        self.assertLessEqual(report.aggregate_hit_rate, 1.0)

    def test_champion_challenger_evaluation(self) -> None:
        champion = [0.45, 0.52, 0.48, 0.51, 0.50, 0.47, 0.49, 0.53]
        challenger = [0.54, 0.59, 0.55, 0.58, 0.57, 0.56, 0.60, 0.61]
        result = self.engine.evaluate_challenger(
            champion_scores=champion,
            challenger_scores=challenger,
            expected_cost_delta_bps=2.0,
        )
        self.assertIn(result.outcome, {"promote", "watch", "hold", "reject"})
        status = self.engine.challenger_status()
        self.assertIn("active_model", status)


if __name__ == "__main__":
    unittest.main()
