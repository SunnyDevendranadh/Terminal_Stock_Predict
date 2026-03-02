"""Tests for news_feed.fetch_news_digest() and helpers."""
from __future__ import annotations

import unittest
from unittest.mock import patch

from quant_platform.news_feed import (
    NewsItem,
    NewsDigestItem,
    fetch_news_digest,
    _generate_summary,
    _why_it_matters,
    _extract_glossary,
)


class NewsDigestTests(unittest.TestCase):

    def _make_news_item(
        self,
        headline: str = "Test headline",
        source: str = "TestSource",
        url: str = "https://example.com",
        published_at: str = "2024-01-01T00:00:00+00:00",
        symbol: str = "AAPL",
        sentiment: str = "neutral",
    ) -> NewsItem:
        return NewsItem(
            headline=headline,
            source=source,
            url=url,
            published_at=published_at,
            symbol=symbol,
            sentiment=sentiment,
        )

    def test_digest_enriches_summary(self):
        """fetch_news_digest should produce NewsDigestItem with non-empty summary."""
        items = [self._make_news_item(headline="Apple earnings beat estimates", sentiment="positive")]

        with patch("quant_platform.news_feed.fetch_news", return_value=items):
            result = fetch_news_digest(["AAPL"], max_items=5, timeout_secs=1)

        self.assertEqual(len(result), 1)
        self.assertIsInstance(result[0], NewsDigestItem)
        self.assertTrue(len(result[0].summary) > 0, "summary must be non-empty")
        self.assertTrue(len(result[0].why_it_matters) > 0, "why_it_matters must be non-empty")

    def test_digest_why_it_matters_positive(self):
        """Positive sentiment item → why_it_matters should mention 'higher'."""
        text = _why_it_matters("positive")
        self.assertIn("higher", text.lower(),
            "positive why_it_matters should mention 'higher'")

    def test_glossary_extraction(self):
        """Headline containing 'EPS beat' should match glossary terms 'eps' and 'beat'."""
        headline = "Apple EPS beat analyst expectations"
        terms = _extract_glossary(headline)
        self.assertIn("eps", terms, "should detect 'eps' in headline")
        self.assertIn("beat", terms, "should detect 'beat' in headline")


if __name__ == "__main__":
    unittest.main()
