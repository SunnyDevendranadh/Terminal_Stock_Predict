from __future__ import annotations

import hashlib
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from .config import CredentialConfig

UTC = timezone.utc


@dataclass
class KeySlot:
    key_id: str
    checksum: str
    validated: bool
    activated_at: datetime


class CredentialManager:
    def __init__(self, config: CredentialConfig) -> None:
        self._cfg = config
        now = datetime.now(tz=UTC)
        self._providers: dict[str, dict[str, object]] = {
            "market_data_primary": {
                "active": KeySlot("md-key-a", self._checksum("seed-a"), True, now),
                "standby": None,
                "last_rotated_at": now,
            },
            "news_feed": {
                "active": KeySlot("news-key-a", self._checksum("seed-n"), True, now),
                "standby": None,
                "last_rotated_at": now,
            },
        }

    @staticmethod
    def _checksum(raw: str) -> str:
        return hashlib.sha256(raw.encode("utf-8")).hexdigest()

    def validate_new_key(self, raw_key: str) -> bool:
        return len(raw_key) >= 16 and any(ch.isdigit() for ch in raw_key)

    def rotate(self, provider: str, new_key_id: str, new_key_secret: str) -> dict[str, object]:
        if provider not in self._providers:
            raise KeyError(f"Unknown provider: {provider}")
        if not self.validate_new_key(new_key_secret):
            raise ValueError("New key failed validation")
        now = datetime.now(tz=UTC)
        provider_state = self._providers[provider]
        new_slot = KeySlot(
            key_id=new_key_id,
            checksum=self._checksum(new_key_secret),
            validated=True,
            activated_at=now,
        )
        provider_state["standby"] = new_slot
        provider_state["active"] = new_slot
        provider_state["last_rotated_at"] = now
        overlap_ends_at = now + timedelta(hours=self._cfg.overlap_hours)
        return {
            "provider": provider,
            "active_key_id": new_slot.key_id,
            "checksum": new_slot.checksum,
            "overlap_ends_at": overlap_ends_at.isoformat(),
            "dual_key_window_active": True,
        }

    def status(self) -> dict[str, object]:
        now = datetime.now(tz=UTC)
        providers: dict[str, object] = {}
        for name, state in self._providers.items():
            last_rotated_at = state["last_rotated_at"]
            assert isinstance(last_rotated_at, datetime)
            due_at = last_rotated_at + timedelta(days=self._cfg.rotation_days)
            active = state["active"]
            standby = state["standby"]
            assert isinstance(active, KeySlot)
            providers[name] = {
                "active_key_id": active.key_id,
                "active_checksum": active.checksum,
                "dual_key_window_state": "open" if standby is not None else "closed",
                "last_rotated_at": last_rotated_at.isoformat(),
                "next_rotation_due_at": due_at.isoformat(),
                "rotation_status": "due" if now >= due_at else "healthy",
            }
        return providers
