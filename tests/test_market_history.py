"""Tests for market_data.fetch_history()."""
from __future__ import annotations

import unittest
from unittest.mock import patch, MagicMock
import pandas as pd
from datetime import datetime, timezone


class MarketHistoryTests(unittest.TestCase):

    def _make_hist_df(self, closes: list[float]) -> "pd.DataFrame":
        """Build a minimal yfinance-style DataFrame."""
        n = len(closes)
        index = pd.date_range("2024-01-01", periods=n, freq="1h", tz="UTC")
        data = {
            "Open":   [c * 0.99 for c in closes],
            "High":   [c * 1.01 for c in closes],
            "Low":    [c * 0.98 for c in closes],
            "Close":  closes,
            "Volume": [1_000_000.0] * n,
        }
        return pd.DataFrame(data, index=index)

    def test_fetch_history_returns_candles(self):
        from quant_platform.market_data import fetch_history

        closes = [100.0, 101.0, 102.0, 103.0, 104.0]
        df = self._make_hist_df(closes)

        mock_ticker = MagicMock()
        mock_ticker.history.return_value = df

        with patch("yfinance.Ticker", return_value=mock_ticker):
            result = fetch_history("AAPL", range="5d", interval="1h")

        self.assertIsNotNone(result)
        self.assertEqual(result.symbol, "AAPL")
        self.assertEqual(result.timeframe, "5d")
        self.assertEqual(len(result.candles), 5)
        for candle in result.candles:
            self.assertGreaterEqual(candle.high, candle.low,
                "high must be >= low for every candle")
        self.assertEqual(len(result.points), 5)

    def test_fetch_history_trend_uptrend(self):
        from quant_platform.market_data import fetch_history

        # Monotonically rising closes → Uptrend
        closes = [100.0, 101.0, 102.5, 104.0, 106.0]
        df = self._make_hist_df(closes)

        mock_ticker = MagicMock()
        mock_ticker.history.return_value = df

        with patch("yfinance.Ticker", return_value=mock_ticker):
            result = fetch_history("SPY", range="5d", interval="1h")

        self.assertIsNotNone(result)
        self.assertEqual(result.meta["trend_label"], "Uptrend",
            "Rising closes should produce Uptrend label")

    def test_fetch_history_error_returns_none(self):
        from quant_platform.market_data import fetch_history

        mock_ticker = MagicMock()
        mock_ticker.history.side_effect = RuntimeError("network error")

        with patch("yfinance.Ticker", return_value=mock_ticker):
            result = fetch_history("AAPL")

        self.assertIsNone(result, "fetch_history must return None on yfinance exception")


if __name__ == "__main__":
    unittest.main()
