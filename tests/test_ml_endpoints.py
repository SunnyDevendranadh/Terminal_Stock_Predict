from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from quant_platform.config import load_config
from quant_platform.main import QuantPlatformApp
from quant_platform.service import PlatformService


class MlEndpointTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        cfg = load_config()
        base = Path(self.tempdir.name)
        cfg.audit.sqlite_path = base / "audit" / "audit_events.sqlite3"
        cfg.audit.object_store_path = base / "audit" / "object_store.ndjson"
        cfg.audit.checkpoint_path = base / "checkpoints" / "checkpoint.txt"
        self.service = PlatformService(cfg)
        self.app = QuantPlatformApp(self.service)

    def tearDown(self) -> None:
        self.service.audit.close()
        self.tempdir.cleanup()

    def test_ml_decision_support_endpoint(self) -> None:
        status, _, body = self.app.handle_request(
            method="POST",
            path="/v1/ml/decision-support",
            body={
                "features": {"open": 0.4, "high": 0.7, "low": 0.3, "close": 0.6, "volume": 0.5},
                "model_version": "champion-v1",
                "confidence_floor": 0.55,
            },
        )
        self.assertEqual(status, 200)
        self.assertIn("horizons", body)
        self.assertIn("portfolio_risk_advisory", body)

    def test_ml_calibration_endpoints(self) -> None:
        status, _, _ = self.app.handle_request(
            method="POST",
            path="/v1/ml/calibration/update",
            body={"prob_up": 0.66, "realized_return": 0.015},
        )
        self.assertEqual(status, 200)

        status, _, body = self.app.handle_request(
            method="GET",
            path="/v1/ml/calibration/status",
        )
        self.assertEqual(status, 200)
        self.assertGreaterEqual(body["sample_count"], 1)

    def test_walk_forward_and_challenger_endpoints(self) -> None:
        status, _, wf = self.app.handle_request(
            method="POST",
            path="/v1/ml/validation/walk-forward",
            body={
                "predictions": [0.55, 0.5, 0.45, 0.6, 0.52, 0.48, 0.51, 0.57, 0.53, 0.49],
                "realized_returns": [0.01, -0.01, -0.02, 0.02, 0.01, -0.01, 0.0, 0.02, 0.01, -0.01],
                "fold_size": 5,
                "purge": 1,
                "embargo": 1,
            },
        )
        self.assertEqual(status, 200)
        self.assertIn("folds", wf)

        status, _, cc = self.app.handle_request(
            method="POST",
            path="/v1/ml/champion-challenger/evaluate",
            body={
                "champion_scores": [0.49, 0.5, 0.52, 0.47, 0.51, 0.5],
                "challenger_scores": [0.58, 0.56, 0.6, 0.57, 0.59, 0.58],
                "expected_cost_delta_bps": 3.0,
            },
        )
        self.assertEqual(status, 200)
        self.assertIn("outcome", cc)

        status, _, status_body = self.app.handle_request(
            method="GET",
            path="/v1/ml/champion-challenger/status",
        )
        self.assertEqual(status, 200)
        self.assertIn("active_model", status_body)


if __name__ == "__main__":
    unittest.main()
