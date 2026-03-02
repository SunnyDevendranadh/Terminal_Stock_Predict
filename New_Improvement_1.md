# Upgrade Plan: Focus Mode + Newbie-Friendly News Navigation for the Stock Terminal

## Summary
Build a **balanced-user** terminal upgrade with:
- A new **Focus Mode** for deep ticker analysis.
- **Both chart modes** (line + candlestick toggle).
- A **Guided Digest** news experience so newer users can understand market context quickly.
- MVP scope on the **current stack** (Python API + Rust gateway + Rust TUI), no paid providers.

This plan keeps the current architecture, adds minimal new contracts, and fixes existing data-shape issues that currently weaken fallback behavior.

## Research Inputs (used to shape UX)
- TradingView chart mode + interval workflows: [TradingView Chart basics](https://www.tradingview.com/support/categories/chart/)
- thinkorswim chart + watchlist/news navigation model: [Charting in thinkorswim](https://www.schwab.com/learn/story/charts-what-to-look-for) and [Using thinkorswim platform](https://www.schwab.com/learn/story/getting-started-with-thinkorswim)
- ratatui capabilities for terminal chart rendering: [ratatui widgets docs](https://docs.rs/ratatui/latest/ratatui/widgets/index.html)
- Existing data-source capability for historical bars: [yfinance docs](https://ranaroussi.github.io/yfinance/reference/api/yfinance.Ticker.history.html)

## Current-State Gaps to Address
- No dedicated focus workflow (current `Signal Detail` is static and non-charted).
- News view is list-only; no guided explanation or educational drilldown.
- Polling fallback has response-shape mismatch in Rust client:
  - `/v1/market-data` is treated like an array but backend returns symbol-keyed object.
  - `/v1/news` is treated like `Vec<NewsItem>` but backend returns `{ "items": [...] }`.
- No retained per-symbol time-series suitable for trend/candle rendering.
- No first-run or beginner guidance layer.

## Public API / Interface / Type Changes

### Python HTTP API additions
- `GET /v1/market-data/history`
- Query: `symbol`, `range` (`1d|5d|1mo`), `interval` (`1m|5m|15m|1h`)
- Response: `{ "symbol": "...", "points": [...], "candles": [...], "meta": {...} }`
- `GET /v1/news/digest`
- Query: `symbol`, `max`
- Response: `{ "items": [{headline, source, url, published_at, symbol, sentiment, summary, why_it_matters, glossary_terms}] }`

### gRPC proto additions (`proto/quant_platform.proto`)
- Add `GetFocusBundle(FocusBundleRequest) returns (FocusBundleResponse)`.
- Add messages:
  - `FocusBundleRequest { symbol, timeframe, actor }`
  - `PricePoint { ts, price, volume }`
  - `Candle { ts, open, high, low, close, volume }`
  - `NewsDigestItem { existing news fields + summary + why_it_matters + glossary_terms }`
  - `FocusBundleResponse { symbol, timeframe, line_series, candles, news_digest, trend_label, change_pct, captured_at }`

### Rust client app types
- Add:
  - `View::Focus`
  - `ChartMode` (`Line`, `Candle`)
  - `FocusState` (`symbol`, `timeframe`, `chart_mode`, selected news index, glossary_enabled)
  - Cache tables for price/news digest history.

## Implementation Plan

## 1. Data Layer and Contracts
- Implement Python `market_data_history(symbol, range, interval)` using `yfinance.Ticker.history`.
- Normalize history response into both:
  - line series (`ts`, `close`, `volume`)
  - candle series (`OHLCV`)
- Implement Python `news_digest(symbol, max)`:
  - reuse existing RSS fetch + sentiment.
  - generate `summary` and `why_it_matters` via deterministic templates.
  - attach `glossary_terms` from keyword mapping (`EPS`, `guidance`, `downgrade`, `layoff`, etc.).
- Extend Rust gateway:
  - Add `GetFocusBundle` handler.
  - Fetch history + digest from Python endpoints.
  - Return typed response through gRPC.
- Fix existing polling-shape bugs in Rust client while touching data layer.

## 2. Focus Mode UX (Terminal)
- Add new view tab: `Focus`.
- Entry behavior:
  - `f` opens Focus for currently selected symbol.
  - `[` and `]` cycle symbols.
- Chart behavior:
  - `c` toggles `Line`/`Candle`.
  - `t` cycles timeframe (`1d`, `5d`, `1mo`).
  - Show trend label (`Uptrend`, `Range`, `Downtrend`) from slope + recent volatility.
- Focus layout:
  - Top: ticker strip and key stats.
  - Main-left: chart canvas.
  - Main-right: model signal explanation in plain language.
  - Bottom: symbol-specific digest headlines with selection.
- Empty/error states:
  - `No history yet`, `History fetch failed`, `Using cached data`.

## 3. News Navigation and Newbie Guidance
- Upgrade News view to split-pane:
  - Left pane: filterable headline list.
  - Right pane: digest details (`summary`, `why it matters`, glossary chips).
- Add filters:
  - symbol (`All` or selected ticker),
  - sentiment (`All|Positive|Negative|Neutral`),
  - recency (`1h|24h|7d`).
- Add navigation:
  - `Up/Down` move list.
  - `Enter` toggle expanded detail.
  - `/` quick filter input.
  - `g` toggle glossary helper panel.
  - `?` show in-app keymap cheat sheet.
- Beginner UX defaults:
  - Guided text on by default.
  - First-run hint overlay for focus/news controls.

## 4. Cache, Resilience, and Performance
- Add local SQLite tables for:
  - `price_samples(symbol, ts, open, high, low, close, volume)`
  - `news_digest(symbol, published_at, headline, sentiment, summary, why_it_matters, glossary_json, url)`
- Retain last 7 days locally.
- On stream/API failure:
  - continue rendering focus/news from cache.
  - surface clear degraded banner with age of last refresh.
- Keep frame budget stable by downsampling chart points per terminal width.

## 5. Tests and Validation

### Unit tests
- Python:
  - history endpoint returns valid OHLC ordering and non-empty metadata.
  - digest generation populates `summary` and `why_it_matters`.
- Rust gateway:
  - `GetFocusBundle` mapping tests (history + digest parsing).
- Rust client:
  - chart mode toggle state.
  - timeframe switching.
  - polling response-shape compatibility fix.

### Integration tests
- gRPC focus request returns data for valid symbol and graceful empty for unknown symbol.
- News filter scenarios (symbol/sentiment/recency).
- Offline/degraded flow renders cached focus/news without panic.

### Manual acceptance scenarios
- From Radar to Focus in one key (`f`) on selected ticker.
- Toggle line/candle (`c`) and timeframe (`t`) with visible chart updates.
- Navigate headlines and read guided digest without leaving terminal.
- New user can discover key actions via `?` and follow the flow without docs.

## Acceptance Criteria
- Focus Mode renders chart + digest for selected ticker in under 1 second on warm cache.
- Both chart modes are usable from keyboard only.
- Guided digest is present for at least 90% of fetched headlines.
- Polling fallback correctly shows price and news data (shape mismatch fixed).
- No regressions in existing views (`Radar`, `Tactical`, `Threat Board`).

## Assumptions and Defaults
- Keep current data providers (`yfinance`, RSS feeds), no premium APIs.
- “Balanced audience” means pro controls are available, but explanations are on by default.
- “Guided digest” is deterministic/template-based, not LLM-generated.
- Terminal baseline target is 120x30; layout degrades gracefully on smaller sizes.
- Existing security/audit controls remain unchanged in this phase.
