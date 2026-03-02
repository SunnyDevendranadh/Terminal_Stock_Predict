use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rusqlite::{params, Connection};
use serde_json;

use crate::client::{
    Candle, EventIntelData, EventSummary, FocusBundleData, MlCalibrationStatusData, MlDecisionSupportData,
    MlHorizonDecisionData, NewsDigestItem, NewsItem, PricePoint, SignalEntry, SignalSnapshot, ThreatSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Radar,
    Tactical,
    SignalDetail,
    ThreatBoard,
    News,
    Focus,
    IntelBoard,
    EventDetail,
    PortfolioRisk,
}

#[allow(dead_code)]
impl View {
    pub fn all() -> [Self; 9] {
        [
            Self::Radar,
            Self::Tactical,
            Self::SignalDetail,
            Self::ThreatBoard,
            Self::News,
            Self::Focus,
            Self::IntelBoard,
            Self::EventDetail,
            Self::PortfolioRisk,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Radar => "Radar",
            Self::Tactical => "Tactical",
            Self::SignalDetail => "Signal Detail",
            Self::ThreatBoard => "Threat Board",
            Self::News => "News",
            Self::Focus => "Focus",
            Self::IntelBoard => "Intel Board",
            Self::EventDetail => "Event Detail",
            Self::PortfolioRisk => "Portfolio Risk",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Radar => 0,
            Self::Tactical => 1,
            Self::SignalDetail => 2,
            Self::ThreatBoard => 3,
            Self::News => 4,
            Self::Focus => 5,
            Self::IntelBoard => 6,
            Self::EventDetail => 7,
            Self::PortfolioRisk => 8,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::all()[index % Self::all().len()]
    }

    pub fn next(self) -> Self {
        Self::from_index(self.index() + 1)
    }

    pub fn prev(self) -> Self {
        if self.index() == 0 {
            Self::from_index(Self::all().len() - 1)
        } else {
            Self::from_index(self.index() - 1)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChartMode {
    Line,
    Candle,
}

#[derive(Debug, Clone)]
pub struct FocusState {
    pub symbol: String,
    pub timeframe: String,
    pub chart_mode: ChartMode,
    pub selected_digest_idx: usize,
    pub data: Option<FocusBundleData>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IntelFilters {
    pub severity: String,
    pub confidence_floor: f64,
    pub contradiction_only: bool,
    pub window: String,
    pub cursor: String,
}

#[derive(Debug, Clone)]
pub struct IntelBoardState {
    pub events: Vec<EventSummary>,
    pub selected_idx: usize,
    pub loading: bool,
    pub stale: bool,
    pub count: i32,
    pub total: i32,
    pub next_cursor: String,
    pub generated_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EventDetailState {
    pub event_id: String,
    pub data: Option<EventIntelData>,
    pub selected_claim_idx: usize,
    pub selected_evidence_idx: usize,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PortfolioRiskState {
    pub symbol: String,
    pub model_version: String,
    pub model_lineage: String,
    pub feature_contract_status: String,
    pub expected_return: f64,
    pub downside_risk: f64,
    pub confidence: f64,
    pub prob_up: f64,
    pub prob_flat: f64,
    pub prob_down: f64,
    pub confidence_floor: f64,
    pub confidence_gated: bool,
    pub regime: String,
    pub regime_momentum: f64,
    pub regime_realized_vol_proxy: f64,
    pub regime_liquidity_proxy: f64,
    pub action_band: String,
    pub horizons: Vec<MlHorizonDecisionData>,
    pub exposure_band: String,
    pub concentration_warning: Option<String>,
    pub suggested_size_band: String,
    pub max_single_position_pct: f64,
    pub stop_review_required: bool,
    pub sample_count: i32,
    pub ece: f64,
    pub brier_score: f64,
    pub hit_rate: f64,
    pub confidence_drift: f64,
    pub model_calibration_error: f64,
    pub model_confidence_drift: f64,
    pub model_generated_at: String,
    pub calibration_updated_at: String,
    pub loading: bool,
    pub error: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub enum IntelRequest {
    Board {
        symbols: Vec<String>,
        severity: String,
        sentiment: String,
        window: String,
        limit: i32,
        cursor: String,
    },
    EventDetail {
        event_id: String,
        symbols: Vec<String>,
    },
    MlRisk {
        symbol: String,
        features: HashMap<String, f64>,
        model_version: String,
        confidence_floor: f64,
    },
}

pub struct App {
    pub view: View,
    pub should_quit: bool,
    pub degraded: bool,
    pub signals: Vec<SignalEntry>,
    pub threat: ThreatSnapshot,
    pub selected_signal: usize,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub refresh_interval: Duration,
    pub locked: bool,
    pub pending_quit: bool,
    pub news: Vec<NewsItem>,
    pub selected_news: usize,
    pub refresh_tx: tokio::sync::mpsc::Sender<()>,
    // Focus mode state
    pub focus: FocusState,
    pub focus_tx: tokio::sync::mpsc::Sender<(String, String)>,
    // News view enhanced state
    pub news_filter_sentiment: Option<String>,
    pub news_expanded: bool,
    pub news_filter_input: Option<String>,
    // Overlay visibility
    pub glossary_visible: bool,
    pub keymap_visible: bool,
    pub intel_filters: IntelFilters,
    pub intel_board: IntelBoardState,
    pub event_detail: EventDetailState,
    pub portfolio_risk: PortfolioRiskState,
    idle_timeout: Duration,
    last_input_at: Instant,
    cache: CacheStore,
    pub intel_tx: tokio::sync::mpsc::Sender<IntelRequest>,
}

impl App {
    pub fn new(
        cache_path: PathBuf,
        refresh_interval: Duration,
        idle_timeout: Duration,
        refresh_tx: tokio::sync::mpsc::Sender<()>,
        focus_tx: tokio::sync::mpsc::Sender<(String, String)>,
        intel_tx: tokio::sync::mpsc::Sender<IntelRequest>,
    ) -> Result<Self> {
        let cache = CacheStore::open(&cache_path)?;
        let cached = cache.load_latest()?;

        let mut app = Self {
            view: View::Radar,
            should_quit: false,
            degraded: true,
            signals: Vec::new(),
            threat: ThreatSnapshot {
                kill_switch_active: false,
                kill_switch_level: "unknown".to_string(),
                chain_verification_status: "unknown".to_string(),
                audit_rpo_seconds: 0,
                api_quota_status: format!("POLL/{:02}s budget mode", refresh_interval.as_secs()),
            },
            selected_signal: 0,
            last_refresh_at: None,
            last_error: Some("Initializing stream session".to_string()),
            refresh_interval,
            locked: false,
            pending_quit: false,
            news: Vec::new(),
            selected_news: 0,
            refresh_tx,
            focus: FocusState {
                symbol: String::new(),
                timeframe: "5d".to_string(),
                chart_mode: ChartMode::Line,
                selected_digest_idx: 0,
                data: None,
                loading: false,
                error: None,
            },
            focus_tx,
            news_filter_sentiment: None,
            news_expanded: false,
            news_filter_input: None,
            glossary_visible: false,
            keymap_visible: false,
            intel_filters: IntelFilters {
                severity: String::new(),
                confidence_floor: 0.40,
                contradiction_only: false,
                window: "24h".to_string(),
                cursor: String::new(),
            },
            intel_board: IntelBoardState {
                events: Vec::new(),
                selected_idx: 0,
                loading: false,
                stale: true,
                count: 0,
                total: 0,
                next_cursor: String::new(),
                generated_at: String::new(),
                error: None,
            },
            event_detail: EventDetailState {
                event_id: String::new(),
                data: None,
                selected_claim_idx: 0,
                selected_evidence_idx: 0,
                loading: false,
                error: None,
            },
            portfolio_risk: PortfolioRiskState {
                symbol: String::new(),
                model_version: String::new(),
                model_lineage: String::new(),
                feature_contract_status: String::new(),
                expected_return: 0.0,
                downside_risk: 0.0,
                confidence: 0.0,
                prob_up: 0.0,
                prob_flat: 0.0,
                prob_down: 0.0,
                confidence_floor: 0.55,
                confidence_gated: false,
                regime: String::new(),
                regime_momentum: 0.0,
                regime_realized_vol_proxy: 0.0,
                regime_liquidity_proxy: 0.0,
                action_band: String::new(),
                horizons: Vec::new(),
                exposure_band: String::new(),
                concentration_warning: None,
                suggested_size_band: "HOLD".to_string(),
                max_single_position_pct: 0.0,
                stop_review_required: false,
                sample_count: 0,
                ece: 0.0,
                brier_score: 0.0,
                hit_rate: 0.0,
                confidence_drift: 0.0,
                model_calibration_error: 0.0,
                model_confidence_drift: 0.0,
                model_generated_at: String::new(),
                calibration_updated_at: String::new(),
                loading: false,
                error: None,
                updated_at: None,
            },
            idle_timeout,
            last_input_at: Instant::now(),
            cache,
            intel_tx,
        };

        if let Some(snapshot) = cached {
            app.apply_snapshot(snapshot)?;
            app.degraded = true;
            app.last_error = Some("Using cached signal set until server link is restored".to_string());
        }

        Ok(app)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Unlock on any key press when locked
        if self.locked {
            self.locked = false;
            return;
        }

        self.last_input_at = Instant::now();

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // Handle active filter input mode
        if self.news_filter_input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.news_filter_input = None;
                    return;
                }
                KeyCode::Enter => {
                    // Commit the filter (keep it active but exit edit mode)
                    let text = self.news_filter_input.take().unwrap_or_default();
                    self.news_filter_input = None;
                    // Apply as sentiment filter keyword if non-empty
                    if !text.is_empty() {
                        let lower = text.to_lowercase();
                        if lower.starts_with("pos") {
                            self.news_filter_sentiment = Some("positive".to_string());
                        } else if lower.starts_with("neg") {
                            self.news_filter_sentiment = Some("negative".to_string());
                        } else if lower.starts_with("neu") {
                            self.news_filter_sentiment = Some("neutral".to_string());
                        }
                    }
                    return;
                }
                KeyCode::Backspace => {
                    if let Some(ref mut s) = self.news_filter_input {
                        s.pop();
                    }
                    return;
                }
                KeyCode::Char(ch) => {
                    if let Some(ref mut s) = self.news_filter_input {
                        s.push(ch);
                    }
                    return;
                }
                _ => return,
            }
        }

        match key.code {
            KeyCode::Char('q') => {
                if self.pending_quit {
                    self.should_quit = true;
                } else {
                    self.pending_quit = true;
                }
            }
            KeyCode::Esc => {
                self.pending_quit = false;
                self.keymap_visible = false;
                self.glossary_visible = false;
            }
            KeyCode::Char('1') => { self.pending_quit = false; self.view = View::Radar; }
            KeyCode::Char('2') => { self.pending_quit = false; self.view = View::Tactical; }
            KeyCode::Char('3') => { self.pending_quit = false; self.view = View::SignalDetail; }
            KeyCode::Char('4') => { self.pending_quit = false; self.view = View::ThreatBoard; }
            KeyCode::Char('5') => { self.pending_quit = false; self.view = View::News; }
            KeyCode::Char('6') => { self.pending_quit = false; self.view = View::Focus; }
            KeyCode::Char('7') => {
                self.pending_quit = false;
                self.view = View::IntelBoard;
                self.request_intel_board();
            }
            KeyCode::Char('8') => { self.pending_quit = false; self.view = View::EventDetail; }
            KeyCode::Char('9') => {
                self.pending_quit = false;
                self.view = View::PortfolioRisk;
                self.request_portfolio_risk();
            }
            KeyCode::Tab => {
                self.pending_quit = false;
                self.view = self.view.next();
                if self.view == View::PortfolioRisk {
                    self.request_portfolio_risk();
                }
            }
            KeyCode::BackTab => {
                self.pending_quit = false;
                self.view = self.view.prev();
                if self.view == View::PortfolioRisk {
                    self.request_portfolio_risk();
                }
            }
            KeyCode::Enter => {
                self.pending_quit = false;
                if self.view == View::Radar {
                    self.view = View::SignalDetail;
                } else if self.view == View::News {
                    self.news_expanded = !self.news_expanded;
                } else if self.view == View::IntelBoard {
                    if let Some(event_id) = self.selected_event_summary().map(|e| e.event_id.clone()) {
                        self.view = View::EventDetail;
                        self.request_event_detail(event_id);
                    }
                }
            }
            KeyCode::Char('r') => {
                self.pending_quit = false;
                if self.view == View::EventDetail && !self.event_detail.event_id.is_empty() {
                    self.request_event_detail(self.event_detail.event_id.clone());
                } else if self.view == View::IntelBoard {
                    self.request_intel_board();
                } else if self.view == View::PortfolioRisk {
                    self.request_portfolio_risk();
                } else if self.view == View::Focus && !self.focus.symbol.is_empty() {
                    self.request_ml_risk_for_symbol(self.focus.symbol.clone());
                } else {
                    let _ = self.refresh_tx.try_send(());
                }
            }
            // Focus mode: enter from Radar on 'f'
            KeyCode::Char('f') => {
                self.pending_quit = false;
                if let Some(sig) = self.selected_signal() {
                    let symbol = sig.symbol.clone();
                    let timeframe = self.focus.timeframe.clone();
                    self.focus.symbol = symbol.clone();
                    self.focus.loading = true;
                    self.focus.error = None;
                    self.view = View::Focus;
                    let _ = self.focus_tx.try_send((symbol, timeframe));
                    self.request_ml_risk_for_symbol(self.focus.symbol.clone());
                }
            }
            // Intel board shortcut
            KeyCode::Char('i') => {
                self.pending_quit = false;
                self.view = View::IntelBoard;
                self.request_intel_board();
            }
            // Event detail shortcut from Intel board
            KeyCode::Char('e') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    if let Some(event_id) = self.selected_event_summary().map(|e| e.event_id.clone()) {
                        self.view = View::EventDetail;
                        self.request_event_detail(event_id);
                    }
                }
            }
            // Portfolio risk view shortcut
            KeyCode::Char('p') => {
                self.pending_quit = false;
                self.view = View::PortfolioRisk;
                self.request_portfolio_risk();
            }
            // Focus: cycle timeframe
            KeyCode::Char('t') => {
                self.pending_quit = false;
                if self.view == View::Focus {
                    self.focus.timeframe = match self.focus.timeframe.as_str() {
                        "1d" => "5d".to_string(),
                        "5d" => "1mo".to_string(),
                        _ => "1d".to_string(),
                    };
                    let symbol = self.focus.symbol.clone();
                    let timeframe = self.focus.timeframe.clone();
                    if !symbol.is_empty() {
                        self.focus.loading = true;
                        let _ = self.focus_tx.try_send((symbol, timeframe));
                        self.request_ml_risk_for_symbol(self.focus.symbol.clone());
                    }
                }
            }
            // Focus: toggle chart mode
            KeyCode::Char('c') => {
                self.pending_quit = false;
                if self.view == View::Focus {
                    self.focus.chart_mode = match self.focus.chart_mode {
                        ChartMode::Line => ChartMode::Candle,
                        ChartMode::Candle => ChartMode::Line,
                    };
                }
            }
            // Intel filters
            KeyCode::Char('v') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    self.intel_filters.severity = match self.intel_filters.severity.as_str() {
                        "" => "medium".to_string(),
                        "medium" => "high".to_string(),
                        "high" => "critical".to_string(),
                        _ => String::new(),
                    };
                    self.intel_filters.cursor = String::new();
                    self.request_intel_board();
                }
            }
            KeyCode::Char('x') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    self.intel_filters.contradiction_only = !self.intel_filters.contradiction_only;
                    self.request_intel_board();
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    self.intel_filters.confidence_floor = (self.intel_filters.confidence_floor + 0.05).clamp(0.0, 0.95);
                }
            }
            KeyCode::Char('-') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    self.intel_filters.confidence_floor = (self.intel_filters.confidence_floor - 0.05).clamp(0.0, 0.95);
                }
            }
            KeyCode::Char('n') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard && !self.intel_board.next_cursor.is_empty() {
                    self.intel_filters.cursor = self.intel_board.next_cursor.clone();
                    self.request_intel_board();
                }
            }
            KeyCode::Char('b') => {
                self.pending_quit = false;
                if self.view == View::IntelBoard {
                    self.intel_filters.cursor.clear();
                    self.request_intel_board();
                }
            }
            // Focus: prev symbol
            KeyCode::Char('[') => {
                self.pending_quit = false;
                if self.view == View::Focus && !self.signals.is_empty() {
                    if self.selected_signal > 0 {
                        self.selected_signal -= 1;
                    } else {
                        self.selected_signal = self.signals.len() - 1;
                    }
                    if let Some(sig) = self.selected_signal() {
                        let symbol = sig.symbol.clone();
                        let timeframe = self.focus.timeframe.clone();
                        self.focus.symbol = symbol.clone();
                        self.focus.loading = true;
                        self.focus.error = None;
                        let _ = self.focus_tx.try_send((symbol, timeframe));
                        self.request_ml_risk_for_symbol(self.focus.symbol.clone());
                    }
                }
            }
            // Focus: next symbol
            KeyCode::Char(']') => {
                self.pending_quit = false;
                if self.view == View::Focus && !self.signals.is_empty() {
                    self.selected_signal = (self.selected_signal + 1) % self.signals.len();
                    if let Some(sig) = self.selected_signal() {
                        let symbol = sig.symbol.clone();
                        let timeframe = self.focus.timeframe.clone();
                        self.focus.symbol = symbol.clone();
                        self.focus.loading = true;
                        self.focus.error = None;
                        let _ = self.focus_tx.try_send((symbol, timeframe));
                        self.request_ml_risk_for_symbol(self.focus.symbol.clone());
                    }
                }
            }
            // Toggle glossary overlay
            KeyCode::Char('g') => {
                self.pending_quit = false;
                self.glossary_visible = !self.glossary_visible;
            }
            // Toggle keymap overlay
            KeyCode::Char('?') => {
                self.pending_quit = false;
                self.keymap_visible = !self.keymap_visible;
            }
            // Enter filter input mode (News view)
            KeyCode::Char('/') => {
                self.pending_quit = false;
                if self.view == View::News {
                    self.news_filter_input = Some(String::new());
                }
            }
            KeyCode::Up => {
                self.pending_quit = false;
                if self.view == View::News {
                    if self.selected_news > 0 {
                        self.selected_news -= 1;
                    }
                } else if self.view == View::Focus {
                    if self.focus.selected_digest_idx > 0 {
                        self.focus.selected_digest_idx -= 1;
                    }
                } else if self.view == View::IntelBoard {
                    if self.intel_board.selected_idx > 0 {
                        self.intel_board.selected_idx -= 1;
                    }
                } else if self.view == View::EventDetail {
                    if self.event_detail.selected_claim_idx > 0 {
                        self.event_detail.selected_claim_idx -= 1;
                    }
                } else if self.selected_signal > 0 {
                    self.selected_signal -= 1;
                    if self.view == View::PortfolioRisk {
                        self.request_portfolio_risk();
                    }
                }
            }
            KeyCode::Down => {
                self.pending_quit = false;
                if self.view == View::News {
                    if !self.news.is_empty() {
                        self.selected_news = (self.selected_news + 1).min(self.news.len() - 1);
                    }
                } else if self.view == View::Focus {
                    if let Some(ref data) = self.focus.data {
                        if !data.digest.is_empty() {
                            self.focus.selected_digest_idx =
                                (self.focus.selected_digest_idx + 1).min(data.digest.len() - 1);
                        }
                    }
                } else if self.view == View::IntelBoard {
                    if !self.intel_board.events.is_empty() {
                        self.intel_board.selected_idx =
                            (self.intel_board.selected_idx + 1).min(self.intel_board.events.len() - 1);
                    }
                } else if self.view == View::EventDetail {
                    if let Some(ref data) = self.event_detail.data {
                        if !data.claims.is_empty() {
                            self.event_detail.selected_claim_idx =
                                (self.event_detail.selected_claim_idx + 1).min(data.claims.len() - 1);
                        }
                    }
                } else if !self.signals.is_empty() {
                    self.selected_signal = (self.selected_signal + 1).min(self.signals.len() - 1);
                    if self.view == View::PortfolioRisk {
                        self.request_portfolio_risk();
                    }
                }
            }
            KeyCode::Left => {
                self.pending_quit = false;
                if self.view == View::EventDetail && self.event_detail.selected_evidence_idx > 0 {
                    self.event_detail.selected_evidence_idx -= 1;
                } else {
                    self.view = self.view.prev();
                    if self.view == View::PortfolioRisk {
                        self.request_portfolio_risk();
                    }
                }
            }
            KeyCode::Right => {
                self.pending_quit = false;
                if self.view == View::EventDetail {
                    if let Some(ref data) = self.event_detail.data {
                        if let Some(claim) = data.claims.get(self.event_detail.selected_claim_idx) {
                            if !claim.evidence.is_empty() {
                                self.event_detail.selected_evidence_idx =
                                    (self.event_detail.selected_evidence_idx + 1).min(claim.evidence.len() - 1);
                            }
                        }
                    }
                } else {
                    self.view = self.view.next();
                    if self.view == View::PortfolioRisk {
                        self.request_portfolio_risk();
                    }
                }
            }
            _ => { self.pending_quit = false; }
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: SignalSnapshot) -> Result<()> {
        self.cache.save_snapshot(&snapshot)?;
        self.last_refresh_at = Some(snapshot.captured_at);
        self.signals = snapshot.signals;
        self.threat = snapshot.threat;
        self.news = snapshot.news.clone();
        self.degraded = false;
        self.last_error = None;
        self.selected_signal = self.selected_signal.min(self.signals.len().saturating_sub(1));
        self.selected_news = self.selected_news.min(self.news.len().saturating_sub(1));
        Ok(())
    }

    pub fn mark_degraded(&mut self, err: String) {
        self.degraded = true;
        self.last_error = Some(err);

        if self.signals.is_empty() {
            if let Ok(Some(snapshot)) = self.cache.load_latest() {
                self.last_refresh_at = Some(snapshot.captured_at);
                self.signals = snapshot.signals;
                self.threat = snapshot.threat;
                self.selected_signal = self.selected_signal.min(self.signals.len().saturating_sub(1));
            }
        }
    }

    pub fn check_idle_timeout(&mut self) {
        if self.locked {
            return;
        }

        if self.last_input_at.elapsed() >= self.idle_timeout {
            self.locked = true;
            self.last_error = Some("Session locked after 5 minutes of inactivity — press any key to resume".to_string());
        }
    }

    pub fn status_banner(&self) -> String {
        if self.degraded {
            "DEGRADED".to_string()
        } else {
            "CONNECTED".to_string()
        }
    }

    pub fn selected_signal(&self) -> Option<&SignalEntry> {
        self.signals.get(self.selected_signal)
    }

    pub fn selected_event_summary(&self) -> Option<&EventSummary> {
        self.intel_board.events.get(self.intel_board.selected_idx)
    }

    fn request_intel_board(&mut self) {
        let symbols = if self.focus.symbol.is_empty() {
            self.signals
                .iter()
                .take(12)
                .map(|s| s.symbol.clone())
                .collect::<Vec<_>>()
        } else {
            vec![self.focus.symbol.clone()]
        };
        let req = IntelRequest::Board {
            symbols,
            severity: self.intel_filters.severity.clone(),
            sentiment: String::new(),
            window: self.intel_filters.window.clone(),
            limit: 30,
            cursor: self.intel_filters.cursor.clone(),
        };
        self.intel_board.loading = true;
        self.intel_board.error = None;
        let _ = self.intel_tx.try_send(req);
    }

    fn request_event_detail(&mut self, event_id: String) {
        let symbols = if let Some(ev) = self.selected_event_summary() {
            if ev.symbol.is_empty() {
                vec![]
            } else {
                vec![ev.symbol.clone()]
            }
        } else {
            vec![]
        };
        self.event_detail.event_id = event_id.clone();
        self.event_detail.loading = true;
        self.event_detail.error = None;
        let _ = self
            .intel_tx
            .try_send(IntelRequest::EventDetail { event_id, symbols });
    }

    fn request_ml_risk_for_symbol(&mut self, symbol: String) {
        let features = self.signal_feature_map(&symbol);
        self.portfolio_risk.symbol = symbol.clone();
        self.portfolio_risk.loading = true;
        self.portfolio_risk.error = None;
        let _ = self.intel_tx.try_send(IntelRequest::MlRisk {
            symbol,
            features,
            model_version: "current".to_string(),
            confidence_floor: self.intel_filters.confidence_floor.clamp(0.0, 0.95),
        });
    }

    fn request_portfolio_risk(&mut self) {
        let symbol = if let Some(sig) = self.selected_signal() {
            sig.symbol.clone()
        } else if !self.focus.symbol.is_empty() {
            self.focus.symbol.clone()
        } else {
            "SPY".to_string()
        };
        self.request_ml_risk_for_symbol(symbol);
    }

    fn signal_feature_map(&self, symbol: &str) -> HashMap<String, f64> {
        let mut out = HashMap::from([
            ("open".to_string(), 0.5),
            ("high".to_string(), 0.6),
            ("low".to_string(), 0.4),
            ("close".to_string(), 0.5),
            ("volume".to_string(), 0.5),
        ]);

        if let Some(sig) = self.signals.iter().find(|s| s.symbol == symbol) {
            for fc in &sig.feature_contributions {
                if out.contains_key(&fc.name) {
                    let approx = clamp01((fc.score / 200.0) + 0.5);
                    out.insert(fc.name.clone(), approx);
                }
            }
            // Backfill missing/flat values from signal strength.
            let close = clamp01(sig.raw_score);
            let open = clamp01(close - sig.bucket_1_5d * 0.0025);
            let high = clamp01(open.max(close) + 0.08);
            let low = clamp01(open.min(close) - 0.08);
            out.insert("open".to_string(), open);
            out.insert("high".to_string(), high);
            out.insert("low".to_string(), low);
            out.insert("close".to_string(), close);
            out.entry("volume".to_string())
                .and_modify(|v| *v = clamp01((*v + close) / 2.0))
                .or_insert(0.5);
        }
        out
    }

    pub fn top_long_short(&self) -> (Vec<SignalEntry>, Vec<SignalEntry>) {
        let mut ranked = self.signals.clone();
        ranked.sort_by(|a, b| b.alpha_score.cmp(&a.alpha_score));

        let longs = ranked.iter().take(10).cloned().collect::<Vec<_>>();
        let mut shorts = ranked
            .iter()
            .rev()
            .take(10)
            .cloned()
            .collect::<Vec<_>>();
        shorts.reverse();

        (longs, shorts)
    }

    pub fn freshness_seconds(&self) -> Option<i64> {
        self.last_refresh_at
            .map(|t| (Utc::now() - t).num_seconds().max(0))
    }

    pub fn freshness_slo_ok(&self) -> bool {
        match self.freshness_seconds() {
            Some(age) => age <= (self.refresh_interval.as_secs() as i64 * 2),
            None => false,
        }
    }

    pub fn apply_focus_bundle(&mut self, data: FocusBundleData) {
        let symbol = data.symbol.clone();
        // Save to SQLite cache
        let _ = self.cache.save_price_samples(&data.symbol, &data.timeframe, &data.candles);
        let _ = self.cache.save_digest(&data.digest);
        self.focus.data = Some(data);
        self.focus.loading = false;
        self.focus.error = None;
        self.focus.selected_digest_idx = 0;
        if !symbol.is_empty() {
            self.request_ml_risk_for_symbol(symbol);
        }
    }

    #[allow(dead_code)]
    pub fn load_focus_from_cache(&mut self, symbol: &str, timeframe: &str) {
        let candles = self.cache.load_price_samples(symbol, timeframe);
        let digest = self.cache.load_digest(symbol);
        if !candles.is_empty() || !digest.is_empty() {
            let line_series: Vec<PricePoint> = candles.iter().map(|c| PricePoint {
                ts: c.ts.clone(),
                price: c.close,
                volume: c.volume,
            }).collect();
            self.focus.data = Some(FocusBundleData {
                symbol: symbol.to_string(),
                timeframe: timeframe.to_string(),
                line_series,
                candles,
                digest,
                trend_label: "Cached".to_string(),
                change_pct: 0.0,
                event_markers: vec![],
                confidence_band: vec![],
                impact_probabilities: vec![],
            });
        }
    }

    pub fn apply_intel_board(
        &mut self,
        events: Vec<EventSummary>,
        count: i32,
        total: i32,
        next_cursor: String,
        stale: bool,
        generated_at: String,
    ) {
        let filtered = events
            .into_iter()
            .filter(|e| {
                e.confidence >= self.intel_filters.confidence_floor
                    && (!self.intel_filters.contradiction_only || e.contradiction_score >= 0.35)
            })
            .collect::<Vec<_>>();

        self.intel_board.events = filtered;
        self.intel_board.count = count;
        self.intel_board.total = total;
        self.intel_board.next_cursor = next_cursor;
        self.intel_board.stale = stale;
        self.intel_board.generated_at = generated_at;
        self.intel_board.loading = false;
        self.intel_board.error = None;
        self.intel_board.selected_idx = self
            .intel_board
            .selected_idx
            .min(self.intel_board.events.len().saturating_sub(1));
    }

    pub fn apply_event_detail(&mut self, detail: EventIntelData) {
        self.event_detail.loading = false;
        self.event_detail.error = None;
        self.event_detail.selected_claim_idx = 0;
        self.event_detail.selected_evidence_idx = 0;
        self.event_detail.data = Some(detail.clone());
    }

    pub fn apply_ml_risk(
        &mut self,
        symbol: String,
        ml: MlDecisionSupportData,
        calibration: MlCalibrationStatusData,
    ) {
        let primary = ml
            .horizons
            .iter()
            .find(|h| h.horizon == "swing")
            .or_else(|| ml.horizons.first())
            .cloned();

        let (expected_return, downside_risk, confidence, prob_up, prob_flat, prob_down, action_band) =
            if let Some(h) = primary {
                (
                    h.expected_return,
                    h.downside_risk,
                    h.confidence,
                    h.prob_up,
                    h.prob_flat,
                    h.prob_down,
                    h.action_band,
                )
            } else {
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, "watch".to_string())
            };

        self.portfolio_risk = PortfolioRiskState {
            symbol,
            model_version: ml.model_version.clone(),
            model_lineage: ml.model_lineage.clone(),
            feature_contract_status: ml.feature_contract_status.clone(),
            expected_return,
            downside_risk,
            confidence,
            prob_up,
            prob_flat,
            prob_down,
            confidence_floor: ml.confidence_floor,
            confidence_gated: ml.confidence_gated,
            regime: ml.regime.regime.clone(),
            regime_momentum: ml.regime.momentum,
            regime_realized_vol_proxy: ml.regime.realized_vol_proxy,
            regime_liquidity_proxy: ml.regime.liquidity_proxy,
            action_band,
            horizons: ml.horizons.clone(),
            exposure_band: ml.portfolio_risk_advisory.exposure_band.clone(),
            concentration_warning: Some(ml.portfolio_risk_advisory.concentration_risk.clone()),
            suggested_size_band: ml.portfolio_risk_advisory.suggested_sizing_band.clone(),
            max_single_position_pct: ml.portfolio_risk_advisory.max_single_position_pct,
            stop_review_required: ml.portfolio_risk_advisory.stop_review_required,
            sample_count: calibration.sample_count,
            ece: calibration.ece,
            brier_score: calibration.brier_score,
            hit_rate: calibration.hit_rate,
            confidence_drift: calibration.confidence_drift,
            model_calibration_error: ml.calibration_error,
            model_confidence_drift: ml.confidence_drift,
            model_generated_at: ml.generated_at,
            calibration_updated_at: calibration.updated_at,
            loading: false,
            error: None,
            updated_at: Some(Utc::now()),
        };
    }

    pub fn mark_intel_board_failed(&mut self, reason: String) {
        self.intel_board.loading = false;
        self.intel_board.error = Some(reason);
    }

    pub fn mark_event_detail_failed(&mut self, reason: String) {
        self.event_detail.loading = false;
        self.event_detail.error = Some(reason);
    }

    pub fn mark_ml_risk_failed(&mut self, reason: String) {
        self.portfolio_risk.loading = false;
        self.portfolio_risk.error = Some(reason);
    }

    #[allow(dead_code)]
    pub fn focus_news_filtered(&self) -> Vec<&NewsItem> {
        self.news.iter().filter(|item| {
            match &self.news_filter_sentiment {
                Some(sentiment) => &item.sentiment == sentiment,
                None => true,
            }
        }).collect()
    }

    pub fn footer_hint(&self) -> &'static str {
        match self.view {
            View::Radar => "1-9 views | Tab cycle | Up/Down select | f focus | i intel | Enter detail | r refresh | q quit",
            View::Tactical => "1-9 views | Up/Down scroll | Tab cycle | r refresh | q quit",
            View::SignalDetail => "1-9 views | Up/Down select signal | Tab cycle | q quit",
            View::ThreatBoard => "1-9 views | Tab cycle | r refresh | q quit",
            View::News => "1-9 views | Up/Down scroll | Enter expand | / filter | g glossary | ? help | q quit",
            View::Focus => "1-9 views | [/] symbol | t timeframe | c chart mode | r refresh+ml | p risk | Up/Down digest | i intel | ? help | q quit",
            View::IntelBoard => "1-9 views | Up/Down select | Enter/e detail | v severity | +/- confidence | x contradiction | n next | r refresh",
            View::EventDetail => "1-9 views | Up/Down claim | Left/Right evidence | p risk | i board | r reload",
            View::PortfolioRisk => "1-9 views | p stays | i board | e detail | r refresh intel",
        }
    }

    #[allow(dead_code)]
    pub fn timed_out(&self) -> bool {
        false // no longer auto-quits on timeout
    }
}

fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

struct CacheStore {
    conn: Connection,
}

impl CacheStore {
    fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create cache directory {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("failed to open cache database {}", path.display()))?;

        conn.execute(
            "
            CREATE TABLE IF NOT EXISTS signal_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                captured_at TEXT NOT NULL,
                payload_json TEXT NOT NULL
            )
            ",
            [],
        )
        .context("failed creating signal_snapshots table")?;

        conn.execute(
            "
            CREATE TABLE IF NOT EXISTS price_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                symbol TEXT NOT NULL,
                timeframe TEXT NOT NULL,
                ts TEXT NOT NULL,
                open REAL, high REAL, low REAL, close REAL, volume REAL,
                UNIQUE(symbol, timeframe, ts)
            )
            ",
            [],
        )
        .context("failed creating price_samples table")?;

        conn.execute(
            "
            CREATE TABLE IF NOT EXISTS news_digest_cache (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                symbol TEXT,
                published_at TEXT,
                headline TEXT UNIQUE,
                sentiment TEXT,
                summary TEXT,
                why_it_matters TEXT,
                glossary_json TEXT,
                source TEXT,
                url TEXT
            )
            ",
            [],
        )
        .context("failed creating news_digest_cache table")?;

        // Prune rows older than 7 days
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let _ = conn.execute(
            "DELETE FROM price_samples WHERE ts < ?1",
            params![cutoff],
        );
        let _ = conn.execute(
            "DELETE FROM news_digest_cache WHERE published_at < ?1",
            params![cutoff],
        );

        Ok(Self { conn })
    }

    pub fn save_price_samples(&self, symbol: &str, timeframe: &str, candles: &[Candle]) -> Result<()> {
        for c in candles {
            let _ = self.conn.execute(
                "
                INSERT OR REPLACE INTO price_samples
                    (symbol, timeframe, ts, open, high, low, close, volume)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ",
                params![symbol, timeframe, c.ts, c.open, c.high, c.low, c.close, c.volume],
            );
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn load_price_samples(&self, symbol: &str, timeframe: &str) -> Vec<Candle> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let mut stmt = match self.conn.prepare(
            "SELECT ts, open, high, low, close, volume FROM price_samples
             WHERE symbol = ?1 AND timeframe = ?2 AND ts > ?3
             ORDER BY ts ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map(params![symbol, timeframe, cutoff], |row| {
            Ok(Candle {
                ts: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub fn save_digest(&self, items: &[NewsDigestItem]) -> Result<()> {
        for item in items {
            let glossary_json = serde_json::to_string(&item.glossary_terms).unwrap_or_default();
            let _ = self.conn.execute(
                "
                INSERT OR IGNORE INTO news_digest_cache
                    (symbol, published_at, headline, sentiment, summary, why_it_matters, glossary_json, source, url)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ",
                params![
                    item.symbol, item.published_at, item.headline, item.sentiment,
                    item.summary, item.why_it_matters, glossary_json, item.source, item.url
                ],
            );
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn load_digest(&self, symbol: &str) -> Vec<NewsDigestItem> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
        let mut stmt = match self.conn.prepare(
            "SELECT symbol, published_at, headline, sentiment, summary, why_it_matters, glossary_json, source, url
             FROM news_digest_cache
             WHERE (symbol = ?1 OR ?1 = '') AND published_at > ?2
             ORDER BY published_at DESC LIMIT 50",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map(params![symbol, cutoff], |row| {
            let glossary_json: String = row.get(6).unwrap_or_default();
            let glossary_terms: Vec<String> = serde_json::from_str(&glossary_json).unwrap_or_default();
            Ok(NewsDigestItem {
                symbol: row.get(0)?,
                published_at: row.get(1)?,
                headline: row.get(2)?,
                sentiment: row.get(3)?,
                summary: row.get(4)?,
                why_it_matters: row.get(5)?,
                glossary_terms,
                source: row.get(7)?,
                url: row.get(8)?,
            })
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn save_snapshot(&self, snapshot: &SignalSnapshot) -> Result<()> {
        let payload = serde_json::to_string(snapshot).context("failed encoding snapshot JSON")?;

        self.conn
            .execute(
                "
                INSERT INTO signal_snapshots (captured_at, payload_json)
                VALUES (?1, ?2)
                ",
                params![snapshot.captured_at.to_rfc3339(), payload],
            )
            .context("failed writing snapshot cache")?;

        self.conn
            .execute(
                "
                DELETE FROM signal_snapshots
                WHERE id NOT IN (
                    SELECT id FROM signal_snapshots ORDER BY id DESC LIMIT 20
                )
                ",
                [],
            )
            .context("failed pruning snapshot cache")?;

        Ok(())
    }

    fn load_latest(&self) -> Result<Option<SignalSnapshot>> {
        let mut stmt = self
            .conn
            .prepare(
                "
                SELECT payload_json
                FROM signal_snapshots
                ORDER BY id DESC
                LIMIT 1
                ",
            )
            .context("failed preparing cache read")?;

        let mut rows = stmt.query([]).context("failed querying cache")?;
        if let Some(row) = rows.next().context("failed iterating cache rows")? {
            let payload: String = row.get(0).context("failed reading cache payload")?;
            let snapshot = serde_json::from_str::<SignalSnapshot>(&payload)
                .context("failed decoding cached snapshot")?;
            return Ok(Some(snapshot));
        }

        Ok(None)
    }
}
