from __future__ import annotations

import hashlib
import math
import re
from dataclasses import asdict, dataclass
from datetime import datetime, timedelta, timezone
from typing import Protocol

from .news_feed import NewsDigestItem, fetch_news_digest

UTC = timezone.utc


@dataclass
class EventEvidence:
    source: str
    url: str
    published_at: str
    quote: str
    reliability: float


@dataclass
class EventClaim:
    claim_id: str
    event_id: str
    claim_type: str
    claim_text: str
    verification_status: str
    confidence: float
    evidence: list[EventEvidence]


@dataclass
class EventImpact:
    event_id: str
    horizon: str
    prob_up: float
    prob_flat: float
    prob_down: float
    expected_return: float
    downside_risk: float
    confidence: float


@dataclass
class EventTimelineEntry:
    event_id: str
    timestamp: str
    status: str
    change_note: str


@dataclass
class EventSummary:
    event_id: str
    symbol: str
    title: str
    severity: str
    novelty_score: float
    contradiction_score: float
    confidence: float
    sentiment: str
    published_at: str
    article_count: int
    stale: bool
    why_it_matters: str


@dataclass
class EventIntel:
    event: EventSummary
    claims: list[EventClaim]
    impacts: list[EventImpact]
    timeline: list[EventTimelineEntry]
    citations: list[str]
    what_changed_since_last_update: str


@dataclass
class EventMarker:
    event_id: str
    ts: str
    severity: str
    headline: str


@dataclass
class ConfidencePoint:
    ts: str
    lower: float
    upper: float


@dataclass
class ImpactProbability:
    horizon: str
    prob_up: float
    prob_flat: float
    prob_down: float


class NewsProvider(Protocol):
    def fetch(self, symbols: list[str], max_items: int, timeout_secs: int) -> list[NewsDigestItem]:
        ...


class RSSDigestProvider:
    def fetch(self, symbols: list[str], max_items: int, timeout_secs: int) -> list[NewsDigestItem]:
        return fetch_news_digest(symbols=symbols, max_items=max_items, timeout_secs=timeout_secs)


def _parse_iso(value: str) -> datetime:
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return datetime.now(tz=UTC)


def _normalize_headline(headline: str) -> str:
    cleaned = re.sub(r"\s+", " ", headline.lower()).strip()
    cleaned = re.sub(r"[^a-z0-9\s]", "", cleaned)
    words = cleaned.split()
    return " ".join(words[:8])


def _window_from_text(window: str) -> timedelta:
    w = window.strip().lower()
    if w.endswith("h"):
        try:
            hours = int(w[:-1])
        except ValueError:
            return timedelta(hours=24)
        return timedelta(hours=max(1, hours))
    if w.endswith("d"):
        try:
            days = int(w[:-1])
        except ValueError:
            return timedelta(days=7)
        return timedelta(days=max(1, days))
    return timedelta(hours=24)


def _severity(headline: str, sentiment: str) -> str:
    text = headline.lower()
    critical_tokens = ("fraud", "default", "bankruptcy", "investigation", "halt", "lawsuit")
    high_tokens = ("downgrade", "guidance cut", "miss", "layoff", "merger", "acquisition")
    medium_tokens = ("beat", "upgrade", "guidance", "rally", "selloff")

    if any(t in text for t in critical_tokens):
        return "critical"
    if any(t in text for t in high_tokens):
        return "high"
    if any(t in text for t in medium_tokens):
        return "medium"
    if sentiment == "negative":
        return "medium"
    return "low"


def _claim_type(headline: str) -> str:
    text = headline.lower()
    if "earn" in text or "eps" in text:
        return "earnings"
    if "guidance" in text:
        return "guidance"
    if "merger" in text or "acquisition" in text:
        return "mna"
    if "lawsuit" in text or "investigation" in text:
        return "legal"
    if "downgrade" in text or "upgrade" in text:
        return "analyst"
    return "market"


def _safe_clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


class NewsIntelEngine:
    def __init__(
        self,
        provider: NewsProvider | None = None,
        timeout_secs: int = 5,
    ) -> None:
        self._provider = provider if provider is not None else RSSDigestProvider()
        self._timeout_secs = timeout_secs
        self._source_reliability: dict[str, float] = {
            "Yahoo Finance": 0.72,
            "MarketWatch": 0.74,
            "Seeking Alpha": 0.68,
        }
        self._severity_rank: dict[str, int] = {
            "critical": 4,
            "high": 3,
            "medium": 2,
            "low": 1,
        }

    def _group_events(self, items: list[NewsDigestItem]) -> dict[str, list[NewsDigestItem]]:
        groups: dict[str, list[NewsDigestItem]] = {}
        for item in items:
            symbol = item.symbol if item.symbol else "MARKET"
            norm = _normalize_headline(item.headline)
            key = f"{symbol}|{norm}"
            groups.setdefault(key, []).append(item)
        return groups

    def _event_id(self, key: str) -> str:
        return hashlib.sha1(key.encode("utf-8")).hexdigest()[:16]

    def _contradiction_score(self, event_items: list[NewsDigestItem]) -> float:
        sentiments = {it.sentiment for it in event_items}
        if "positive" in sentiments and "negative" in sentiments:
            return 0.8
        if len(sentiments) > 1:
            return 0.35
        return 0.0

    def _confidence(self, event_items: list[NewsDigestItem], contradiction_score: float) -> float:
        reliability = 0.0
        for it in event_items:
            reliability += self._source_reliability.get(it.source, 0.60)
        mean_rel = reliability / max(1, len(event_items))
        corroboration = 1.0 - math.exp(-0.6 * len(event_items))
        confidence = mean_rel * (0.55 + 0.45 * corroboration) * (1.0 - 0.6 * contradiction_score)
        return round(_safe_clamp(confidence, 0.0, 0.99), 4)

    def _summary_for_event(self, event_items: list[NewsDigestItem]) -> EventSummary:
        ordered = sorted(event_items, key=lambda x: x.published_at, reverse=True)
        latest = ordered[0]
        event_key = f"{latest.symbol if latest.symbol else 'MARKET'}|{_normalize_headline(latest.headline)}"
        event_id = self._event_id(event_key)
        contradiction = self._contradiction_score(ordered)
        confidence = self._confidence(ordered, contradiction)
        first_ts = _parse_iso(ordered[-1].published_at)
        novelty_seconds = (datetime.now(tz=UTC) - first_ts).total_seconds()
        novelty = round(_safe_clamp(1.0 - novelty_seconds / (24 * 3600), 0.0, 1.0), 4)
        severity = _severity(latest.headline, latest.sentiment)
        stale = (datetime.now(tz=UTC) - _parse_iso(latest.published_at)) > timedelta(hours=24)

        why = latest.why_it_matters
        if confidence < 0.5:
            why = f"Low-confidence event: {why}"
        if contradiction > 0.6:
            why = f"Conflicting-source event: {why}"

        return EventSummary(
            event_id=event_id,
            symbol=latest.symbol,
            title=latest.headline,
            severity=severity,
            novelty_score=novelty,
            contradiction_score=round(contradiction, 4),
            confidence=confidence,
            sentiment=latest.sentiment,
            published_at=latest.published_at,
            article_count=len(ordered),
            stale=stale,
            why_it_matters=why,
        )

    def _claims_for_event(self, summary: EventSummary, event_items: list[NewsDigestItem]) -> list[EventClaim]:
        claims: list[EventClaim] = []
        for idx, item in enumerate(sorted(event_items, key=lambda x: x.published_at, reverse=True), start=1):
            claim_id = f"{summary.event_id}-c{idx}"
            evidence = EventEvidence(
                source=item.source,
                url=item.url,
                published_at=item.published_at,
                quote=item.headline,
                reliability=self._source_reliability.get(item.source, 0.60),
            )
            status = "verified" if summary.confidence >= 0.65 and summary.article_count >= 2 else "unverified"
            if summary.contradiction_score >= 0.6:
                status = "contradicted"
            claims.append(
                EventClaim(
                    claim_id=claim_id,
                    event_id=summary.event_id,
                    claim_type=_claim_type(item.headline),
                    claim_text=item.summary,
                    verification_status=status,
                    confidence=summary.confidence,
                    evidence=[evidence],
                )
            )
        return claims

    def _impact_for_event(self, summary: EventSummary) -> list[EventImpact]:
        base = {
            "positive": (0.58, 0.30, 0.12, 0.012),
            "negative": (0.16, 0.29, 0.55, -0.014),
            "neutral": (0.33, 0.45, 0.22, 0.001),
        }
        prob_up, prob_flat, prob_down, exp = base.get(summary.sentiment, base["neutral"])

        sev_boost = {"critical": 0.08, "high": 0.05, "medium": 0.02, "low": 0.0}[summary.severity]
        if summary.sentiment == "negative":
            prob_down = _safe_clamp(prob_down + sev_boost, 0.0, 0.92)
            prob_up = _safe_clamp(prob_up - sev_boost / 2, 0.0, 0.9)
            exp -= sev_boost * 0.06
        elif summary.sentiment == "positive":
            prob_up = _safe_clamp(prob_up + sev_boost, 0.0, 0.92)
            prob_down = _safe_clamp(prob_down - sev_boost / 2, 0.0, 0.9)
            exp += sev_boost * 0.06

        norm = prob_up + prob_flat + prob_down
        prob_up, prob_flat, prob_down = prob_up / norm, prob_flat / norm, prob_down / norm

        horizons = (("intraday", 1.00), ("swing", 0.72), ("position", 0.54))
        impacts: list[EventImpact] = []
        for horizon, scale in horizons:
            scaled_up = _safe_clamp(prob_up * (0.92 + 0.08 * scale), 0.0, 1.0)
            scaled_flat = _safe_clamp(prob_flat, 0.0, 1.0)
            scaled_down = _safe_clamp(prob_down * (1.08 - 0.08 * scale), 0.0, 1.0)
            norm = max(1e-9, scaled_up + scaled_flat + scaled_down)
            impacts.append(
                EventImpact(
                    event_id=summary.event_id,
                    horizon=horizon,
                    prob_up=round(scaled_up / norm, 4),
                    prob_flat=round(scaled_flat / norm, 4),
                    prob_down=round(scaled_down / norm, 4),
                    expected_return=round(exp * scale, 4),
                    downside_risk=round(abs(exp) * (1.1 if summary.sentiment == "negative" else 0.8), 4),
                    confidence=summary.confidence,
                )
            )
        return impacts

    def _timeline_for_event(self, summary: EventSummary, event_items: list[NewsDigestItem]) -> list[EventTimelineEntry]:
        ordered = sorted(event_items, key=lambda x: x.published_at)
        entries: list[EventTimelineEntry] = []
        for idx, item in enumerate(ordered, start=1):
            status = "initial" if idx == 1 else "update"
            entries.append(
                EventTimelineEntry(
                    event_id=summary.event_id,
                    timestamp=item.published_at,
                    status=status,
                    change_note=f"{item.source}: {item.headline}",
                )
            )
        return entries

    def _event_from_group(self, event_items: list[NewsDigestItem]) -> EventIntel:
        summary = self._summary_for_event(event_items)
        claims = self._claims_for_event(summary, event_items)
        impacts = self._impact_for_event(summary)
        timeline = self._timeline_for_event(summary, event_items)
        citations = [f"{c.evidence[0].source}::{c.evidence[0].url}" for c in claims if c.evidence]
        if len(timeline) > 1:
            delta = f"{len(timeline) - 1} follow-up update(s) after initial publication"
        else:
            delta = "Initial publication only"

        return EventIntel(
            event=summary,
            claims=claims,
            impacts=impacts,
            timeline=timeline,
            citations=citations,
            what_changed_since_last_update=delta,
        )

    def _all_events(self, symbols: list[str], max_items: int, window: str) -> list[EventIntel]:
        items = self._provider.fetch(symbols=symbols, max_items=max_items, timeout_secs=self._timeout_secs)
        groups = self._group_events(items)
        events = [self._event_from_group(group_items) for group_items in groups.values()]

        cutoff = datetime.now(tz=UTC) - _window_from_text(window)
        events = [e for e in events if _parse_iso(e.event.published_at) >= cutoff]
        events.sort(
            key=lambda e: (
                self._severity_rank.get(e.event.severity, 0),
                e.event.contradiction_score,
                e.event.novelty_score,
                e.event.confidence,
            ),
            reverse=True,
        )
        return events

    def list_events(
        self,
        symbols: list[str],
        severity: str,
        sentiment: str,
        window: str,
        limit: int,
        cursor: str,
    ) -> dict[str, object]:
        events = self._all_events(symbols=symbols, max_items=max(50, limit * 3), window=window)

        if severity:
            events = [e for e in events if e.event.severity == severity]
        if sentiment:
            events = [e for e in events if e.event.sentiment == sentiment]

        offset = 0
        if cursor:
            try:
                offset = max(0, int(cursor))
            except ValueError:
                offset = 0
        page = events[offset : offset + limit]
        next_cursor = str(offset + limit) if offset + limit < len(events) else ""

        return {
            "items": [asdict(e.event) for e in page],
            "next_cursor": next_cursor,
            "count": len(page),
            "total": len(events),
        }

    def get_event(self, event_id: str, symbols: list[str], window: str = "7d") -> EventIntel | None:
        events = self._all_events(symbols=symbols, max_items=120, window=window)
        for event in events:
            if event.event.event_id == event_id:
                return event
        return None

    def focus_overlay(self, symbol: str, timeframe: str) -> dict[str, object]:
        events = self._all_events(symbols=[symbol] if symbol else [], max_items=60, window="7d")
        markers = [
            EventMarker(
                event_id=e.event.event_id,
                ts=e.event.published_at,
                severity=e.event.severity,
                headline=e.event.title,
            )
            for e in events[:12]
        ]

        top = events[0].impacts if events else []
        impact_probs = [
            ImpactProbability(
                horizon=i.horizon,
                prob_up=i.prob_up,
                prob_flat=i.prob_flat,
                prob_down=i.prob_down,
            )
            for i in top
        ]

        # Simple model confidence ribbon template, terminal charts can align by index.
        confidence_band = [
            ConfidencePoint(ts=m.ts, lower=0.35, upper=0.75)
            for m in markers
        ]

        return {
            "event_markers": [asdict(m) for m in markers],
            "confidence_band": [asdict(c) for c in confidence_band],
            "impact_probabilities": [asdict(p) for p in impact_probs],
        }

# News intelligence: NLP pipeline for sentiment extraction and entity recognition
