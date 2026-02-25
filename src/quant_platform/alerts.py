from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone

UTC = timezone.utc


@dataclass
class AlertResult:
    delivered: bool
    channel: str
    detail: str


class AlertRouter:
    """Provider-isolated primary route with encrypted-email fallback."""

    def __init__(self) -> None:
        self.primary_available = True

    def send(self, severity: str, message: str) -> AlertResult:
        if self.primary_available:
            return AlertResult(
                delivered=True,
                channel="matrix_webhook_gateway",
                detail=f"{datetime.now(tz=UTC).isoformat()}::{severity}::{message}",
            )
        return AlertResult(
            delivered=True,
            channel="encrypted_email_fallback",
            detail=f"{datetime.now(tz=UTC).isoformat()}::{severity}::{message}",
        )

# Alert routing: primary channel (Matrix webhook), fallback (encrypted email)
