from __future__ import annotations
import hashlib
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Protocol, Any

@dataclass
class MarketDataResult:
    symbol: str
    price: float           # latest close price (raw)
    change_pct: float      # % change vs previous close
    features: dict         # open/high/low/close/volume normalized 0-1
    fetched_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))


class MarketDataProvider(Protocol):
    def fetch(self, symbol: str) -> MarketDataResult | None:
        ...


class YFinanceProvider:
    """Fetches real OHLCV data via yfinance and normalises to [0,1]."""

    def fetch(self, symbol: str) -> MarketDataResult | None:
        try:
            import yfinance as yf
            ticker = yf.Ticker(symbol)
            hist = ticker.history(period="5d")
            if hist.empty or len(hist) < 2:
                return None

            # Use last row for current, second-to-last for prev close
            row = hist.iloc[-1]
            prev_close = hist.iloc[-2]["Close"]

            price = float(row["Close"])
            change_pct = ((price - prev_close) / prev_close) * 100.0 if prev_close else 0.0

            # 5-day range for normalisation
            hi5 = float(hist["High"].max())
            lo5 = float(hist["Low"].min())
            avg_vol = float(hist["Volume"].mean()) if hist["Volume"].mean() > 0 else 1.0

            rng = hi5 - lo5 if hi5 != lo5 else 1.0

            def clamp(v: float) -> float:
                return max(0.0, min(1.0, v))

            features = {
                "close": clamp((price - lo5) / rng),
                "volume": clamp(float(row["Volume"]) / avg_vol) if avg_vol > 0 else 0.5,
                "open": clamp(((float(row["Open"]) / prev_close) - 1.0 + 0.5) if prev_close else 0.5),
                "high": clamp((float(row["High"]) - price) / price if price else 0.0),
                "low": clamp(1.0 - (price - float(row["Low"])) / price if price else 0.5),
            }

            return MarketDataResult(
                symbol=symbol,
                price=price,
                change_pct=change_pct,
                features=features,
            )
        except Exception:
            return None


class StubProvider:
    """Deterministic provider — no network, suitable for tests and fallback."""

    def fetch(self, symbol: str) -> MarketDataResult | None:
        h = int(hashlib.md5(symbol.encode()).hexdigest(), 16)
        def pseudo(salt: int) -> float:
            return ((h ^ (salt * 2654435761)) % 1000) / 1000.0

        price = 50.0 + (h % 9500) / 100.0
        change_pct = ((h % 2001) - 1000) / 100.0  # -10 to +10
        features = {
            "open":   pseudo(1),
            "high":   max(pseudo(2), 0.1),
            "low":    min(pseudo(3), 0.9),
            "close":  pseudo(4),
            "volume": pseudo(5),
        }
        return MarketDataResult(symbol=symbol, price=price, change_pct=change_pct, features=features)


@dataclass
class PricePoint:
    ts: str           # ISO-8601
    price: float
    volume: float


@dataclass
class Candle:
    ts: str
    open: float
    high: float
    low: float
    close: float
    volume: float


@dataclass
class HistoryResult:
    symbol: str
    timeframe: str    # "1d" | "5d" | "1mo"
    points: list
    candles: list
    meta: dict        # {range, interval, count, trend_label, change_pct}


def _trend_label(closes: list[float]) -> str:
    """Compute trend from linear slope of closes."""
    n = len(closes)
    if n < 2:
        return "Range"
    # Simple slope: (last - first) / first as a percentage
    change = (closes[-1] - closes[0]) / closes[0] * 100.0 if closes[0] else 0.0
    if change > 0.5:
        return "Uptrend"
    if change < -0.5:
        return "Downtrend"
    return "Range"


def fetch_history(
    symbol: str,
    range: str = "5d",
    interval: str = "1h",
) -> "HistoryResult | None":
    """Fetch OHLCV history for a symbol; returns None on any failure."""
    try:
        import yfinance as yf
        ticker = yf.Ticker(symbol)
        hist = ticker.history(period=range, interval=interval)
        if hist is None or hist.empty:
            return None

        candles: list[Candle] = []
        points: list[PricePoint] = []

        for idx, row in hist.iterrows():
            ts = idx.isoformat() if hasattr(idx, "isoformat") else str(idx)
            c = Candle(
                ts=ts,
                open=float(row.get("Open", 0.0)),
                high=float(row.get("High", 0.0)),
                low=float(row.get("Low", 0.0)),
                close=float(row.get("Close", 0.0)),
                volume=float(row.get("Volume", 0.0)),
            )
            candles.append(c)
            points.append(PricePoint(ts=ts, price=c.close, volume=c.volume))

        closes = [c.close for c in candles]
        trend = _trend_label(closes)
        change_pct = 0.0
        if len(closes) >= 2 and closes[0]:
            change_pct = (closes[-1] - closes[0]) / closes[0] * 100.0

        meta: dict[str, Any] = {
            "range": range,
            "interval": interval,
            "count": len(candles),
            "trend_label": trend,
            "change_pct": round(change_pct, 4),
        }

        return HistoryResult(
            symbol=symbol,
            timeframe=range,
            points=points,
            candles=candles,
            meta=meta,
        )
    except Exception:
        return None


def fetch_batch(
    symbols: list[str],
    provider: MarketDataProvider,
    max_workers: int = 8,
) -> dict[str, MarketDataResult]:
    """Fetch all symbols concurrently; swallow per-symbol errors."""
    results: dict[str, MarketDataResult] = {}
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = {pool.submit(provider.fetch, sym): sym for sym in symbols}
        for fut in as_completed(futures):
            sym = futures[fut]
            try:
                result = fut.result()
                if result is not None:
                    results[sym] = result
            except Exception:
                pass
    return results
