from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from quant_platform.config import load_config
from quant_platform.main import QuantPlatformApp
from quant_platform.service import PlatformService


class NewsIntelEndpointTests(unittest.TestCase):
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

    def _request(self, method: str, path: str):
        return self.app.handle_request(method=method, path=path, headers={}, body={}, query={})

    def test_news_events_endpoint(self) -> None:
        with patch.object(
            self.service,
            "news_events_snapshot",
            return_value={"items": [{"event_id": "evt-1"}], "next_cursor": "", "count": 1, "total": 1},
        ) as mocked:
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/news/events",
                query={"symbols": ["AAPL,MSFT"], "severity": ["high"], "window": ["24h"], "limit": ["20"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["count"], 1)
            mocked.assert_called_once()

    def test_news_event_detail_claims_impact_timeline_endpoints(self) -> None:
        with patch.object(self.service, "news_event_snapshot", return_value={"event": {"event_id": "evt-1"}}):
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/news/events/evt-1",
                query={"symbols": ["AAPL"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["event"]["event_id"], "evt-1")

        with patch.object(self.service, "news_event_claims_snapshot", return_value={"items": [{"claim_id": "c1"}]}):
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/news/events/evt-1/claims",
                query={"symbols": ["AAPL"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["items"][0]["claim_id"], "c1")

        with patch.object(self.service, "news_event_impact_snapshot", return_value={"items": [{"horizon": "swing"}]}):
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/news/events/evt-1/impact",
                query={"symbols": ["AAPL"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["items"][0]["horizon"], "swing")

        with patch.object(self.service, "news_event_timeline_snapshot", return_value={"items": [{"status": "initial"}]}):
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/news/events/evt-1/timeline",
                query={"symbols": ["AAPL"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["items"][0]["status"], "initial")

    def test_focus_bundle_endpoint(self) -> None:
        status, _, body = self.app.handle_request(method="GET", path="/v1/focus/bundle", query={})
        self.assertEqual(status, 422)
        self.assertIn("symbol query param required", body["detail"])

        with patch.object(
            self.service,
            "focus_bundle_snapshot",
            return_value={"symbol": "AAPL", "timeframe": "5d", "event_markers": []},
        ) as mocked:
            status, _, body = self.app.handle_request(
                method="GET",
                path="/v1/focus/bundle",
                query={"symbol": ["AAPL"], "timeframe": ["5d"]},
            )
            self.assertEqual(status, 200)
            self.assertEqual(body["symbol"], "AAPL")
            mocked.assert_called_once()


if __name__ == "__main__":
    unittest.main()
