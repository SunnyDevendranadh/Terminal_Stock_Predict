from __future__ import annotations

import hashlib
import hmac
import tempfile
import unittest
from pathlib import Path

from quant_platform.config import load_config
from quant_platform.main import QuantPlatformApp
from quant_platform.service import PlatformService


class PlanControlTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        cfg = load_config()
        base = Path(self.tempdir.name)
        cfg.audit.sqlite_path = base / "audit" / "audit_events.sqlite3"
        cfg.audit.object_store_path = base / "audit" / "object_store.ndjson"
        cfg.audit.checkpoint_path = base / "checkpoints" / "checkpoint.txt"
        self.config = cfg
        self.service = PlatformService(cfg)
        self.app = QuantPlatformApp(self.service)

    def tearDown(self) -> None:
        self.service.audit.close()
        self.tempdir.cleanup()

    def _request(self, method: str, path: str, headers=None, body=None):
        return self.app.handle_request(method=method, path=path, headers=headers or {}, body=body or {})

    def _reason_signature(self, reason: str) -> str:
        return hmac.new(
            self.config.security.kill_switch_reason_secret.encode("utf-8"),
            reason.encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()

    def test_audit_durability_and_chain_integrity(self) -> None:
        for _ in range(20):
            status, _, _ = self._request(
                "POST",
                "/v1/predict",
                body={"features": {"open": 0.2, "high": 0.3, "low": 0.1, "close": 0.4, "volume": 0.5}},
            )
            self.assertEqual(status, 200)

        _, _, audit = self._request("GET", "/v1/ops/audit")
        self.assertLessEqual(audit["audit_rpo_seconds"], 60)
        self.assertEqual(audit["chain_verification_status"], "valid")

    def test_alerting_fallback_channel(self) -> None:
        self.service.alerts.primary_available = False
        result = self.service.alerts.send("critical", "primary out")
        self.assertTrue(result.delivered)
        self.assertEqual(result.channel, "encrypted_email_fallback")

    def test_jurisdiction_headers_and_residency_map(self) -> None:
        status, headers, body = self._request("GET", "/v1/ops/compliance")
        self.assertEqual(status, 200)
        self.assertEqual(headers["X-Jurisdiction-Primary"], "Iceland")
        self.assertEqual(body["jurisdiction_backup"], "Switzerland")
        self.assertEqual(body["data_residency_map"]["serving"], "Iceland")

    def test_credential_rotation_zero_downtime_state(self) -> None:
        status, _, before = self._request("GET", "/v1/ops/credentials")
        self.assertEqual(status, 200)
        self.assertIn("market_data_primary", before)

        status, _, payload = self._request(
            "POST",
            "/v1/ops/credentials/rotate",
            body={
                "provider": "market_data_primary",
                "new_key_id": "md-key-b",
                "new_key_secret": "new-secret-value-12345",
            },
        )
        self.assertEqual(status, 200)
        self.assertTrue(payload["dual_key_window_active"])

        _, _, after = self._request("GET", "/v1/ops/credentials")
        self.assertEqual(after["market_data_primary"]["active_key_id"], "md-key-b")

    def test_canary_shift_fails_and_requests_rollback(self) -> None:
        current = [0.05, 0.08, 0.1, 0.11, 0.13, 0.09, 0.07, 0.12] * 4
        candidate = [0.92, 0.95, 0.9, 0.89, 0.93, 0.96, 0.91, 0.94] * 4
        status, _, data = self._request(
            "POST",
            "/v1/ops/canary/evaluate",
            body={"current_scores": current, "candidate_scores": candidate},
        )
        self.assertEqual(status, 200)
        self.assertEqual(data["outcome"], "fail")
        self.assertEqual(data["action"], "automatic_rollback_and_incident")

    def test_hard_kill_blocks_predictions_but_ops_live(self) -> None:
        reason = "integrity breach"
        status, _, data = self._request(
            "POST",
            "/v1/ops/kill-switch",
            headers={"x-role": "ops-admin", "x-mfa-token": "123456"},
            body={
                "action": "hard_on",
                "reason": reason,
                "reason_signature": self._reason_signature(reason),
                "approvers": ["operator-a", "operator-b"],
            },
        )
        self.assertEqual(status, 200)
        self.assertEqual(data["level"], "hard")

        status, _, _ = self._request(
            "POST",
            "/v1/predict",
            body={"features": {"open": 0.2, "high": 0.3, "low": 0.1, "close": 0.4, "volume": 0.5}},
        )
        self.assertEqual(status, 503)

        status, _, _ = self._request("GET", "/v1/ops/compliance")
        self.assertEqual(status, 200)

        clear_reason = "incident closed"
        status, _, data = self._request(
            "POST",
            "/v1/ops/kill-switch",
            headers={"x-role": "ops-admin", "x-mfa-token": "123456"},
            body={
                "action": "hard_off",
                "reason": clear_reason,
                "reason_signature": self._reason_signature(clear_reason),
                "approvers": ["operator-a", "operator-b"],
            },
        )
        self.assertEqual(status, 200)
        self.assertEqual(data["level"], "off")

    def test_checkpoint_file_exists_after_audit_ops(self) -> None:
        status, _, _ = self._request("GET", "/v1/ops/audit")
        self.assertEqual(status, 200)
        self.assertTrue(self.config.audit.checkpoint_path.exists())


if __name__ == "__main__":
    unittest.main()
