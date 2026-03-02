from __future__ import annotations
import re
import urllib.request
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Optional

@dataclass
class NewsItem:
    headline: str
    source: str
    url: str
    published_at: str   # ISO-8601
    symbol: str         # matched ticker, empty = general market
    sentiment: str      # "positive" | "negative" | "neutral"


GLOSSARY_MAP: dict[str, str] = {
    "eps": "Earnings Per Share — profit per share",
    "guidance": "Company's own forecast for future performance",
    "downgrade": "Analyst reduced their rating",
    "upgrade": "Analyst raised their rating",
    "layoff": "Company reducing workforce, often signals cost pressure",
    "beat": "Results exceeded analyst expectations",
    "miss": "Results fell short of expectations",
    "rally": "Rapid price increase over a short period",
    "selloff": "Rapid price decrease driven by high selling volume",
    "ipo": "Initial Public Offering — first time shares sold publicly",
    "merger": "Two companies combining into one entity",
    "acquisition": "One company buying another",
    "dividend": "Cash payment from company profits to shareholders",
    "default": "Company failing to repay debt obligations",
    "short": "Bet that a stock will fall in price",
}


@dataclass
class NewsDigestItem:
    headline: str
    source: str
    url: str
    published_at: str
    symbol: str
    sentiment: str
    summary: str
    why_it_matters: str
    glossary_terms: list


RSS_SOURCES: list[tuple[str, str]] = [
    ("Yahoo Finance",  "https://finance.yahoo.com/news/rssindex"),
    ("MarketWatch",    "https://feeds.marketwatch.com/marketwatch/topstories"),
    ("Seeking Alpha",  "https://seekingalpha.com/feed.xml"),
]

POSITIVE_WORDS = frozenset({
    "surge", "surges", "surged", "rally", "rallies", "rallied",
    "gain", "gains", "gained", "rise", "rises", "rose",
    "beat", "beats", "exceeded", "record", "high", "profit",
    "growth", "upgrade", "buy", "bullish", "soar", "soars",
    "strong", "robust", "positive", "outperform", "success",
})

NEGATIVE_WORDS = frozenset({
    "fall", "falls", "fell", "drop", "drops", "dropped",
    "loss", "losses", "decline", "declines", "declined",
    "miss", "misses", "missed", "crash", "crashes", "crashed",
    "cut", "cuts", "downgrade", "sell", "bearish", "weak",
    "warn", "warning", "risk", "debt", "default", "layoff",
    "layoffs", "lawsuit", "fraud", "investigation",
})


def _simple_sentiment(text: str) -> str:
    words = set(re.findall(r"\b\w+\b", text.lower()))
    pos = len(words & POSITIVE_WORDS)
    neg = len(words & NEGATIVE_WORDS)
    if pos > neg:
        return "positive"
    if neg > pos:
        return "negative"
    return "neutral"


def _match_symbol(text: str, symbols: list[str]) -> str:
    text_upper = text.upper()
    for sym in symbols:
        if re.search(rf"\b{re.escape(sym)}\b", text_upper):
            return sym
    return ""


def _parse_rss_date(date_str: str) -> str:
    """Try to parse common RSS date formats; return ISO-8601 string."""
    if not date_str:
        return datetime.now(timezone.utc).isoformat()
    for fmt in (
        "%a, %d %b %Y %H:%M:%S %z",
        "%a, %d %b %Y %H:%M:%S GMT",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%SZ",
    ):
        try:
            dt = datetime.strptime(date_str.strip(), fmt)
            if dt.tzinfo is None:
                dt = dt.replace(tzinfo=timezone.utc)
            return dt.isoformat()
        except ValueError:
            continue
    return datetime.now(timezone.utc).isoformat()


def fetch_news(
    symbols: list[str],
    max_items: int = 50,
    timeout_secs: int = 5,
) -> list[NewsItem]:
    """Fetch RSS news from configured sources; deduplicate; sort newest-first."""
    items: list[NewsItem] = []
    seen_headlines: set[str] = set()

    for source_name, url in RSS_SOURCES:
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "terminal-client/1.0"})
            with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
                xml_bytes = resp.read()
            root = ET.fromstring(xml_bytes)

            # Handle both RSS 2.0 (<channel><item>) and Atom feeds
            ns = {"atom": "http://www.w3.org/2005/Atom"}
            rss_items = root.findall(".//item") or root.findall(".//atom:entry", ns)

            for entry in rss_items:
                def text(tag: str, ns_tag: str = "") -> str:
                    el = entry.find(tag)
                    if el is None and ns_tag:
                        el = entry.find(ns_tag, ns)
                    return (el.text or "").strip() if el is not None else ""

                headline = text("title") or text("atom:title", "atom:title")
                if not headline or headline in seen_headlines:
                    continue
                seen_headlines.add(headline)

                link = text("link") or text("atom:link", "atom:link")
                pub_date = text("pubDate") or text("published") or text("atom:published", "atom:published")

                combined = f"{headline} {text('description')}"
                symbol = _match_symbol(combined, symbols)
                sentiment = _simple_sentiment(combined)

                items.append(NewsItem(
                    headline=headline,
                    source=source_name,
                    url=link,
                    published_at=_parse_rss_date(pub_date),
                    symbol=symbol,
                    sentiment=sentiment,
                ))
        except Exception:
            continue  # skip failed sources

    # Sort newest-first
    items.sort(key=lambda x: x.published_at, reverse=True)
    return items[:max_items]


def _generate_summary(headline: str, symbol: str, sentiment: str) -> str:
    subject = symbol if symbol else "the market"
    hl = headline.lower()
    if sentiment == "positive":
        return f"This story reports positive news for {subject}: {hl}"
    if sentiment == "negative":
        return f"This story flags a risk: {hl}"
    return f"Latest market update: {hl}"


def _why_it_matters(sentiment: str) -> str:
    if sentiment == "positive":
        return "Positive news can attract buyers, pushing the stock price higher."
    if sentiment == "negative":
        return "Negative news may cause investors to sell, pushing the price lower."
    return "Monitor this development; it may affect future price direction."


def _extract_glossary(text: str) -> list[str]:
    text_lower = text.lower()
    return [term for term in GLOSSARY_MAP if re.search(rf"\b{re.escape(term)}\b", text_lower)]


def fetch_news_digest(
    symbols: list[str],
    max_items: int = 20,
    timeout_secs: int = 5,
) -> list[NewsDigestItem]:
    """Fetch news and enrich each item with summary, why_it_matters, and glossary_terms."""
    raw = fetch_news(symbols, max_items, timeout_secs)
    result: list[NewsDigestItem] = []
    for item in raw:
        combined_text = item.headline
        result.append(NewsDigestItem(
            headline=item.headline,
            source=item.source,
            url=item.url,
            published_at=item.published_at,
            symbol=item.symbol,
            sentiment=item.sentiment,
            summary=_generate_summary(item.headline, item.symbol, item.sentiment),
            why_it_matters=_why_it_matters(item.sentiment),
            glossary_terms=_extract_glossary(combined_text),
        ))
    return result
