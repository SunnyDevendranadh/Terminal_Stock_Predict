mod app;
mod client;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use app::{App, IntelRequest};
use client::{
    ClientConfig, EventIntelData, FocusBundleData, GrpcSignalClient, MlCalibrationStatusData, MlDecisionSupportData,
    NewsIntelBoardData, SignalSnapshot,
};
use crossterm::event::{self, Event as CEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, MissedTickBehavior};

const FRAME_TIME_MS: u64 = 16;
const IDLE_TIMEOUT_SECS: u64 = 300;

#[derive(Debug)]
enum StreamEvent {
    Snapshot(SignalSnapshot),
    Degraded(String),
    FocusBundle(FocusBundleData),
    FocusFailed(String),
    IntelBoard(NewsIntelBoardData),
    EventDetail(EventIntelData),
    IntelBoardFailed(String),
    EventDetailFailed(String),
    MlRisk(String, MlDecisionSupportData, MlCalibrationStatusData),
    MlRiskFailed(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ClientConfig::from_env()?;

    prompt_mtls_password(config.tls_enabled)?;

    let (refresh_tx, refresh_rx) = mpsc::channel::<()>(1);
    let (focus_tx, focus_rx) = mpsc::channel::<(String, String)>(8);
    let (intel_tx, intel_rx) = mpsc::channel::<IntelRequest>(16);

    let mut app = App::new(
        config.cache_path.clone(),
        config.refresh_interval,
        Duration::from_secs(IDLE_TIMEOUT_SECS),
        refresh_tx,
        focus_tx,
        intel_tx,
    )?;

    let (tx, mut rx) = mpsc::channel::<StreamEvent>(32);
    let signal_event_tx = tx.clone();
    let focus_event_tx = tx.clone();
    let intel_event_tx = tx.clone();
    tokio::spawn(run_signal_stream(config.clone(), signal_event_tx, refresh_rx));
    tokio::spawn(run_focus_fetcher(config.clone(), focus_rx, focus_event_tx));
    tokio::spawn(run_intel_fetcher(config.clone(), intel_rx, intel_event_tx));

    let mut terminal = setup_terminal()?;
    let run_result = run_event_loop(&mut terminal, &mut app, &mut rx).await;
    let restore_result = restore_terminal(&mut terminal);

    if let Err(ref err) = run_result {
        eprintln!("terminal-client exited with error: {err:#}");
    }

    restore_result?;

    run_result
}

async fn run_signal_stream(config: ClientConfig, tx: mpsc::Sender<StreamEvent>, mut refresh_rx: mpsc::Receiver<()>) {
    let mut client: Option<GrpcSignalClient> = None;

    loop {
        if client.is_none() {
            match GrpcSignalClient::connect(&config).await {
                Ok(connected) => {
                    client = Some(connected);
                }
                Err(err) => {
                    let _ = tx
                        .send(StreamEvent::Degraded(format!("connect failed: {err}")))
                        .await;
                    tokio::select! {
                        _ = sleep(config.refresh_interval) => {}
                        _ = refresh_rx.recv() => {}
                    }
                    continue;
                }
            }
        }

        if let Some(inner) = client.as_mut() {
            match inner.subscribe_signals().await {
                Ok(mut stream) => loop {
                    match stream.message().await {
                        Ok(Some(pb_snapshot)) => match GrpcSignalClient::map_stream_snapshot(pb_snapshot) {
                            Ok(snapshot) => {
                                let _ = tx.send(StreamEvent::Snapshot(snapshot)).await;
                            }
                            Err(err) => {
                                let _ = tx
                                    .send(StreamEvent::Degraded(format!(
                                        "stream decode failed: {err}"
                                    )))
                                    .await;
                            }
                        },
                        Ok(None) => {
                            let _ = tx
                                .send(StreamEvent::Degraded(
                                    "stream closed by server; reconnecting".to_string(),
                                ))
                                .await;
                            client = None;
                            break;
                        }
                        Err(err) => {
                            let _ = tx
                                .send(StreamEvent::Degraded(format!(
                                    "stream transport error: {err}"
                                )))
                                .await;
                            client = None;
                            break;
                        }
                    }
                },
                Err(err) => {
                    let _ = tx
                        .send(StreamEvent::Degraded(format!(
                            "stream unavailable ({err}); using polling fallback"
                        )))
                        .await;
                    match inner.fetch_snapshot_via_polling().await {
                        Ok(snapshot) => {
                            let _ = tx.send(StreamEvent::Snapshot(snapshot)).await;
                        }
                        Err(poll_err) => {
                            let _ = tx
                                .send(StreamEvent::Degraded(format!(
                                    "polling fallback failed: {poll_err}"
                                )))
                                .await;
                            client = None;
                        }
                    }
                }
            }
        }

        tokio::select! {
            _ = sleep(config.refresh_interval) => {}
            _ = refresh_rx.recv() => {}
        }
    }
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &mut mpsc::Receiver<StreamEvent>,
) -> Result<()> {
    let mut frame_tick = interval(Duration::from_millis(FRAME_TIME_MS));
    frame_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                if let Some(stream_event) = maybe_event {
                    match stream_event {
                        StreamEvent::Snapshot(snapshot) => {
                            if let Err(err) = app.apply_snapshot(snapshot) {
                                app.mark_degraded(format!("cache write failed: {err}"));
                            }
                        }
                        StreamEvent::Degraded(reason) => {
                            app.mark_degraded(reason);
                        }
                        StreamEvent::FocusBundle(data) => {
                            app.apply_focus_bundle(data);
                        }
                        StreamEvent::FocusFailed(reason) => {
                            app.focus.loading = false;
                            app.focus.error = Some(reason);
                        }
                        StreamEvent::IntelBoard(board) => {
                            app.apply_intel_board(
                                board.events,
                                board.count,
                                board.total,
                                board.next_cursor,
                                board.stale,
                                board.generated_at,
                            );
                        }
                        StreamEvent::EventDetail(detail) => {
                            app.apply_event_detail(detail);
                        }
                        StreamEvent::IntelBoardFailed(reason) => {
                            app.mark_intel_board_failed(reason);
                        }
                        StreamEvent::EventDetailFailed(reason) => {
                            app.mark_event_detail_failed(reason);
                        }
                        StreamEvent::MlRisk(symbol, ml, calib) => {
                            app.apply_ml_risk(symbol, ml, calib);
                        }
                        StreamEvent::MlRiskFailed(reason) => {
                            app.mark_ml_risk_failed(reason);
                        }
                    }
                }
            }
            _ = frame_tick.tick() => {
                while event::poll(Duration::from_millis(0)).context("failed polling keyboard input")? {
                    if let CEvent::Key(key) = event::read().context("failed reading keyboard input")? {
                        if key.kind == KeyEventKind::Press {
                            app.handle_key(key);
                        }
                    }
                }

                app.check_idle_timeout();

                terminal.draw(|frame| {
                    ui::draw(frame, app);
                }).context("failed rendering terminal frame")?;

                if app.should_quit {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn run_intel_fetcher(
    config: ClientConfig,
    mut intel_rx: mpsc::Receiver<IntelRequest>,
    event_tx: mpsc::Sender<StreamEvent>,
) {
    let mut client: Option<GrpcSignalClient> = None;

    while let Some(request) = intel_rx.recv().await {
        if client.is_none() {
            match GrpcSignalClient::connect(&config).await {
                Ok(c) => client = Some(c),
                Err(err) => {
                    let msg = format!("intel connect: {err}");
                    let event = match &request {
                        IntelRequest::Board { .. } => StreamEvent::IntelBoardFailed(msg),
                        IntelRequest::EventDetail { .. } => StreamEvent::EventDetailFailed(msg),
                        IntelRequest::MlRisk { .. } => StreamEvent::MlRiskFailed(msg),
                    };
                    let _ = event_tx.send(event).await;
                    continue;
                }
            }
        }

        if let Some(ref mut c) = client {
            match request {
                IntelRequest::Board {
                    symbols,
                    severity,
                    sentiment,
                    window,
                    limit,
                    cursor,
                } => {
                    match c
                        .fetch_news_intel_board(
                            &symbols,
                            &severity,
                            &sentiment,
                            &window,
                            limit,
                            &cursor,
                        )
                        .await
                    {
                        Ok(board) => {
                            let _ = event_tx.send(StreamEvent::IntelBoard(board)).await;
                        }
                        Err(err) => {
                            client = None;
                            let _ = event_tx
                                .send(StreamEvent::IntelBoardFailed(format!("intel board: {err}")))
                                .await;
                        }
                    }
                }
                IntelRequest::EventDetail { event_id, symbols } => {
                    match c.fetch_event_intel(&event_id, &symbols).await {
                        Ok(detail) => {
                            let _ = event_tx.send(StreamEvent::EventDetail(detail)).await;
                        }
                        Err(err) => {
                            client = None;
                            let _ = event_tx
                                .send(StreamEvent::EventDetailFailed(format!("event detail: {err}")))
                                .await;
                        }
                    }
                }
                IntelRequest::MlRisk {
                    symbol,
                    features,
                    model_version,
                    confidence_floor,
                } => {
                    match c
                        .fetch_ml_decision_support(&symbol, &features, &model_version, confidence_floor)
                        .await
                    {
                        Ok(ml) => match c.fetch_ml_calibration_status().await {
                            Ok(calib) => {
                                let _ = event_tx.send(StreamEvent::MlRisk(symbol, ml, calib)).await;
                            }
                            Err(err) => {
                                let _ = event_tx
                                    .send(StreamEvent::MlRiskFailed(format!(
                                        "ml calibration status: {err}"
                                    )))
                                    .await;
                            }
                        },
                        Err(err) => {
                            client = None;
                            let _ = event_tx
                                .send(StreamEvent::MlRiskFailed(format!("ml decision support: {err}")))
                                .await;
                        }
                    }
                }
            }
        }
    }
}

async fn run_focus_fetcher(
    config: ClientConfig,
    mut focus_rx: mpsc::Receiver<(String, String)>,
    event_tx: mpsc::Sender<StreamEvent>,
) {
    let mut client: Option<GrpcSignalClient> = None;

    while let Some((symbol, timeframe)) = focus_rx.recv().await {
        // Drain any queued requests; use only the latest
        let mut latest_symbol = symbol;
        let mut latest_timeframe = timeframe;
        while let Ok((sym, tf)) = focus_rx.try_recv() {
            latest_symbol = sym;
            latest_timeframe = tf;
        }

        if client.is_none() {
            match GrpcSignalClient::connect(&config).await {
                Ok(c) => client = Some(c),
                Err(err) => {
                    let _ = event_tx
                        .send(StreamEvent::FocusFailed(format!("focus connect: {err}")))
                        .await;
                    continue;
                }
            }
        }

        if let Some(ref mut c) = client {
            match c.fetch_focus_bundle(&latest_symbol, &latest_timeframe).await {
                Ok(data) => {
                    let _ = event_tx.send(StreamEvent::FocusBundle(data)).await;
                }
                Err(err) => {
                    client = None;
                    let _ = event_tx
                        .send(StreamEvent::FocusFailed(format!("focus fetch: {err}")))
                        .await;
                }
            }
        }
    }
}

fn prompt_mtls_password(tls_enabled: bool) -> Result<()> {
    if !tls_enabled {
        return Ok(());
    }

    let passphrase = rpassword::prompt_password("mTLS certificate passphrase: ")
        .context("failed to read certificate passphrase")?;

    if passphrase.trim().is_empty() {
        eprintln!("Warning: empty mTLS passphrase — skipping certificate validation");
        return Ok(());
    }

    if let Ok(expected) = std::env::var("TC_TLS_CERT_PASSWORD") {
        if passphrase != expected {
            return Err(anyhow::anyhow!("certificate passphrase validation failed"));
        }
    }

    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("failed enabling raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed entering alternate screen")?;
    stdout
        .execute(crossterm::event::EnableMouseCapture)
        .context("failed enabling mouse capture")?;

    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed creating terminal backend")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed disabling raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed leaving alternate screen")?;
    terminal
        .backend_mut()
        .execute(crossterm::event::DisableMouseCapture)
        .context("failed disabling mouse capture")?;
    terminal.show_cursor().context("failed restoring cursor")?;
    Ok(())
}
