import unittest
from unittest.mock import patch, MagicMock
from io import BytesIO
from quant_platform.news_feed import (
    _simple_sentiment,
    _match_symbol,
    fetch_news,
    NewsItem,
)


MINIMAL_RSS = b"""<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <item>
      <title>AAPL earnings beat expectations</title>
      <link>https://example.com/aapl</link>
      <pubDate>Tue, 01 Jan 2030 12:00:00 GMT</pubDate>
      <description>Apple reported strong quarterly results.</description>
    </item>
    <item>
      <title>Market faces massive loss fears</title>
      <link>https://example.com/market</link>
      <pubDate>Tue, 01 Jan 2030 11:00:00 GMT</pubDate>
      <description>Investors worry about declining profits.</description>
    </item>
  </channel>
</rss>"""


class NewsFeedTests(unittest.TestCase):

    def test_sentiment_positive_keywords(self):
        self.assertEqual(_simple_sentiment("Stock surges to record high"), "positive")

    def test_sentiment_negative_keywords(self):
        self.assertEqual(_simple_sentiment("Company reports massive loss"), "negative")

    def test_sentiment_neutral(self):
        self.assertEqual(_simple_sentiment("Market opens today"), "neutral")

    def test_symbol_matching(self):
        result = _match_symbol("AAPL earnings beat", ["AAPL", "MSFT"])
        self.assertEqual(result, "AAPL")

    def test_symbol_no_match(self):
        result = _match_symbol("Market overview for today", ["AAPL"])
        self.assertEqual(result, "")

    def test_symbol_case_insensitive(self):
        # symbols list contains uppercase; text may contain lowercase
        result = _match_symbol("aapl reported earnings", ["AAPL", "MSFT"])
        self.assertEqual(result, "AAPL")

    def test_fetch_news_with_mock_urllib(self):
        """fetch_news parses mock RSS XML and returns NewsItem objects."""
        mock_response = MagicMock()
        mock_response.read.return_value = MINIMAL_RSS
        mock_response.__enter__.return_value = mock_response
        mock_response.__exit__.return_value = False

        with patch("urllib.request.urlopen", return_value=mock_response):
            # Patch RSS_SOURCES to only use one source so we call urlopen once per source
            with patch("quant_platform.news_feed.RSS_SOURCES", [("Test", "https://test.example.com/rss")]):
                items = fetch_news(["AAPL", "MSFT"], max_items=50)

        self.assertGreater(len(items), 0)
        headlines = [item.headline for item in items]
        self.assertIn("AAPL earnings beat expectations", headlines)

        # Find the AAPL item
        aapl_item = next(i for i in items if "AAPL" in i.headline)
        self.assertEqual(aapl_item.source, "Test")
        self.assertEqual(aapl_item.url, "https://example.com/aapl")
        self.assertEqual(aapl_item.symbol, "AAPL")
        # "beat" is a positive word, "strong" is a positive word — should be positive
        self.assertEqual(aapl_item.sentiment, "positive")


if __name__ == "__main__":
    unittest.main()
