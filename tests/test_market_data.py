import unittest
from unittest.mock import patch, MagicMock
from quant_platform.market_data import StubProvider, fetch_batch, MarketDataResult


class ErrorProvider:
    """A provider that always raises an exception."""
    def fetch(self, symbol: str):
        raise RuntimeError(f"simulated failure for {symbol}")


class MarketDataTests(unittest.TestCase):

    def test_stub_provider_returns_valid_features(self):
        """StubProvider returns a result with all 5 required features in [0, 1]."""
        provider = StubProvider()
        result = provider.fetch("AAPL")
        self.assertIsNotNone(result)
        self.assertEqual(result.symbol, "AAPL")
        required = {"open", "high", "low", "close", "volume"}
        self.assertEqual(set(result.features.keys()), required)
        for key, val in result.features.items():
            self.assertGreaterEqual(val, 0.0, f"{key} below 0")
            self.assertLessEqual(val, 1.0, f"{key} above 1")

    def test_fetch_batch_partial_failures(self):
        """fetch_batch returns partial results when some symbols fail."""
        class MixedProvider:
            def fetch(self, symbol: str):
                if symbol == "BAD":
                    raise RuntimeError("simulated error")
                return StubProvider().fetch(symbol)

        results = fetch_batch(["AAPL", "BAD"], MixedProvider())
        self.assertIn("AAPL", results)
        self.assertNotIn("BAD", results)

    def test_normalisation_clamps_to_unit_interval(self):
        """All feature values from StubProvider are in [0.0, 1.0]."""
        provider = StubProvider()
        symbols = ["BTCUSD", "ETHUSD", "SPY", "NVDA", "GLD"]
        for sym in symbols:
            result = provider.fetch(sym)
            self.assertIsNotNone(result, f"No result for {sym}")
            for key, val in result.features.items():
                self.assertGreaterEqual(val, 0.0, f"{sym}/{key} below 0: {val}")
                self.assertLessEqual(val, 1.0, f"{sym}/{key} above 1: {val}")

    def test_stub_provider_is_deterministic(self):
        """StubProvider returns same result for same symbol."""
        provider = StubProvider()
        r1 = provider.fetch("TSLA")
        r2 = provider.fetch("TSLA")
        self.assertEqual(r1.price, r2.price)
        self.assertEqual(r1.features, r2.features)


if __name__ == "__main__":
    unittest.main()
