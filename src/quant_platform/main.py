from __future__ import annotations

import argparse
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse

from .config import load_config
from .service import PlatformService


class QuantPlatformApp:
    def __init__(self, platform_service: PlatformService) -> None:
        self.service = platform_service
        self.config = platform_service.config

    def _base_headers(self) -> dict[str, str]:
        return {
            "Content-Type": "application/json",
            "X-Jurisdiction-Policy-Version": self.config.jurisdiction.version,
            "X-Jurisdiction-Primary": self.config.jurisdiction.primary,
            "X-Jurisdiction-Backup": self.config.jurisdiction.backup,
        }

    def handle_request(
        self,
        method: str,
        path: str,
        headers: dict[str, str] | None = None,
        body: dict[str, Any] | None = None,
        query: dict[str, list[str]] | None = None,
    ) -> tuple[int, dict[str, str], dict[str, Any]]:
        headers = {k.lower(): v for k, v in (headers or {}).items()}
        body = body or {}
        query = query or {}
        actor = headers.get("x-actor", "system")

        def _split_symbols(raw: str) -> list[str]:
            return [s.strip() for s in raw.split(",") if s.strip()]

        if method == "GET" and path == "/health":
            return 200, self._base_headers(), {"status": "ok"}

        if method == "POST" and path == "/v1/predict":
            try:
                result = self.service.predict(
                    features=body.get("features", {}),
                    model_version=body.get("model_version", "current"),
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except ValueError as exc:
                return 422, self._base_headers(), {"detail": str(exc)}
            except PermissionError as exc:
                return 503, self._base_headers(), {"detail": str(exc)}

        if method == "GET" and path == "/v1/ops/audit":
            return 200, self._base_headers(), self.service.ops_audit()

        if method == "GET" and path == "/v1/ops/credentials":
            return 200, self._base_headers(), self.service.ops_credentials()

        if method == "POST" and path == "/v1/ops/credentials/rotate":
            try:
                result = self.service.rotate_credentials(
                    provider=body["provider"],
                    new_key_id=body["new_key_id"],
                    new_key_secret=body["new_key_secret"],
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except (KeyError, ValueError) as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "POST" and path == "/v1/ops/kill-switch":
            try:
                result = self.service.set_kill_switch(
                    action=body["action"],
                    reason=body["reason"],
                    reason_signature=body["reason_signature"],
                    approvers=body.get("approvers", []),
                    actor=actor,
                    role=headers.get("x-role", "viewer"),
                    mfa_token=headers.get("x-mfa-token", ""),
                )
                return 200, self._base_headers(), result
            except PermissionError as exc:
                return 403, self._base_headers(), {"detail": str(exc)}
            except (KeyError, ValueError) as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "POST" and path == "/v1/ops/canary/evaluate":
            result = self.service.evaluate_canary(
                current_scores=body.get("current_scores", []),
                candidate_scores=body.get("candidate_scores", []),
                actor=actor,
            )
            return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/ops/compliance":
            return 200, self._base_headers(), self.service.compliance_summary()

        if method == "GET" and path == "/v1/ops/status":
            return 200, self._base_headers(), {
                "kill_switch_active": self.service.kill_switch.active,
                "kill_switch_level": self.service.kill_switch.state.level.value,
                "audit": self.service.ops_audit(),
            }

        if method == "GET" and path == "/v1/market-data":
            raw_symbols = query.get("symbols", [""])[0]
            symbols = _split_symbols(raw_symbols)
            if not symbols:
                return 422, self._base_headers(), {"detail": "symbols query param required"}
            result = self.service.market_data_snapshot(symbols)
            return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/market-data/history":
            symbol = query.get("symbol", [""])[0].strip()
            if not symbol:
                return 422, self._base_headers(), {"detail": "symbol query param required"}
            range_param = query.get("range", ["5d"])[0].strip() or "5d"
            interval_param = query.get("interval", ["1h"])[0].strip() or "1h"
            result = self.service.market_data_history_snapshot(symbol, range_param, interval_param)
            return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/news":
            raw_symbols = query.get("symbols", [""])[0]
            symbols = _split_symbols(raw_symbols)
            raw_max = query.get("max", ["50"])[0]
            try:
                max_items = int(raw_max)
            except ValueError:
                max_items = 50
            result = self.service.news_snapshot(symbols=symbols, max_items=max_items)
            return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/news/digest":
            symbol = query.get("symbol", [""])[0].strip()
            raw_max = query.get("max", ["20"])[0]
            try:
                max_items = int(raw_max)
            except ValueError:
                max_items = 20
            result = self.service.news_digest_snapshot(symbol=symbol, max_items=max_items)
            return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/news/events":
            symbols = _split_symbols(query.get("symbols", [""])[0])
            severity = query.get("severity", [""])[0].strip().lower()
            sentiment = query.get("sentiment", [""])[0].strip().lower()
            window = query.get("window", ["24h"])[0].strip() or "24h"
            cursor = query.get("cursor", [""])[0].strip()
            raw_limit = query.get("limit", ["25"])[0]
            try:
                limit = int(raw_limit)
            except ValueError:
                limit = 25
            result = self.service.news_events_snapshot(
                symbols=symbols,
                severity=severity,
                sentiment=sentiment,
                window=window,
                limit=limit,
                cursor=cursor,
                actor=actor,
            )
            return 200, self._base_headers(), result

        if method == "GET" and path.startswith("/v1/news/events/"):
            parts = [p for p in path.split("/") if p]
            # /v1/news/events/{event_id}
            # /v1/news/events/{event_id}/claims
            # /v1/news/events/{event_id}/impact
            # /v1/news/events/{event_id}/timeline
            if len(parts) >= 4 and parts[0] == "v1" and parts[1] == "news" and parts[2] == "events":
                event_id = parts[3]
                symbols = _split_symbols(query.get("symbols", [""])[0])
                if len(parts) == 4:
                    result = self.service.news_event_snapshot(event_id=event_id, symbols=symbols, actor=actor)
                    status = 404 if result.get("detail") == "event_not_found" else 200
                    return status, self._base_headers(), result
                if len(parts) == 5 and parts[4] == "claims":
                    result = self.service.news_event_claims_snapshot(event_id=event_id, symbols=symbols)
                    return 200, self._base_headers(), result
                if len(parts) == 5 and parts[4] == "impact":
                    result = self.service.news_event_impact_snapshot(event_id=event_id, symbols=symbols)
                    return 200, self._base_headers(), result
                if len(parts) == 5 and parts[4] == "timeline":
                    result = self.service.news_event_timeline_snapshot(event_id=event_id, symbols=symbols)
                    return 200, self._base_headers(), result

        if method == "GET" and path == "/v1/focus/bundle":
            symbol = query.get("symbol", [""])[0].strip()
            if not symbol:
                return 422, self._base_headers(), {"detail": "symbol query param required"}
            timeframe = query.get("timeframe", ["5d"])[0].strip() or "5d"
            result = self.service.focus_bundle_snapshot(symbol=symbol, timeframe=timeframe, actor=actor)
            return 200, self._base_headers(), result

        if method == "POST" and path == "/v1/ml/decision-support":
            features = body.get("features", {})
            model_version = body.get("model_version", "current")
            confidence_floor_raw = body.get("confidence_floor", 0.55)
            try:
                confidence_floor = float(confidence_floor_raw)
            except (TypeError, ValueError):
                confidence_floor = 0.55
            try:
                result = self.service.ml_decision_support_snapshot(
                    features=features,
                    model_version=model_version,
                    confidence_floor=confidence_floor,
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except ValueError as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "POST" and path == "/v1/ml/calibration/update":
            try:
                prob_up = float(body["prob_up"])
                realized_return = float(body["realized_return"])
                result = self.service.ml_calibration_update(
                    prob_up=prob_up,
                    realized_return=realized_return,
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except (KeyError, TypeError, ValueError) as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "GET" and path == "/v1/ml/calibration/status":
            return 200, self._base_headers(), self.service.ml_calibration_status()

        if method == "POST" and path == "/v1/ml/validation/walk-forward":
            try:
                predictions = [float(v) for v in body.get("predictions", [])]
                realized = [float(v) for v in body.get("realized_returns", [])]
                fold_size = int(body.get("fold_size", 30))
                purge = int(body.get("purge", 2))
                embargo = int(body.get("embargo", 2))
            except (TypeError, ValueError) as exc:
                return 422, self._base_headers(), {"detail": str(exc)}
            try:
                result = self.service.ml_walk_forward_snapshot(
                    predictions=predictions,
                    realized_returns=realized,
                    fold_size=fold_size,
                    purge=purge,
                    embargo=embargo,
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except ValueError as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "POST" and path == "/v1/ml/champion-challenger/evaluate":
            try:
                champion_scores = [float(v) for v in body.get("champion_scores", [])]
                challenger_scores = [float(v) for v in body.get("challenger_scores", [])]
                expected_cost_delta_bps = float(body.get("expected_cost_delta_bps", 0.0))
                result = self.service.ml_challenger_evaluate(
                    champion_scores=champion_scores,
                    challenger_scores=challenger_scores,
                    expected_cost_delta_bps=expected_cost_delta_bps,
                    actor=actor,
                )
                return 200, self._base_headers(), result
            except (TypeError, ValueError) as exc:
                return 422, self._base_headers(), {"detail": str(exc)}

        if method == "GET" and path == "/v1/ml/champion-challenger/status":
            return 200, self._base_headers(), self.service.ml_challenger_status()

        return 404, self._base_headers(), {"detail": "not_found"}


class _RequestHandler(BaseHTTPRequestHandler):
    app: QuantPlatformApp | None = None

    def _dispatch(self) -> None:
        if self.app is None:
            self.send_response(500)
            self.end_headers()
            return
        parsed = urlparse(self.path)
        query_params = parse_qs(parsed.query)
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b"{}"
        try:
            payload = json.loads(raw.decode("utf-8") or "{}")
        except json.JSONDecodeError:
            payload = {}
        status, headers, response_body = self.app.handle_request(
            method=self.command,
            path=parsed.path,
            headers={k: v for k, v in self.headers.items()},
            body=payload,
            query=query_params,
        )
        encoded = json.dumps(response_body, sort_keys=True).encode("utf-8")
        self.send_response(status)
        for key, value in headers.items():
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self) -> None:  # noqa: N802
        self._dispatch()

    def do_POST(self) -> None:  # noqa: N802
        self._dispatch()

    def log_message(self, _format: str, *_args: Any) -> None:
        return


def serve(host: str = "127.0.0.1", port: int = 8080) -> None:
    _RequestHandler.app = QuantPlatformApp(PlatformService(load_config()))
    server = ThreadingHTTPServer((host, port), _RequestHandler)
    try:
        server.serve_forever()
    finally:
        server.server_close()


def _int_from_env(name: str, default: int) -> int:
    raw = os.getenv(name, str(default))
    try:
        return int(raw)
    except ValueError:
        return default


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Quant Platform serving runtime")
    parser.add_argument(
        "--host",
        default=os.getenv("QP_HOST", "127.0.0.1"),
        help="Bind host (default from QP_HOST or 127.0.0.1)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=_int_from_env("QP_PORT", 8080),
        help="Bind port (default from QP_PORT or 8080)",
    )
    return parser.parse_args()


if __name__ == "__main__":
    args = parse_args()
    serve(host=args.host, port=args.port)
