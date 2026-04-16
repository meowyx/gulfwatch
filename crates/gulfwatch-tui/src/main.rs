mod app;
mod ui;

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use app::{App, View};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use gulfwatch_core::alert::AlertEngine;
use gulfwatch_core::detections::{
    AuthorityChangeDetection, CrossProgramCorrelationDetection, DefaultAccountStateFrozenDetection,
    Detection, FailedTxClusterDetection, LargeTransferDetection, PermanentDelegateDetection,
    TransferFeeAuthorityChangeDetection, TransferHookUpgradeDetection,
};
use gulfwatch_core::pipeline::{run_alert_recorder, run_processing_worker, WorkerHandle};
use gulfwatch_core::AppState;
use gulfwatch_ingest::client::IngestConfig;
use gulfwatch_ingest::SolanaIngestClient;
use ratatui::prelude::*;

#[tokio::main]
async fn main() -> io::Result<()> {
    load_dotenv();

    let (state, ingest_rx) = AppState::new(1024, parse_rolling_window_minutes());

    let programs_str = std::env::var("MONITOR_PROGRAMS")
        .or_else(|_| std::env::var("MONITOR_PROGRAM"))
        .expect("Set MONITOR_PROGRAM or MONITOR_PROGRAMS in .env");
    let program_ids: Vec<String> = programs_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if program_ids.is_empty() {
        panic!("MONITOR_PROGRAM(S) must contain at least one program id");
    }
    for pid in &program_ids {
        state.add_program(pid.clone()).await;
    }

    let worker_handle = WorkerHandle::from(&state);
    let watched_accounts = parse_watched_accounts();
    let large_transfer_threshold = parse_large_transfer_threshold();
    let correlation_min_programs = parse_correlation_min_programs();
    let correlation_window_secs = parse_correlation_window_secs();
    let detections: Vec<Box<dyn Detection>> = vec![
        Box::new(AuthorityChangeDetection),
        Box::new(FailedTxClusterDetection::default()),
        Box::new(LargeTransferDetection::new(
            watched_accounts,
            large_transfer_threshold,
        )),
        Box::new(TransferHookUpgradeDetection),
        Box::new(PermanentDelegateDetection),
        Box::new(TransferFeeAuthorityChangeDetection),
        Box::new(DefaultAccountStateFrozenDetection),
        Box::new(CrossProgramCorrelationDetection::new(
            correlation_min_programs,
            correlation_window_secs,
            large_transfer_threshold,
        )),
    ];
    tokio::spawn(run_processing_worker(worker_handle, ingest_rx, detections));

    let ws_url = require_env("SOLANA_WS_URL");
    let rpc_url = require_env("SOLANA_RPC_URL");

    let ingest_config = IngestConfig {
        ws_url,
        rpc_url,
        program_ids,
        max_backoff_secs: 60,
    };
    let ingest_client = SolanaIngestClient::new(ingest_config, state.ingest_tx.clone());
    tokio::spawn(async move { ingest_client.run().await });

    tokio::spawn(run_alert_recorder(state.clone(), 500));

    let mut alert_engine = AlertEngine::new(
        state.alert_rules.clone(),
        state.windows.clone(),
        state.alert_broadcast.clone(),
        30,
    );
    tokio::spawn(async move { alert_engine.run(Duration::from_secs(5)).await });

    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3001".to_string())
        .parse()
        .expect("invalid LISTEN_ADDR");
    {
        let server_state = state.clone();
        tokio::spawn(async move {
            // Bind failure here means the port is taken (likely gulfwatch-server is
            // already running in another terminal). Silent on purpose — the TUI's
            // alternate screen would eat the message anyway, and the MCP user will
            // notice immediately when their tools fail to connect.
            let _ = gulfwatch_server::run_server(server_state, listen_addr).await;
        });
    }

    let mut app = App::new(state);
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app).await;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Drain any broadcast messages
        app.poll_updates();

        terminal.draw(|f| ui::draw(f, app))?;

        // Poll for events with a short timeout so we refresh frequently
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Global keys
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    _ => {}
                }

                // View-specific keys
                if !matches!(app.view, View::Dashboard) {
                    // Detail view: Esc or Backspace to go back
                    match key.code {
                        KeyCode::Esc | KeyCode::Backspace => app.close_detail(),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                        KeyCode::Left | KeyCode::Char('h') => app.prev_detail_tab(),
                        KeyCode::Right | KeyCode::Char('l') => app.next_detail_tab(),
                        _ => {}
                    }
                } else {
                    // Dashboard view
                    match key.code {
                        KeyCode::Tab => app.next_panel(),
                        KeyCode::BackTab => app.prev_panel(),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                        KeyCode::Enter => app.open_detail(),
                        KeyCode::Char(c @ '1'..='9') => {
                            let idx = (c as usize) - ('1' as usize);
                            app.select_program(Some(idx));
                        }
                        KeyCode::Char('a') => app.select_program(None),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} not set — add it to .env"))
}

fn parse_watched_accounts() -> HashSet<String> {
    std::env::var("WATCHED_ACCOUNTS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_large_transfer_threshold() -> u64 {
    std::env::var("LARGE_TRANSFER_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(u64::MAX)
}

fn parse_rolling_window_minutes() -> i64 {
    std::env::var("ROLLING_WINDOW_MINUTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10)
}

fn parse_correlation_min_programs() -> usize {
    std::env::var("CORRELATION_MIN_PROGRAMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
}

fn parse_correlation_window_secs() -> u64 {
    std::env::var("CORRELATION_WINDOW_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300)
}

fn load_dotenv() {
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let env_file = d.join(".env");
        if env_file.exists() {
            if let Ok(contents) = std::fs::read_to_string(&env_file) {
                for line in contents.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=') {
                        let key = key.trim();
                        let value = value.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var(key).is_err() {
                            // SAFETY: called before any threads are spawned
                            unsafe { std::env::set_var(key, value); }
                        }
                    }
                }
            }
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
}
