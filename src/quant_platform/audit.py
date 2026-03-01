from __future__ import annotations

import hashlib
import hmac
import json
import sqlite3
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .config import AuditConfig

UTC = timezone.utc


def _utc_now() -> datetime:
    return datetime.now(tz=UTC)


def _iso(dt: datetime) -> str:
    return dt.astimezone(UTC).isoformat()


def _sha256(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


@dataclass
class AuditEvent:
    event_type: str
    actor: str
    payload: dict[str, Any]
    created_at: datetime
    event_hash: str
    prev_hash: str
    signature: str


class AuditStore:
    """Append-only audit storage with DB + object-store style batched flush."""

    def __init__(self, config: AuditConfig, signing_key: str) -> None:
        self._cfg = config
        self._signing_key = signing_key.encode("utf-8")
        self._last_event_hash = "GENESIS"
        self._last_flush_at = _utc_now()
        self._buffer: list[AuditEvent] = []
        self._cfg.sqlite_path.parent.mkdir(parents=True, exist_ok=True)
        self._cfg.object_store_path.parent.mkdir(parents=True, exist_ok=True)
        self._cfg.checkpoint_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self._cfg.sqlite_path, check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._initialize()
        self._load_last_hash()

    def _initialize(self) -> None:
        self._conn.execute(
            """
            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                actor TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL,
                event_hash TEXT NOT NULL UNIQUE,
                prev_hash TEXT NOT NULL,
                signature TEXT NOT NULL
            )
            """
        )
        self._conn.commit()

    def _load_last_hash(self) -> None:
        row = self._conn.execute(
            "SELECT event_hash FROM audit_events ORDER BY id DESC LIMIT 1"
        ).fetchone()
        if row:
            self._last_event_hash = str(row["event_hash"])

    def append(self, event_type: str, actor: str, payload: dict[str, Any]) -> AuditEvent:
        created_at = _utc_now()
        payload_text = json.dumps(payload, sort_keys=True, separators=(",", ":"))
        canonical = f"{event_type}|{actor}|{_iso(created_at)}|{payload_text}|{self._last_event_hash}"
        event_hash = _sha256(canonical)
        signature = hmac.new(self._signing_key, event_hash.encode("utf-8"), hashlib.sha256).hexdigest()
        event = AuditEvent(
            event_type=event_type,
            actor=actor,
            payload=payload,
            created_at=created_at,
            event_hash=event_hash,
            prev_hash=self._last_event_hash,
            signature=signature,
        )
        self._persist_db(event)
        self._buffer.append(event)
        self._last_event_hash = event.event_hash
        if (created_at - self._last_flush_at).total_seconds() >= self._cfg.flush_interval_seconds:
            self.flush_object_store()
        return event

    def _persist_db(self, event: AuditEvent) -> None:
        self._conn.execute(
            """
            INSERT INTO audit_events (event_type, actor, payload, created_at, event_hash, prev_hash, signature)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            """,
            (
                event.event_type,
                event.actor,
                json.dumps(event.payload, sort_keys=True),
                _iso(event.created_at),
                event.event_hash,
                event.prev_hash,
                event.signature,
            ),
        )
        self._conn.commit()

    def flush_object_store(self) -> dict[str, Any]:
        if not self._buffer:
            return {
                "flushed": False,
                "reason": "empty_buffer",
                "last_flush_at": _iso(self._last_flush_at),
            }
        manifest_time = _utc_now()
        lines = [
            {
                "event_type": e.event_type,
                "actor": e.actor,
                "payload": e.payload,
                "created_at": _iso(e.created_at),
                "event_hash": e.event_hash,
                "prev_hash": e.prev_hash,
                "signature": e.signature,
            }
            for e in self._buffer
        ]
        aggregate = _sha256("|".join(entry["event_hash"] for entry in lines))
        manifest = {
            "manifest_created_at": _iso(manifest_time),
            "start_hash": lines[0]["prev_hash"],
            "end_hash": lines[-1]["event_hash"],
            "count": len(lines),
            "aggregate_hash": aggregate,
        }
        manifest["manifest_signature"] = hmac.new(
            self._signing_key,
            json.dumps(manifest, sort_keys=True).encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()
        with self._cfg.object_store_path.open("a", encoding="utf-8") as f:
            f.write(json.dumps({"events": lines, "manifest": manifest}, sort_keys=True) + "\n")
        self._buffer.clear()
        self._last_flush_at = manifest_time
        return {"flushed": True, "manifest": manifest}

    def create_checkpoint(self) -> dict[str, Any]:
        row = self._conn.execute(
            "SELECT event_hash, created_at FROM audit_events ORDER BY id DESC LIMIT 1"
        ).fetchone()
        if not row:
            checkpoint_hash = "GENESIS"
            created_at = _iso(_utc_now())
        else:
            checkpoint_hash = str(row["event_hash"])
            created_at = str(row["created_at"])
        digest_body = {
            "created_at": created_at,
            "checkpoint_hash": checkpoint_hash,
        }
        digest_body["signature"] = hmac.new(
            self._signing_key,
            json.dumps(digest_body, sort_keys=True).encode("utf-8"),
            hashlib.sha256,
        ).hexdigest()
        self._cfg.checkpoint_path.write_text(
            json.dumps(digest_body, sort_keys=True), encoding="utf-8"
        )
        return digest_body

    def verify_chain(self) -> dict[str, Any]:
        rows = self._conn.execute(
            "SELECT event_type, actor, payload, created_at, event_hash, prev_hash, signature FROM audit_events ORDER BY id ASC"
        ).fetchall()
        expected_prev = "GENESIS"
        for idx, row in enumerate(rows, start=1):
            canonical = (
                f"{row['event_type']}|{row['actor']}|{row['created_at']}|"
                f"{json.dumps(json.loads(row['payload']), sort_keys=True, separators=(',', ':'))}|{row['prev_hash']}"
            )
            expected_hash = _sha256(canonical)
            if row["prev_hash"] != expected_prev or row["event_hash"] != expected_hash:
                return {
                    "valid": False,
                    "failed_index": idx,
                    "expected_prev": expected_prev,
                    "seen_prev": row["prev_hash"],
                }
            expected_sig = hmac.new(
                self._signing_key,
                str(row["event_hash"]).encode("utf-8"),
                hashlib.sha256,
            ).hexdigest()
            if str(row["signature"]) != expected_sig:
                return {
                    "valid": False,
                    "failed_index": idx,
                    "reason": "signature_mismatch",
                }
            expected_prev = str(row["event_hash"])
        return {
            "valid": True,
            "count": len(rows),
            "last_hash": expected_prev,
        }

    def ops_summary(self) -> dict[str, Any]:
        checkpoint = {}
        if self._cfg.checkpoint_path.exists():
            checkpoint = json.loads(self._cfg.checkpoint_path.read_text(encoding="utf-8"))
        verification = self.verify_chain()
        return {
            "last_checkpoint_at": checkpoint.get("created_at"),
            "chain_verification_status": "valid" if verification.get("valid") else "invalid",
            "audit_rpo_seconds": self._cfg.flush_interval_seconds,
            "last_event_hash": verification.get("last_hash", "GENESIS"),
            "events_buffered": len(self._buffer),
        }

    def close(self) -> None:
        self._conn.close()


# Audit chain: each event includes hash of previous event for tamper detection
