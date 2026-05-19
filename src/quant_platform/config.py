from __future__ import annotations

import os
from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path


@dataclass
class JurisdictionPolicy:
    version: str = field(default_factory=lambda: os.getenv("JURISDICTION_POLICY_VERSION", "2026.02"))
    primary: str = field(default_factory=lambda: os.getenv("JURISDICTION_PRIMARY", "Iceland"))
    backup: str = field(default_factory=lambda: os.getenv("JURISDICTION_BACKUP", "Switzerland"))
    residency_map: dict[str, str] = field(
        default_factory=lambda: _default_residency_map(
            primary=os.getenv("JURISDICTION_PRIMARY", "Iceland"),
            backup=os.getenv("JURISDICTION_BACKUP", "Switzerland"),
        )
    )


@dataclass
class AuditConfig:
    sqlite_path: Path = field(default_factory=lambda: _path_from_env("AUDIT_SQLITE_PATH", "data/audit/audit_events.sqlite3"))
    object_store_path: Path = field(default_factory=lambda: _path_from_env("AUDIT_OBJECT_STORE_PATH", "data/audit/object_store.ndjson"))
    checkpoint_path: Path = field(default_factory=lambda: _path_from_env("AUDIT_CHECKPOINT_PATH", "data/checkpoints/off_provider_checkpoint.txt"))
    flush_interval_seconds: int = field(default_factory=lambda: _int_from_env("AUDIT_FLUSH_INTERVAL_SECONDS", 60))


@dataclass
class FeatureManifestConfig:
    expected_features: tuple[str, ...] = ("open", "high", "low", "close", "volume")
    stale_after: timedelta = field(default_factory=lambda: timedelta(hours=_int_from_env("FEATURE_STALE_AFTER_HOURS", 24)))


@dataclass
class CredentialConfig:
    overlap_hours: int = field(default_factory=lambda: _int_from_env("CREDENTIAL_OVERLAP_HOURS", 24))
    rotation_days: int = field(default_factory=lambda: _int_from_env("CREDENTIAL_ROTATION_DAYS", 90))


@dataclass
class SecurityConfig:
    model_signing_key: str = field(default_factory=lambda: os.getenv("MODEL_SIGNING_KEY", "dev-model-signing-key"))
    kill_switch_reason_secret: str = field(default_factory=lambda: os.getenv("KILL_SWITCH_REASON_SECRET", "dev-kill-secret"))


@dataclass
class NewsFeedConfig:
    max_items: int = field(default_factory=lambda: _int_from_env("NEWS_MAX_ITEMS", 50))
    timeout_secs: int = field(default_factory=lambda: _int_from_env("NEWS_TIMEOUT_SECS", 5))


@dataclass
class AppConfig:
    app_name: str = field(default_factory=lambda: os.getenv("APP_NAME", "Quant Research Platform v1.2"))
    disclaimer: str = field(default_factory=lambda: os.getenv("APP_DISCLAIMER", "Decision-support output only. Not investment advice."))
    jurisdiction: JurisdictionPolicy = field(default_factory=JurisdictionPolicy)
    audit: AuditConfig = field(default_factory=AuditConfig)
    features: FeatureManifestConfig = field(default_factory=FeatureManifestConfig)
    credentials: CredentialConfig = field(default_factory=CredentialConfig)
    security: SecurityConfig = field(default_factory=SecurityConfig)
    news: NewsFeedConfig = field(default_factory=NewsFeedConfig)


def _path_from_env(env_name: str, default: str) -> Path:
    raw = os.getenv(env_name, default)
    return Path(raw)


def _int_from_env(env_name: str, default: int) -> int:
    raw = os.getenv(env_name, str(default))
    try:
        value = int(raw)
    except ValueError:
        return default
    return value


def _default_residency_map(primary: str, backup: str) -> dict[str, str]:
    return {
        "serving": primary,
        "backup": backup,
        "audit_archive": backup,
        "checkpoint_digest": backup,
    }


def load_config() -> AppConfig:
    return AppConfig()

