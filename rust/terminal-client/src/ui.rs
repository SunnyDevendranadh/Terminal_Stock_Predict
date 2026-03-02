use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Rectangle};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Dataset, Gauge, GraphType, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::Frame;

use crate::app::{App, ChartMode, View};
use crate::client::{Candle, PricePoint, SignalEntry};

const HUD_BG: Color = Color::Black;
const NEUTRAL: Color = Color::Gray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const CARD_BG_BUY: Color = Color::Rgb(8, 34, 8);
const CARD_BG_WATCH: Color = Color::Rgb(28, 24, 8);
const CARD_BG_AVOID: Color = Color::Rgb(34, 10, 10);

pub fn draw(frame: &mut Frame, app: &mut App) {
    // Lock screen takes over the entire terminal
    if app.locked {
        render_locked_screen(frame);
        return;
    }

    let area = frame.area();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);

    render_banner(frame, layout[0], app);
    render_tabs(frame, layout[1], app);

    match app.view {
        View::Radar => render_radar(frame, layout[2], app),
        View::Tactical => render_tactical(frame, layout[2], app),
        View::SignalDetail => render_signal_detail(frame, layout[2], app),
        View::ThreatBoard => render_threat_board(frame, layout[2], app),
        View::News => render_news(frame, layout[2], app),
        View::Focus => render_focus(frame, layout[2], app),
        View::IntelBoard => render_intel_board(frame, layout[2], app),
        View::EventDetail => render_event_detail(frame, layout[2], app),
        View::PortfolioRisk => render_portfolio_risk(frame, layout[2], app),
    }

    render_footer(frame, layout[3], app);

    // Overlay panels (rendered last, on top)
    if app.keymap_visible {
        render_keymap_overlay(frame, frame.area());
    }
    if app.glossary_visible {
        render_glossary_panel(frame, frame.area(), app);
    }
}

fn render_locked_screen(frame: &mut Frame) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title("SESSION LOCKED");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = Paragraph::new("Press any key to resume")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White));

    // Center vertically
    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(inner);
    frame.render_widget(text, vert[1]);
}

fn render_banner(frame: &mut Frame, area: Rect, app: &App) {
    let status_color = if app.degraded { BAD } else { OK };
    let status_text = format!("{}", app.status_banner());
    let posture = market_posture(app);
    let refresh_text = match app.freshness_seconds() {
        Some(age) => format!("Freshness: {}s", age),
        None => "Freshness: n/a".to_string(),
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            "QUANT C2 SIGNAL COMMAND",
            Style::default()
                .fg(OK)
                .bg(HUD_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(refresh_text, Style::default().fg(WARN)),
        Span::raw("  "),
        Span::styled(format!("Posture: {}", posture), Style::default().fg(NEUTRAL)),
    ])];

    if let Some(err) = &app.last_error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(if app.degraded { BAD } else { WARN }),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(status_color)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let tab_labels = [
        "[1] Radar",
        "[2] Tactical",
        "[3] Signal Detail",
        "[4] Threat Board",
        "[5] News",
        "[6] Focus",
        "[7] Intel",
        "[8] Event",
        "[9] Risk",
    ];

    let titles = tab_labels
        .iter()
        .map(|label| {
            Line::from(Span::styled(
                *label,
                Style::default().fg(OK).add_modifier(Modifier::BOLD),
            ))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Tabs::new(titles)
            .select(app.view.index())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled("VIEWS", Style::default().fg(WARN))),
            )
            .style(Style::default().fg(OK).bg(HUD_BG))
            .highlight_style(
                Style::default()
                    .fg(HUD_BG)
                    .bg(OK)
                    .add_modifier(Modifier::BOLD),
            ),
        area,
    );
}

fn render_radar(frame: &mut Frame, area: Rect, app: &App) {
    if app.signals.is_empty() {
        frame.render_widget(empty_block("No signal feed"), area);
        return;
    }

    let mut ranked = app.signals.iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.alpha_score.cmp(&a.alpha_score));
    let selected_symbol = app.selected_signal().map(|s| s.symbol.as_str());

    let buy_count = ranked
        .iter()
        .filter(|s| matches!(action_label(s.alpha_score), "BUY"))
        .count();
    let watch_count = ranked
        .iter()
        .filter(|s| matches!(action_label(s.alpha_score), "WATCH"))
        .count();
    let avoid_count = ranked
        .iter()
        .filter(|s| matches!(action_label(s.alpha_score), "AVOID"))
        .count();
    let avg_score = ranked.iter().map(|s| s.alpha_score as u32).sum::<u32>() as f64 / ranked.len() as f64;

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(10), Constraint::Length(2)])
        .split(area);

    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(18),
            Constraint::Percentage(28),
            Constraint::Percentage(20),
        ])
        .split(split[0]);

    let posture_text = market_posture(app);
    let top_idea = ranked
        .first()
        .map(|s| format!("{} {} ({})", s.symbol, action_label(s.alpha_score), s.alpha_score))
        .unwrap_or_else(|| "n/a".to_string());

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("RADAR OPS PICTURE", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
            Line::from(format!(
                "Link {} | Posture {}",
                if app.degraded { "DEGRADED" } else { "LIVE" },
                posture_text
            )),
        ])
        .style(Style::default().fg(NEUTRAL))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(OK))),
        header[0],
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("UNIVERSE", Style::default().fg(WARN).add_modifier(Modifier::BOLD))),
            Line::from(format!("{} symbols", ranked.len())),
        ])
        .style(Style::default().fg(NEUTRAL))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(WARN))),
        header[1],
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("ACTION MIX", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
            Line::from(vec![
                Span::styled(format!("BUY {}", buy_count), Style::default().fg(OK)),
                Span::raw("  "),
                Span::styled(format!("WATCH {}", watch_count), Style::default().fg(WARN)),
                Span::raw("  "),
                Span::styled(format!("AVOID {}", avoid_count), Style::default().fg(BAD)),
            ]),
            Line::from(format!("Avg score {:.1}", avg_score)),
        ])
        .style(Style::default().fg(NEUTRAL))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(OK))),
        header[2],
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("TOP IDEA", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
            Line::from(top_idea),
        ])
        .style(Style::default().fg(NEUTRAL))
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(OK))),
        header[3],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(split[1]);

    let cols = if body[0].width >= 140 {
        4usize
    } else if body[0].width >= 110 {
        3usize
    } else if body[0].width >= 72 {
        2usize
    } else {
        1usize
    };
    let rows = (ranked.len() + cols - 1) / cols;
    let row_height = if body[0].height >= 22 { 4 } else { 3 };
    let row_constraints = vec![Constraint::Length(row_height); rows.max(1)];
    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(body[0]);

    for (row_index, row_area) in row_chunks.iter().enumerate() {
        let col_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Percentage((100 / cols) as u16); cols])
            .split(*row_area);

        for (col_index, col_area) in col_chunks.iter().enumerate() {
            let idx = row_index * cols + col_index;
            if let Some(signal) = ranked.get(idx) {
                let bar = alpha_bar(signal.alpha_score, 10);
                let action = action_label(signal.alpha_score);
                let action_color = action_color(signal.alpha_score);
                let is_selected = selected_symbol == Some(signal.symbol.as_str());
                let block_style = Style::default()
                    .fg(NEUTRAL)
                    .bg(alpha_heat(signal.alpha_score))
                    .add_modifier(if is_selected { Modifier::BOLD } else { Modifier::empty() });
                let rank = idx + 1;

                // Build price line
                let price_line = if signal.price > 0.0 {
                    format!("${:.2} {:+.1}%", signal.price, signal.price_change_pct)
                } else {
                    String::new()
                };

                let text = if row_height >= 4 {
                    vec![
                        Line::from(vec![
                            Span::styled(
                                format!("#{rank:02} {}", signal.symbol),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                format!("[{}]", action),
                                Style::default().fg(action_color).add_modifier(Modifier::BOLD),
                            ),
                            if !price_line.is_empty() {
                                Span::styled(
                                    format!("  {}", price_line),
                                    Style::default().fg(NEUTRAL),
                                )
                            } else {
                                Span::raw("")
                            },
                        ]),
                        Line::from(format!(
                            "Score {:>3} | 1-5d {:+.2} | 20-60d {:+.2}",
                            signal.alpha_score, signal.bucket_1_5d, signal.bucket_20_60d
                        )),
                        Line::from(Span::styled(bar, Style::default().fg(action_color))),
                    ]
                } else {
                    vec![
                        Line::from(vec![
                            Span::styled(
                                format!("#{rank:02} {}", signal.symbol),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(" "),
                            Span::styled(
                                format!("{:>3} {}", signal.alpha_score, action),
                                Style::default().fg(action_color),
                            ),
                        ]),
                        Line::from(Span::styled(bar, Style::default().fg(action_color))),
                    ]
                };
                frame.render_widget(
                    Paragraph::new(text)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(if is_selected { OK } else { action_color }))
                                .title(if is_selected { "FOCUS" } else { "" }),
                        )
                        .style(block_style),
                    *col_area,
                );
            } else {
                frame.render_widget(Block::default(), *col_area);
            }
        }
    }

    let side = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Length(3), Constraint::Min(6)])
        .split(body[1]);

    if let Some(selected) = app.selected_signal() {
        let action = action_label(selected.alpha_score);
        let action_color = action_color(selected.alpha_score);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("FOCUS SYMBOL", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
                Line::from(vec![
                    Span::styled(&selected.symbol, Style::default().fg(OK).add_modifier(Modifier::BOLD)),
                    Span::raw("  "),
                    Span::styled(action, Style::default().fg(action_color).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(format!("Near {:+.2} | Mid {:+.2}", selected.bucket_1_5d, selected.bucket_20_60d)),
                Line::from(Span::styled(explain_signal(selected.alpha_score), Style::default().fg(NEUTRAL))),
            ])
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(OK))),
            side[0],
        );

        // Replace Gauge with Paragraph "ALPHA SCORE"
        let alpha_filled = ((selected.alpha_score as usize) * 10) / 100;
        let alpha_bar_text = "▓".repeat(alpha_filled) + &"░".repeat(10 - alpha_filled);
        frame.render_widget(
            Paragraph::new(format!("{} {}/100", alpha_bar_text, selected.alpha_score))
                .style(Style::default().fg(action_color))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(Span::styled("ALPHA SCORE", Style::default().fg(WARN))),
                ),
            side[1],
        );
    } else {
        frame.render_widget(empty_block("No selected symbol"), side[0]);
        frame.render_widget(empty_block("No score"), side[1]);
    }

    let shortlist_rows = ranked
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, s)| {
            Row::new(vec![
                Cell::from(format!("#{:02}", i + 1)),
                Cell::from(s.symbol.clone()),
                Cell::from(format!("{:>3}", s.alpha_score)),
                Cell::from(action_label(s.alpha_score)),
            ])
            .style(Style::default().fg(action_color(s.alpha_score)))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Table::new(
            shortlist_rows,
            [
                Constraint::Length(4),
                Constraint::Length(8),
                Constraint::Length(5),
                Constraint::Length(7),
            ],
        )
        .header(
            Row::new(vec!["Rank", "Symbol", "Score", "Action"])
                .style(Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled("TOP 8 SHORTLIST", Style::default().fg(OK))),
        ),
        side[2],
    );

    frame.render_widget(
        Paragraph::new(
            "Radar guide: green cards = stronger setups. Use Up/Down to set focus. Open Signal Detail for provenance before action.",
        )
        .style(Style::default().fg(NEUTRAL))
        .alignment(Alignment::Left),
        split[2],
    );
}

fn render_tactical(frame: &mut Frame, area: Rect, app: &mut App) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(area);

    let tables = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(split[0]);

    let (longs, shorts) = app.top_long_short();

    render_side_table_stateful(frame, tables[0], "TOP BUY CANDIDATES", &longs, OK, Some(app.selected_signal));
    render_side_table(frame, tables[1], "TOP SELL / HEDGE", &shorts, BAD);

    let row_count_hint = format!("Showing {} longs / {} shorts", longs.len(), shorts.len());
    let tactical_text = match (longs.first(), shorts.first()) {
        (Some(long), Some(short)) => format!(
            "Best long: {} (score {}) | Highest-risk short: {} (score {}) | {}",
            long.symbol, long.alpha_score, short.symbol, short.alpha_score, row_count_hint
        ),
        _ => format!("No tactical ranking available | {}", row_count_hint),
    };
    frame.render_widget(
        Paragraph::new(tactical_text)
            .style(Style::default().fg(NEUTRAL))
            .block(Block::default().borders(Borders::TOP).title("Decision Help")),
        split[1],
    );
}

fn render_side_table_stateful(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    signals: &[SignalEntry],
    accent: Color,
    selected: Option<usize>,
) {
    let header = Row::new(vec!["#", "Symbol", "Score", "Action", "1-5d", "20-60d"])
        .style(Style::default().fg(WARN).add_modifier(Modifier::BOLD));

    let rows = signals
        .iter()
        .enumerate()
        .map(|(idx, signal)| {
            Row::new(vec![
                Cell::from(format!("{:>2}", idx + 1)),
                Cell::from(signal.symbol.clone()),
                Cell::from(format!("{:>3}", signal.alpha_score)),
                Cell::from(action_label(signal.alpha_score)),
                Cell::from(format!("{:+.2}", signal.bucket_1_5d)),
                Cell::from(format!("{:+.2}", signal.bucket_20_60d)),
            ])
            .style(Style::default().fg(accent))
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(title, Style::default().fg(accent).add_modifier(Modifier::BOLD))),
    )
    .highlight_style(Style::default().bg(Color::DarkGray));

    let mut state = TableState::default();
    if let Some(sel) = selected {
        state.select(Some(sel.min(signals.len().saturating_sub(1))));
    }

    frame.render_stateful_widget(table, area, &mut state);
}

fn render_side_table(frame: &mut Frame, area: Rect, title: &str, signals: &[SignalEntry], accent: Color) {
    let header = Row::new(vec!["#", "Symbol", "Score", "Action", "1-5d", "20-60d"])
        .style(Style::default().fg(WARN).add_modifier(Modifier::BOLD));

    let rows = signals
        .iter()
        .enumerate()
        .map(|(idx, signal)| {
            Row::new(vec![
                Cell::from(format!("{:>2}", idx + 1)),
                Cell::from(signal.symbol.clone()),
                Cell::from(format!("{:>3}", signal.alpha_score)),
                Cell::from(action_label(signal.alpha_score)),
                Cell::from(format!("{:+.2}", signal.bucket_1_5d)),
                Cell::from(format!("{:+.2}", signal.bucket_20_60d)),
            ])
            .style(Style::default().fg(accent))
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(title, Style::default().fg(accent).add_modifier(Modifier::BOLD))),
    );

    frame.render_widget(table, area);
}

fn render_signal_detail(frame: &mut Frame, area: Rect, app: &App) {
    let signal = match app.selected_signal() {
        Some(signal) => signal,
        None => {
            frame.render_widget(empty_block("No signal selected"), area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(8)])
        .split(area);

    let action = action_label(signal.alpha_score);
    let action_color = action_color(signal.alpha_score);

    let summary = vec![
        Line::from(vec![
            Span::styled("SYMBOL: ", Style::default().fg(WARN)),
            Span::styled(&signal.symbol, Style::default().fg(OK).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("ALPHA SCORE: ", Style::default().fg(WARN)),
            Span::styled(format!("{}", signal.alpha_score), Style::default().fg(OK)),
        ]),
        Line::from(vec![
            Span::styled("RECOMMENDED ACTION: ", Style::default().fg(WARN)),
            Span::styled(action, Style::default().fg(action_color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("MODEL PROVENANCE: ", Style::default().fg(WARN)),
            Span::styled(&signal.model_provenance, Style::default().fg(NEUTRAL)),
        ]),
        Line::from(vec![
            Span::styled("MODEL VERSION: ", Style::default().fg(WARN)),
            Span::styled(&signal.model_version, Style::default().fg(NEUTRAL)),
        ]),
        Line::from(vec![
            Span::styled("FEATURE CONTRACT: ", Style::default().fg(WARN)),
            Span::styled(&signal.feature_contract_status, Style::default().fg(NEUTRAL)),
        ]),
        Line::from(vec![
            Span::styled("AUDIT HASH: ", Style::default().fg(WARN)),
            Span::styled(&signal.audit_hash, Style::default().fg(NEUTRAL)),
        ]),
        Line::from(vec![
            Span::styled("WHAT THIS MEANS: ", Style::default().fg(WARN)),
            Span::styled(explain_signal(signal.alpha_score), Style::default().fg(NEUTRAL)),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(summary)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled("SIGNAL BRIEF", Style::default().fg(OK))),
            )
            .wrap(Wrap { trim: true }),
        chunks[0],
    );

    // Feature contributions with visual bars
    let contrib_rows = signal
        .feature_contributions
        .iter()
        .map(|contrib| {
            let filled = ((contrib.score.abs() * 10.0).round() as usize).min(10);
            let bar: String = "▓".repeat(filled) + &"░".repeat(10 - filled);
            let contrib_color = if contrib.score >= 0.0 { OK } else { BAD };
            Row::new(vec![
                Cell::from(contrib.name.clone()),
                Cell::from(format!("{:+.2}", contrib.score)),
                Cell::from(bar).style(Style::default().fg(contrib_color)),
            ])
        })
        .collect::<Vec<_>>();

    let contrib_table = Table::new(
        contrib_rows,
        [Constraint::Length(16), Constraint::Length(10), Constraint::Length(12)],
    )
    .header(
        Row::new(vec!["Feature", "Contribution", "Bar"])
            .style(Style::default().fg(WARN).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled("WHY THE MODEL SAID THIS", Style::default().fg(OK))),
    )
    .highlight_style(Style::default().bg(Color::Rgb(20, 40, 20)));

    frame.render_widget(contrib_table, chunks[1]);
}

fn render_threat_board(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Min(3),
            Constraint::Min(3),
        ])
        .split(area);

    let freshness = app
        .freshness_seconds()
        .map(|age| age as f64)
        .unwrap_or(app.refresh_interval.as_secs() as f64 * 2.0);
    let max_age = (app.refresh_interval.as_secs() * 2).max(1) as f64;
    let freshness_ratio = (1.0 - (freshness / max_age)).clamp(0.0, 1.0);

    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled("DATA FRESHNESS", Style::default().fg(OK))),
            )
            .gauge_style(Style::default().fg(if app.freshness_slo_ok() { OK } else { BAD }))
            .label(format!(
                "{} ({}s old)",
                if app.freshness_slo_ok() { "OK" } else { "STALE" },
                app.freshness_seconds().unwrap_or(-1),
            ))
            .ratio(freshness_ratio),
        chunks[0],
    );

    frame.render_widget(metric_block("API QUOTA", &app.threat.api_quota_status, WARN), chunks[1]);

    let kill_status = if app.threat.kill_switch_active {
        format!("ACTIVE [{}]", app.threat.kill_switch_level)
    } else {
        format!("INACTIVE [{}]", app.threat.kill_switch_level)
    };
    frame.render_widget(
        metric_block(
            "KILL SWITCH",
            &kill_status,
            if app.threat.kill_switch_active { BAD } else { OK },
        ),
        chunks[2],
    );

    frame.render_widget(
        metric_block(
            "AUDIT CHAIN",
            &app.threat.chain_verification_status,
            if app.threat.chain_verification_status == "valid" {
                OK
            } else if app.threat.chain_verification_status == "disabled" {
                WARN
            } else {
                BAD
            },
        ),
        chunks[3],
    );

    frame.render_widget(
        metric_block(
            "AUDIT RPO",
            &format!("{}s", app.threat.audit_rpo_seconds),
            if app.threat.audit_rpo_seconds <= 60 {
                OK
            } else {
                BAD
            },
        ),
        chunks[4],
    );

    let connection_text = if app.degraded {
        "DEGRADED - cached data mode"
    } else {
        "ONLINE - live gRPC stream"
    };
    frame.render_widget(
        Paragraph::new(connection_text)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled("LINK STATE", Style::default().fg(WARN))),
            )
            .style(Style::default().fg(if app.degraded { BAD } else { OK })),
        chunks[5],
    );
}

fn render_news(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.news.is_empty() {
        let block = empty_block("No news — waiting for feed");
        frame.render_widget(block, area);
        return;
    }

    // Compute filtered list
    let filtered: Vec<usize> = (0..app.news.len())
        .filter(|&i| match &app.news_filter_sentiment {
            Some(s) => &app.news[i].sentiment == s,
            None => true,
        })
        .collect();

    // Layout: filter bar (2), content (min 8), footer hint (1)
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .split(area);

    // Filter bar
    let filter_label = match &app.news_filter_sentiment {
        Some(s) => format!("[Filter: {}]", s),
        None => "[All]".to_string(),
    };
    let filter_input_text = match &app.news_filter_input {
        Some(s) => format!(" / {}_", s),
        None => String::new(),
    };
    let filter_bar = Paragraph::new(format!("{}{} — {} items", filter_label, filter_input_text, filtered.len()))
        .style(Style::default().fg(WARN))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(filter_bar, chunks[0]);

    if app.news_expanded {
        // Split-pane: 55% list, 45% detail
        let panes = Layout::horizontal([
            Constraint::Percentage(55),
            Constraint::Percentage(45),
        ])
        .split(chunks[1]);

        // Left: headline list
        let header_row = Row::new(["Time", "Sym", "Source", "Headline"].iter().map(|h| {
            Cell::from(*h).style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
        }))
        .height(1);

        let rows: Vec<Row> = filtered
            .iter()
            .map(|&i| {
                let item = &app.news[i];
                let rel_time = relative_time(&item.published_at);
                let sym = if item.symbol.is_empty() { "MKT".to_string() } else { item.symbol.clone() };
                let sc = sentiment_color(&item.sentiment);
                Row::new(vec![
                    Cell::from(rel_time),
                    Cell::from(sym),
                    Cell::from(item.source.clone()),
                    Cell::from(item.headline.clone()).style(Style::default().fg(sc)),
                ])
            })
            .collect();

        let sel_in_filtered = filtered.iter().position(|&i| i == app.selected_news).unwrap_or(0);
        let mut state = TableState::default();
        state.select(Some(sel_in_filtered));

        let table = Table::new(
            rows,
            [Constraint::Length(5), Constraint::Length(5), Constraint::Length(10), Constraint::Min(10)],
        )
        .header(header_row)
        .block(Block::default().borders(Borders::ALL).title("Headlines"))
        .highlight_style(Style::default().bg(Color::DarkGray));
        frame.render_stateful_widget(table, panes[0], &mut state);

        // Right: detail pane
        let selected_item = app.news.get(app.selected_news);
        if let Some(item) = selected_item {
            let sc = sentiment_color(&item.sentiment);
            let detail_lines = vec![
                Line::from(Span::styled(&item.headline, Style::default().fg(sc).add_modifier(Modifier::BOLD))),
                Line::from("─────────────────────────────"),
                Line::from(vec![
                    Span::styled("Source: ", Style::default().fg(WARN)),
                    Span::raw(&item.source),
                    Span::raw("  "),
                    Span::styled(&item.sentiment, Style::default().fg(sc)),
                ]),
                Line::from("─────────────────────────────"),
                Line::from(Span::styled("Summary:", Style::default().fg(WARN))),
                Line::from(format!("  {}", relative_time(&item.published_at))),
                Line::from("─────────────────────────────"),
                Line::from(Span::styled("Why it matters:", Style::default().fg(WARN))),
                Line::from(format!("  Monitor: {} news may affect price.", item.sentiment)),
            ];
            frame.render_widget(
                Paragraph::new(detail_lines)
                    .block(Block::default().borders(Borders::ALL).title("Detail"))
                    .wrap(Wrap { trim: true }),
                panes[1],
            );
        }
    } else {
        // List-only mode
        let header_row = Row::new(["Time", "Symbol", "Source", "Headline"].iter().map(|h| {
            Cell::from(*h).style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
        }))
        .height(1);

        let rows: Vec<Row> = filtered
            .iter()
            .map(|&i| {
                let item = &app.news[i];
                let rel_time = relative_time(&item.published_at);
                let sym = if item.symbol.is_empty() { "MARKET".to_string() } else { item.symbol.clone() };
                let sc = sentiment_color(&item.sentiment);
                Row::new(vec![
                    Cell::from(rel_time),
                    Cell::from(sym),
                    Cell::from(item.source.clone()),
                    Cell::from(item.headline.clone()).style(Style::default().fg(sc)),
                ])
            })
            .collect();

        let sel_in_filtered = filtered.iter().position(|&i| i == app.selected_news).unwrap_or(0);
        let mut state = TableState::default();
        state.select(Some(sel_in_filtered));

        let table = Table::new(
            rows,
            [Constraint::Length(6), Constraint::Length(8), Constraint::Length(12), Constraint::Min(20)],
        )
        .header(header_row)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().bg(Color::DarkGray));
        frame.render_stateful_widget(table, chunks[1], &mut state);
    }

    // Footer hint
    let hint = Paragraph::new(format!(
        "Row {} of {} — Enter expand | / filter | g glossary | ? help",
        app.selected_news + 1,
        app.news.len()
    ))
    .style(Style::default().fg(NEUTRAL));
    frame.render_widget(hint, chunks[2]);
}

fn render_focus(frame: &mut Frame, area: Rect, app: &App) {
    // Layout: ticker strip + ML status, chart+side, digest
    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(15),
        Constraint::Length(8),
    ])
    .split(area);

    // Ticker strip
    let focus = &app.focus;
    let (trend_label, change_pct, chart_mode_label) = if let Some(ref data) = focus.data {
        (
            data.trend_label.clone(),
            data.change_pct,
            match focus.chart_mode { ChartMode::Line => "Line", ChartMode::Candle => "Candle" },
        )
    } else {
        ("--".to_string(), 0.0, match focus.chart_mode { ChartMode::Line => "Line", ChartMode::Candle => "Candle" })
    };

    let trend_color = match trend_label.as_str() {
        "Uptrend" => OK,
        "Downtrend" => BAD,
        _ => WARN,
    };

    let loading_str = if focus.loading {
        " [loading...]"
    } else if focus.error.is_some() {
        " [fetch failed — r to retry]"
    } else {
        ""
    };
    let strip_text = format!(
        " {} | {} | {:+.2}% | TF:{} | Chart:{}{} ",
        focus.symbol, trend_label, change_pct, focus.timeframe, chart_mode_label, loading_str
    );
    let risk = &app.portfolio_risk;
    let ml_status_line = if !focus.symbol.is_empty() && risk.symbol == focus.symbol {
        if risk.loading {
            "ML: loading decision-support snapshot...".to_string()
        } else if let Some(err) = &risk.error {
            format!("ML: {}", err)
        } else {
            format!(
                "ML {} | Regime {} | Action {} | Conf {:.0}% | Gate {} | ECE {:.4}",
                if risk.model_version.is_empty() { "n/a" } else { &risk.model_version },
                if risk.regime.is_empty() { "unknown" } else { &risk.regime },
                if risk.action_band.is_empty() { "watch" } else { &risk.action_band },
                risk.confidence * 100.0,
                if risk.confidence_gated { "ON" } else { "OFF" },
                risk.ece
            )
        }
    } else {
        "ML: awaiting symbol-linked snapshot (auto-refresh on symbol change)".to_string()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(strip_text, Style::default().fg(trend_color).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(
                ml_status_line,
                Style::default().fg(if risk.confidence_gated { WARN } else { OK }),
            )),
        ])
            .block(Block::default().borders(Borders::ALL).title("FOCUS")),
        chunks[0],
    );

    // Chart + signal side pane
    let chart_panes = Layout::horizontal([
        Constraint::Percentage(60),
        Constraint::Percentage(40),
    ])
    .split(chunks[1]);

    if let Some(ref data) = focus.data {
        match focus.chart_mode {
            ChartMode::Line => render_line_chart(frame, chart_panes[0], &data.line_series),
            ChartMode::Candle => render_candle_chart(frame, chart_panes[0], &data.candles),
        }

        // Signal explanation panel
        let alpha_text = if let Some(sig) = app.signals.iter().find(|s| s.symbol == data.symbol) {
            let action = action_label(sig.alpha_score);
            let action_color = action_color(sig.alpha_score);
            let marker_line = data
                .event_markers
                .first()
                .map(|m| format!("Marker: {} [{}]", m.headline, m.severity.to_uppercase()))
                .unwrap_or_else(|| "Marker: none".to_string());
            let impact_line = if data.impact_probabilities.is_empty() {
                "Impact: n/a".to_string()
            } else {
                data.impact_probabilities
                    .iter()
                    .map(|p| format!("{} U{:.0}%/F{:.0}%/D{:.0}%", p.horizon, p.prob_up * 100.0, p.prob_flat * 100.0, p.prob_down * 100.0))
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            let conf_line = data
                .confidence_band
                .first()
                .map(|c| format!("Conf band {:.0}%..{:.0}%", c.lower * 100.0, c.upper * 100.0))
                .unwrap_or_else(|| "Conf band n/a".to_string());
            vec![
                Line::from(Span::styled("ALPHA ANALYSIS", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
                Line::from(vec![
                    Span::styled(format!("{} ", data.symbol), Style::default().fg(OK).add_modifier(Modifier::BOLD)),
                    Span::styled(action, Style::default().fg(action_color).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(format!("Score: {}/100", sig.alpha_score)),
                Line::from(format!("Near: {:+.2} | Mid: {:+.2}", sig.bucket_1_5d, sig.bucket_20_60d)),
                Line::from("─────────────────"),
                Line::from(Span::styled(explain_signal(sig.alpha_score), Style::default().fg(NEUTRAL))),
                Line::from("─────────────────"),
                Line::from(format!("Trend: {}  Chg: {:+.2}%", data.trend_label, data.change_pct)),
                Line::from(marker_line),
                Line::from(conf_line),
                Line::from(impact_line),
            ]
        } else {
            vec![
                Line::from(Span::styled("ALPHA ANALYSIS", Style::default().fg(OK))),
                Line::from(format!("{} — no signal data", data.symbol)),
                Line::from(format!("Trend: {}  Chg: {:+.2}%", data.trend_label, data.change_pct)),
            ]
        };

        frame.render_widget(
            Paragraph::new(alpha_text)
                .block(Block::default().borders(Borders::ALL).title("Signal"))
                .wrap(Wrap { trim: true }),
            chart_panes[1],
        );
    } else {
        let msg = if focus.loading {
            "Loading focus data..."
        } else if focus.symbol.is_empty() {
            "Press 'f' on Radar to enter Focus mode"
        } else {
            "No data available"
        };
        frame.render_widget(empty_block(msg), chart_panes[0]);
        frame.render_widget(empty_block(""), chart_panes[1]);
    }

    // Digest headlines at bottom
    if let Some(ref data) = focus.data {
        if data.digest.is_empty() {
            frame.render_widget(empty_block("No digest items"), chunks[2]);
        } else {
            let digest_chunks = if app.news_expanded && data.digest.len() > 0 {
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(chunks[2])
            } else {
                Layout::horizontal([Constraint::Percentage(100)]).split(chunks[2])
            };

            // Digest table
            let header_row = Row::new(["Sym", "Sentiment", "Headline"].iter().map(|h| {
                Cell::from(*h).style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
            }));

            let rows: Vec<Row> = data.digest.iter().enumerate().map(|(i, item)| {
                let sc = sentiment_color(&item.sentiment);
                let is_sel = i == focus.selected_digest_idx;
                Row::new(vec![
                    Cell::from(item.symbol.clone()),
                    Cell::from(item.sentiment.clone()).style(Style::default().fg(sc)),
                    Cell::from(item.headline.clone()),
                ])
                .style(if is_sel { Style::default().bg(Color::DarkGray) } else { Style::default() })
            }).collect();

            let mut state = TableState::default();
            state.select(Some(focus.selected_digest_idx));

            let digest_table = Table::new(
                rows,
                [Constraint::Length(6), Constraint::Length(10), Constraint::Min(20)],
            )
            .header(header_row)
            .block(Block::default().borders(Borders::ALL).title("News Digest"))
            .highlight_style(Style::default().bg(Color::DarkGray));
            frame.render_stateful_widget(digest_table, digest_chunks[0], &mut state);

            // Detail pane if expanded and multi-column
            if digest_chunks.len() > 1 {
                if let Some(item) = data.digest.get(focus.selected_digest_idx) {
                    let sc = sentiment_color(&item.sentiment);
                    let glossary_str = if item.glossary_terms.is_empty() {
                        "none".to_string()
                    } else {
                        item.glossary_terms.iter().map(|t| format!("[{}]", t.to_uppercase())).collect::<Vec<_>>().join(" ")
                    };
                    let detail_lines = vec![
                        Line::from(Span::styled(&item.headline, Style::default().fg(sc).add_modifier(Modifier::BOLD))),
                        Line::from("─────────────"),
                        Line::from(vec![Span::styled("Summary: ", Style::default().fg(WARN)), Span::raw(&item.summary)]),
                        Line::from("─────────────"),
                        Line::from(vec![Span::styled("Why: ", Style::default().fg(WARN)), Span::raw(&item.why_it_matters)]),
                        Line::from("─────────────"),
                        Line::from(vec![Span::styled("Terms: ", Style::default().fg(WARN)), Span::raw(glossary_str)]),
                    ];
                    frame.render_widget(
                        Paragraph::new(detail_lines)
                            .block(Block::default().borders(Borders::ALL).title("Digest Detail"))
                            .wrap(Wrap { trim: true }),
                        digest_chunks[1],
                    );
                }
            }
        }
    } else {
        frame.render_widget(empty_block("Digest loading..."), chunks[2]);
    }
}

fn render_intel_board(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(area);

    let filter_line = format!(
        "Severity:{}  Conf>={:.0}%  Contradiction:{}  Window:{}  Generated:{}{}",
        if app.intel_filters.severity.is_empty() { "all" } else { &app.intel_filters.severity },
        app.intel_filters.confidence_floor * 100.0,
        if app.intel_filters.contradiction_only { "on" } else { "off" },
        app.intel_filters.window,
        if app.intel_board.generated_at.is_empty() { "n/a" } else { &app.intel_board.generated_at },
        if app.intel_board.loading { " [loading]" } else { "" },
    );
    frame.render_widget(
        Paragraph::new(filter_line)
            .block(Block::default().borders(Borders::ALL).title("INTEL FILTERS"))
            .style(Style::default().fg(WARN)),
        chunks[0],
    );

    if app.intel_board.events.is_empty() {
        let msg = app
            .intel_board
            .error
            .clone()
            .unwrap_or_else(|| "No events. Press 'r' to refresh intel board".to_string());
        frame.render_widget(empty_block(&msg), chunks[1]);
    } else {
        let rows = app
            .intel_board
            .events
            .iter()
            .map(|e| {
                let sev_color = match e.severity.as_str() {
                    "critical" => BAD,
                    "high" => WARN,
                    "medium" => OK,
                    _ => NEUTRAL,
                };
                Row::new(vec![
                    Cell::from(e.severity.to_uppercase()).style(Style::default().fg(sev_color)),
                    Cell::from(format!("{:.0}%", e.confidence * 100.0)),
                    Cell::from(format!("{:.0}%", e.contradiction_score * 100.0)),
                    Cell::from(e.symbol.clone()),
                    Cell::from(relative_time(&e.published_at)),
                    Cell::from(e.title.clone()),
                ])
            })
            .collect::<Vec<_>>();

        let header = Row::new(["SEV", "CONF", "CNTR", "SYM", "AGE", "HEADLINE"].iter().map(|h| {
            Cell::from(*h).style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
        }));
        let mut state = TableState::default();
        state.select(Some(app.intel_board.selected_idx));
        let table = Table::new(
            rows,
            [
                Constraint::Length(8),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Min(20),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("INTEL BOARD"))
        .highlight_style(Style::default().bg(Color::DarkGray));
        frame.render_stateful_widget(table, chunks[1], &mut state);
    }

    let footer = format!(
        "Row {} of {} (page {}, total {}) | next_cursor={} | Enter/e detail | v severity | +/- confidence | x contradiction | n next",
        app.intel_board.selected_idx + 1,
        app.intel_board.events.len(),
        app.intel_board.count,
        app.intel_board.total,
        if app.intel_board.next_cursor.is_empty() {
            "none".to_string()
        } else {
            app.intel_board.next_cursor.clone()
        }
    );
    frame.render_widget(Paragraph::new(footer).style(Style::default().fg(NEUTRAL)), chunks[2]);
}

fn render_event_detail(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(10),
        Constraint::Length(8),
    ])
    .split(area);

    if app.event_detail.loading {
        frame.render_widget(empty_block("Loading event detail..."), area);
        return;
    }
    if let Some(err) = &app.event_detail.error {
        frame.render_widget(empty_block(err), area);
        return;
    }
    let Some(detail) = app.event_detail.data.as_ref() else {
        frame.render_widget(empty_block("No event selected. Use Intel Board and press Enter/e"), area);
        return;
    };

    let summary = detail.event.as_ref();
    let summary_line = if let Some(ev) = summary {
        format!(
            "{} {} | severity {} | conf {:.0}% | contradiction {:.0}% | {}",
            ev.symbol,
            ev.title,
            ev.severity.to_uppercase(),
            ev.confidence * 100.0,
            ev.contradiction_score * 100.0,
            relative_time(&ev.published_at)
        )
    } else {
        "Unknown event".to_string()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled("EVENT SUMMARY", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
            Line::from(summary_line),
            Line::from(format!("Evidence-first note: {}", summary.map(|s| s.why_it_matters.clone()).unwrap_or_default())),
            Line::from(format!("Update delta: {}", detail.what_changed_since_last_update)),
        ])
        .block(Block::default().borders(Borders::ALL)),
        chunks[0],
    );

    let body = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).split(chunks[1]);

    let claim_rows = detail
        .claims
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let status_color = match c.verification_status.as_str() {
                "verified" => OK,
                "contradicted" => BAD,
                _ => WARN,
            };
            Row::new(vec![
                Cell::from((i + 1).to_string()),
                Cell::from(c.verification_status.clone()).style(Style::default().fg(status_color)),
                Cell::from(c.claim_type.clone()),
                Cell::from(format!("{:.0}%", c.confidence * 100.0)),
                Cell::from(c.claim_text.clone()),
            ])
        })
        .collect::<Vec<_>>();
    let mut claim_state = TableState::default();
    claim_state.select(Some(app.event_detail.selected_claim_idx));
    let claim_table = Table::new(
        claim_rows,
        [
            Constraint::Length(3),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Min(15),
        ],
    )
    .header(Row::new(["#", "STATUS", "TYPE", "CONF", "CLAIM"].iter().map(|h| {
        Cell::from(*h).style(Style::default().fg(WARN).add_modifier(Modifier::BOLD))
    })))
    .block(Block::default().borders(Borders::ALL).title("CLAIMS"))
    .highlight_style(Style::default().bg(Color::DarkGray));
    frame.render_stateful_widget(claim_table, body[0], &mut claim_state);

    let evidence_lines = if let Some(claim) = detail.claims.get(app.event_detail.selected_claim_idx) {
        if let Some(ev) = claim.evidence.get(app.event_detail.selected_evidence_idx) {
            vec![
                Line::from(Span::styled("EVIDENCE", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
                Line::from(format!("Source: {} ({:.0}%)", ev.source, ev.reliability * 100.0)),
                Line::from(format!("Published: {}", ev.published_at)),
                Line::from(format!("Quote: {}", ev.quote)),
                Line::from(format!("URL: {}", ev.url)),
                Line::from(format!("Citations: {}", detail.citations.len())),
            ]
        } else {
            vec![Line::from("No evidence row selected")]
        }
    } else {
        vec![Line::from("No claim selected")]
    };
    frame.render_widget(
        Paragraph::new(evidence_lines)
            .block(Block::default().borders(Borders::ALL).title("EVIDENCE"))
            .wrap(Wrap { trim: true }),
        body[1],
    );

    let bottom = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(chunks[2]);
    let impact_lines = if detail.impacts.is_empty() {
        vec![Line::from("No impact rows")]
    } else {
        detail
            .impacts
            .iter()
            .map(|i| {
                Line::from(format!(
                    "{}  U{:.0}%/F{:.0}%/D{:.0}%  ER {:+.2}%  DR {:.2}%",
                    i.horizon,
                    i.prob_up * 100.0,
                    i.prob_flat * 100.0,
                    i.prob_down * 100.0,
                    i.expected_return * 100.0,
                    i.downside_risk * 100.0
                ))
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(impact_lines)
            .block(Block::default().borders(Borders::ALL).title("IMPACT")),
        bottom[0],
    );

    let timeline_lines = if detail.timeline.is_empty() {
        vec![Line::from("No timeline entries")]
    } else {
        detail
            .timeline
            .iter()
            .rev()
            .take(4)
            .map(|t| Line::from(format!("{} {} {}", relative_time(&t.timestamp), t.status, t.change_note)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(timeline_lines)
            .block(Block::default().borders(Borders::ALL).title("TIMELINE"))
            .wrap(Wrap { trim: true }),
        bottom[1],
    );
}

fn render_portfolio_risk(frame: &mut Frame, area: Rect, app: &App) {
    let risk = &app.portfolio_risk;
    if risk.loading {
        frame.render_widget(empty_block("Loading ML portfolio risk snapshot..."), area);
        return;
    }
    if let Some(err) = &risk.error {
        frame.render_widget(empty_block(err), area);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Min(6),
    ])
    .split(area);

    let header = vec![
        Line::from(Span::styled("PORTFOLIO RISK ADVISORY (ML)", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
        Line::from(format!(
            "Symbol {} | Model {} | Lineage {}",
            if risk.symbol.is_empty() { "n/a" } else { &risk.symbol },
            if risk.model_version.is_empty() { "n/a" } else { &risk.model_version },
            if risk.model_lineage.is_empty() { "n/a" } else { &risk.model_lineage }
        )),
        Line::from(format!(
            "Regime {} (mom {:+.3}, vol {:.3}, liq {:.3}) | Action {} | Exposure {}",
            if risk.regime.is_empty() { "unknown" } else { &risk.regime },
            risk.regime_momentum,
            risk.regime_realized_vol_proxy,
            risk.regime_liquidity_proxy,
            if risk.action_band.is_empty() { "watch" } else { &risk.action_band },
            if risk.exposure_band.is_empty() { "n/a" } else { &risk.exposure_band }
        )),
        Line::from(format!(
            "Expected Return {:+.2}% | Downside Risk {:.2}% | Confidence {:.0}%",
            risk.expected_return * 100.0,
            risk.downside_risk * 100.0,
            risk.confidence * 100.0
        )),
        Line::from(format!(
            "Regime Probabilities: UP {:.0}%  FLAT {:.0}%  DOWN {:.0}%",
            risk.prob_up * 100.0,
            risk.prob_flat * 100.0,
            risk.prob_down * 100.0
        )),
        Line::from(format!(
            "Suggested Size Band {} | Max Single {:.1}% | Stop Review {}",
            risk.suggested_size_band,
            risk.max_single_position_pct * 100.0,
            if risk.stop_review_required { "YES" } else { "NO" }
        )),
        Line::from(format!(
            "Confidence Floor {:.0}% | Gated {} | Contract {}",
            risk.confidence_floor * 100.0,
            if risk.confidence_gated { "YES" } else { "NO" },
            if risk.feature_contract_status.is_empty() { "n/a" } else { &risk.feature_contract_status }
        )),
        Line::from(format!(
            "Updated: {}",
            risk.updated_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "n/a".to_string())
        )),
        Line::from(format!(
            "Model Snapshot At: {}",
            if risk.model_generated_at.is_empty() {
                "n/a".to_string()
            } else {
                risk.model_generated_at.clone()
            }
        )),
    ];
    frame.render_widget(
        Paragraph::new(header)
            .block(Block::default().borders(Borders::ALL).title("RISK OVERVIEW"))
            .wrap(Wrap { trim: true }),
        chunks[0],
    );

    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("DOWN RISK"))
            .ratio(risk.prob_down.clamp(0.0, 1.0))
            .gauge_style(Style::default().fg(if risk.prob_down > 0.5 { BAD } else { WARN })),
        chunks[1],
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("CONFIDENCE"))
            .ratio(risk.confidence.clamp(0.0, 1.0))
            .gauge_style(Style::default().fg(if risk.confidence > 0.65 { OK } else { WARN })),
        chunks[2],
    );

    let warning = risk
        .concentration_warning
        .clone()
        .unwrap_or_else(|| "No concentration warning".to_string());

    let horizon_lines = if risk.horizons.is_empty() {
        vec![Line::from("No horizon rows")]
    } else {
        risk.horizons
            .iter()
            .map(|h| {
                Line::from(format!(
                    "{}  U{:.0}%/F{:.0}%/D{:.0}%  ER {:+.2}%  DR {:.2}%  {}",
                    h.horizon,
                    h.prob_up * 100.0,
                    h.prob_flat * 100.0,
                    h.prob_down * 100.0,
                    h.expected_return * 100.0,
                    h.downside_risk * 100.0,
                    h.action_band,
                ))
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        Paragraph::new(
            vec![
                Line::from(Span::styled("POLICY + CALIBRATION", Style::default().fg(WARN).add_modifier(Modifier::BOLD))),
                Line::from(format!(
                    "Calibration: samples {} | ECE {:.4} | Brier {:.4} | Hit {:.1}% | Drift {:.4} | Model ECE {:.4} | Model Drift {:.4}",
                    risk.sample_count,
                    risk.ece,
                    risk.brier_score,
                    risk.hit_rate * 100.0,
                    risk.confidence_drift,
                    risk.model_calibration_error,
                    risk.model_confidence_drift
                )),
                Line::from(format!(
                    "Calibration Updated: {}",
                    if risk.calibration_updated_at.is_empty() {
                        "n/a".to_string()
                    } else {
                        risk.calibration_updated_at.clone()
                    }
                )),
                Line::from("Decision-support only. No auto-execution is enabled."),
                Line::from(format!("Concentration: {}", warning)),
                Line::from("Horizon Plan:"),
            ]
            .into_iter()
            .chain(horizon_lines.into_iter())
            .collect::<Vec<_>>(),
        )
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true }),
        chunks[3],
    );
}

fn render_line_chart(frame: &mut Frame, area: Rect, points: &[PricePoint]) {
    if points.is_empty() {
        frame.render_widget(empty_block("No price data"), area);
        return;
    }

    let data: Vec<(f64, f64)> = points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.price.is_finite())
        .map(|(i, p)| (i as f64, p.price))
        .collect();

    if data.is_empty() {
        frame.render_widget(empty_block("No valid price data"), area);
        return;
    }

    let min_price = data.iter().map(|(_, p)| *p).fold(f64::INFINITY, f64::min);
    let max_price = data.iter().map(|(_, p)| *p).fold(f64::NEG_INFINITY, f64::max);
    let price_range = (max_price - min_price).max(0.01);
    let y_min = min_price - price_range * 0.05;
    let y_max = max_price + price_range * 0.05;
    let n = data.len() as f64;

    // Timestamp labels: first, mid, last
    let first_ts = points.first().map(|p| p.ts.get(..16).unwrap_or(&p.ts).to_string()).unwrap_or_default();
    let mid_ts = points.get(points.len() / 2).map(|p| p.ts.get(..16).unwrap_or(&p.ts).to_string()).unwrap_or_default();
    let last_ts = points.last().map(|p| p.ts.get(..16).unwrap_or(&p.ts).to_string()).unwrap_or_default();

    let x_labels = vec![
        Span::styled(first_ts, Style::default().fg(NEUTRAL)),
        Span::styled(mid_ts, Style::default().fg(NEUTRAL)),
        Span::styled(last_ts, Style::default().fg(NEUTRAL)),
    ];

    let y_mid = (y_min + y_max) / 2.0;
    let y_labels = vec![
        Span::styled(format!("{:.2}", y_min), Style::default().fg(NEUTRAL)),
        Span::styled(format!("{:.2}", y_mid), Style::default().fg(NEUTRAL)),
        Span::styled(format!("{:.2}", y_max), Style::default().fg(NEUTRAL)),
    ];

    let dataset = Dataset::default()
        .graph_type(GraphType::Line)
        .style(Style::default().fg(OK))
        .data(&data);

    let chart = Chart::new(vec![dataset])
        .block(Block::default().borders(Borders::ALL).title("Price (Line)"))
        .x_axis(
            Axis::default()
                .bounds([0.0, n.max(1.0)])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .bounds([y_min, y_max])
                .labels(y_labels),
        );

    frame.render_widget(chart, area);
}

fn render_candle_chart(frame: &mut Frame, area: Rect, candles: &[Candle]) {
    if candles.is_empty() {
        frame.render_widget(empty_block("No candle data"), area);
        return;
    }

    let n = candles.len() as f64;
    let min_low = candles.iter().map(|c| c.low).filter(|v| v.is_finite()).fold(f64::INFINITY, f64::min);
    let max_high = candles.iter().map(|c| c.high).filter(|v| v.is_finite()).fold(f64::NEG_INFINITY, f64::max);

    if !min_low.is_finite() || !max_high.is_finite() {
        frame.render_widget(empty_block("No valid candle data"), area);
        return;
    }

    let y_range = (max_high - min_low).max(0.01);
    let y_min = min_low - y_range * 0.05;
    let y_max = max_high + y_range * 0.05;

    let canvas = Canvas::default()
        .block(Block::default().borders(Borders::ALL).title("Price (Candle)"))
        .x_bounds([0.0, n])
        .y_bounds([y_min, y_max])
        .marker(Marker::Braille)
        .paint(|ctx| {
            for (i, candle) in candles.iter().enumerate() {
                let x = i as f64 + 0.5;
                let body_color = if candle.close >= candle.open { Color::Green } else { Color::Red };

                // Wick
                ctx.draw(&CanvasLine {
                    x1: x, y1: candle.low,
                    x2: x, y2: candle.high,
                    color: NEUTRAL,
                });

                // Body (as a small rectangle)
                let (body_top, body_bot) = if candle.close >= candle.open {
                    (candle.close, candle.open)
                } else {
                    (candle.open, candle.close)
                };
                let width = 0.4;
                ctx.draw(&Rectangle {
                    x: x - width / 2.0,
                    y: body_bot,
                    width,
                    height: (body_top - body_bot).max(0.001),
                    color: body_color,
                });
            }
        });

    frame.render_widget(canvas, area);
}

fn render_keymap_overlay(frame: &mut Frame, area: Rect) {
    let popup_width = 60u16.min(area.width.saturating_sub(4));
    let popup_height = 24u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    let lines = vec![
        Line::from(Span::styled("KEYMAP", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
        Line::from("─────────────────────────────────────────"),
        Line::from(vec![Span::styled("1-9", Style::default().fg(WARN)), Span::raw("       switch view")]),
        Line::from(vec![Span::styled("Tab/→", Style::default().fg(WARN)), Span::raw("     next view")]),
        Line::from(vec![Span::styled("BackTab/←", Style::default().fg(WARN)), Span::raw(" prev view")]),
        Line::from(vec![Span::styled("r", Style::default().fg(WARN)), Span::raw("         refresh data")]),
        Line::from(vec![Span::styled("q", Style::default().fg(WARN)), Span::raw("         quit (confirm)")]),
        Line::from(vec![Span::styled("Esc", Style::default().fg(WARN)), Span::raw("       cancel / close overlay")]),
        Line::from("── Radar ─────────────────────────────────"),
        Line::from(vec![Span::styled("Up/Down", Style::default().fg(WARN)), Span::raw("   select signal")]),
        Line::from(vec![Span::styled("Enter", Style::default().fg(WARN)), Span::raw("     signal detail")]),
        Line::from(vec![Span::styled("f", Style::default().fg(WARN)), Span::raw("         enter Focus view")]),
        Line::from("── News ──────────────────────────────────"),
        Line::from(vec![Span::styled("Up/Down", Style::default().fg(WARN)), Span::raw("   scroll")]),
        Line::from(vec![Span::styled("Enter", Style::default().fg(WARN)), Span::raw("     toggle split pane")]),
        Line::from(vec![Span::styled("/", Style::default().fg(WARN)), Span::raw("         filter by sentiment")]),
        Line::from("── Focus ─────────────────────────────────"),
        Line::from(vec![Span::styled("[/]", Style::default().fg(WARN)), Span::raw("       prev/next symbol")]),
        Line::from(vec![Span::styled("t", Style::default().fg(WARN)), Span::raw("         cycle timeframe")]),
        Line::from(vec![Span::styled("c", Style::default().fg(WARN)), Span::raw("         toggle chart mode")]),
        Line::from(vec![Span::styled("Up/Down", Style::default().fg(WARN)), Span::raw("   scroll digest")]),
        Line::from("── Intel ─────────────────────────────────"),
        Line::from(vec![Span::styled("i", Style::default().fg(WARN)), Span::raw("         open intel board")]),
        Line::from(vec![Span::styled("Enter/e", Style::default().fg(WARN)), Span::raw("   open event detail")]),
        Line::from(vec![Span::styled("v/+/-/x", Style::default().fg(WARN)), Span::raw("  severity/conf/contradiction filters")]),
        Line::from(vec![Span::styled("p", Style::default().fg(WARN)), Span::raw("         open portfolio risk")]),
        Line::from("── Global ────────────────────────────────"),
        Line::from(vec![Span::styled("g", Style::default().fg(WARN)), Span::raw("         glossary panel")]),
        Line::from(vec![Span::styled("?", Style::default().fg(WARN)), Span::raw("         this keymap")]),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(OK)).title("KEYS"))
            .style(Style::default().bg(Color::Black)),
        popup_area,
    );
}

fn render_glossary_panel(frame: &mut Frame, area: Rect, app: &App) {
    // Right-side panel, 30% width
    let panel_width = (area.width * 30 / 100).max(30).min(area.width.saturating_sub(2));
    let panel_area = Rect::new(
        area.x + area.width.saturating_sub(panel_width),
        area.y,
        panel_width,
        area.height,
    );

    // Collect relevant glossary terms from selected digest item, or show all
    let relevant_keys: Vec<String> = if let Some(ref data) = app.focus.data {
        if let Some(item) = data.digest.get(app.focus.selected_digest_idx) {
            item.glossary_terms.clone()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let glossary = crate::client::GLOSSARY_TERMS;
    let terms_to_show: Vec<(&str, &str)> = if relevant_keys.is_empty() {
        glossary.iter().map(|(k, v)| (*k, *v)).collect()
    } else {
        glossary
            .iter()
            .filter(|(k, _)| relevant_keys.iter().any(|rk| rk.as_str() == *k))
            .map(|(k, v)| (*k, *v))
            .collect()
    };

    let mut lines = vec![
        Line::from(Span::styled("GLOSSARY", Style::default().fg(OK).add_modifier(Modifier::BOLD))),
        Line::from("─────────────────────────────"),
    ];

    for (term, def) in &terms_to_show {
        lines.push(Line::from(Span::styled(term.to_uppercase(), Style::default().fg(WARN).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(format!("  {}", def)));
        lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(WARN)).title("GLOSSARY"))
            .style(Style::default().bg(Color::Black))
            .wrap(Wrap { trim: true }),
        panel_area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    if app.pending_quit {
        frame.render_widget(
            Paragraph::new("CONFIRM QUIT — press q again | ESC to cancel")
                .alignment(Alignment::Center)
                .style(Style::default().fg(BAD).add_modifier(Modifier::BOLD)),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(app.footer_hint())
                .alignment(Alignment::Center)
                .style(Style::default().fg(NEUTRAL)),
            area,
        );
    }
}

fn sentiment_color(sentiment: &str) -> Color {
    match sentiment {
        "positive" => Color::Green,
        "negative" => Color::Red,
        _ => Color::Gray,
    }
}

fn relative_time(iso: &str) -> String {
    use chrono::{DateTime, Utc};
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        let secs = (Utc::now() - dt.with_timezone(&Utc)).num_seconds().max(0);
        if secs < 60 {
            return format!("{}s", secs);
        }
        if secs < 3600 {
            return format!("{}m", secs / 60);
        }
        if secs < 86400 {
            return format!("{}h", secs / 3600);
        }
        return format!("{}d", secs / 86400);
    }
    "?".to_string()
}

fn alpha_bar(score: u8, width: usize) -> String {
    let filled = ((score as usize) * width) / 100;
    let mut s = String::with_capacity(width);
    for i in 0..width {
        if i < filled {
            s.push('█');
        } else {
            s.push('░');
        }
    }
    s
}

fn alpha_heat(score: u8) -> Color {
    match score {
        0..=24 => CARD_BG_AVOID,
        25..=69 => CARD_BG_WATCH,
        _ => CARD_BG_BUY,
    }
}

fn metric_block<'a>(title: &'a str, value: &'a str, color: Color) -> Paragraph<'a> {
    Paragraph::new(value)
        .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(title, Style::default().fg(WARN))),
        )
}

fn empty_block<'a>(text: &'a str) -> Paragraph<'a> {
    Paragraph::new(text)
        .style(Style::default().fg(WARN))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled("NO DATA", Style::default().fg(WARN))),
        )
        .alignment(Alignment::Center)
}

fn action_label(score: u8) -> &'static str {
    match score {
        70..=100 => "BUY",
        40..=69 => "WATCH",
        _ => "AVOID",
    }
}

fn action_color(score: u8) -> Color {
    match score {
        70..=100 => OK,
        40..=69 => WARN,
        _ => BAD,
    }
}

fn explain_signal(score: u8) -> &'static str {
    match score {
        70..=100 => "Strong positive setup from the model.",
        40..=69 => "Mixed setup. Wait for confirmation.",
        _ => "Weak setup. Preserve capital or hedge.",
    }
}

fn market_posture(app: &App) -> &'static str {
    if app.threat.kill_switch_active {
        return "RISK-OFF (KILL SWITCH)";
    }
    if app.signals.is_empty() {
        return "NO SIGNALS";
    }

    let avg = app
        .signals
        .iter()
        .map(|s| s.alpha_score as u32)
        .sum::<u32>() as f64
        / app.signals.len() as f64;

    if avg >= 65.0 {
        "BULLISH"
    } else if avg >= 45.0 {
        "NEUTRAL"
    } else {
        "DEFENSIVE"
    }
}
