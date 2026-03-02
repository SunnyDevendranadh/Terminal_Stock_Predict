from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from enum import Enum

UTC = timezone.utc


class KillLevel(str, Enum):
    OFF = "off"
    SOFT = "soft"
    HARD = "hard"


@dataclass
class KillSwitchState:
    level: KillLevel = KillLevel.OFF
    reason: str = ""
    updated_at: datetime = datetime.now(tz=UTC)


class KillSwitch:
    def __init__(self) -> None:
        self._state = KillSwitchState()

    def activate_soft(self, reason: str) -> KillSwitchState:
        self._state = KillSwitchState(
            level=KillLevel.SOFT,
            reason=reason,
            updated_at=datetime.now(tz=UTC),
        )
        return self._state

    def activate_hard(self, reason: str) -> KillSwitchState:
        self._state = KillSwitchState(
            level=KillLevel.HARD,
            reason=reason,
            updated_at=datetime.now(tz=UTC),
        )
        return self._state

    def deactivate_hard(self, reason: str, approvers: list[str]) -> KillSwitchState:
        unique = {a.strip() for a in approvers if a.strip()}
        if len(unique) < 2:
            raise ValueError("Hard kill requires two-person approval")
        self._state = KillSwitchState(
            level=KillLevel.OFF,
            reason=reason,
            updated_at=datetime.now(tz=UTC),
        )
        return self._state

    def clear_soft(self, reason: str) -> KillSwitchState:
        self._state = KillSwitchState(
            level=KillLevel.OFF,
            reason=reason,
            updated_at=datetime.now(tz=UTC),
        )
        return self._state

    @property
    def state(self) -> KillSwitchState:
        return self._state

    @property
    def active(self) -> bool:
        return self._state.level in {KillLevel.SOFT, KillLevel.HARD}
