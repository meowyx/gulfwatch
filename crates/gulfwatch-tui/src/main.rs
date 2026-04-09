mod app;
mod ui;

use std::collections::HashSet;
use std::io;
use std::time::Duration;

use app::{App, View};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use gulfwatch_core::detections::{
    AuthorityChangeDetection, Detection, FailedTxClusterDetection, LargeTransferDetection,
};
use gulfwatch_core::pipeline::{run_processing_worker, WorkerHandle};
use gulfwatch_core::AppState;
use gulfwatch_ingest::client::IngestConfig;
use gulfwatch_ingest::SolanaIngestClient;
use ratatui::prelude::*;

#[tokio::main]
async fn main() -> io::Result<()> {
    load_dotenv();

    let (state, ingest_rx) = AppState::new(1024, 10);

    let program_id = require_env("MONITOR_PROGRAM");
    state.add_program(program_id.clone()).await;

    let worker_handle = WorkerHandle::from(&state);
    let watched_accounts = parse_watched_accounts();
    let large_transfer_threshold = parse_large_transfer_threshold();
    let detections: Vec<Box<dyn Detection>> = vec![
        Box::new(AuthorityChangeDetection),
        Box::new(FailedTxClusterDetection::default()),
        Box::new(LargeTransferDetection::new(
            watched_accounts,
            large_transfer_threshold,
        )),
    ];
    tokio::spawn(run_processing_worker(worker_handle, ingest_rx, detections));

    let ws_url = require_env("SOLANA_WS_URL");
    let rpc_url = require_env("SOLANA_RPC_URL");

    let ingest_config = IngestConfig {
        ws_url,
        rpc_url,
        program_ids: vec![program_id],
        max_backoff_secs: 60,
    };
    let ingest_client = SolanaIngestClient::new(ingest_config, state.ingest_tx.clone());
    tokio::spawn(async move { ingest_client.run().await });

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
                        KeyCode::Char('1') => { app.active_panel = 0; app.selected = 0; }
                        KeyCode::Char('2') => { app.active_panel = 1; app.selected = 0; }
                        KeyCode::Char('3') => { app.active_panel = 2; app.selected = 0; }
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
