use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use gulfwatch_core::alert::AlertEngine;
use gulfwatch_core::detections::{
    AuthorityChangeDetection, CrossProgramCorrelationDetection, DefaultAccountStateFrozenDetection,
    Detection, FailedTxClusterDetection, LargeTransferDetection, PermanentDelegateDetection,
    TransferFeeAuthorityChangeDetection, TransferHookUpgradeDetection,
};
use gulfwatch_core::pipeline::{WorkerHandle, run_alert_recorder, run_processing_worker};
use gulfwatch_core::AppState;
use gulfwatch_ingest::{spawn_boot_idl_discovery, SolanaIngestClient, client::IngestConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Load .env before anything else
    load_dotenv();

    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Starting GulfWatch");

    let ws_url = require_env("SOLANA_WS_URL");
    let rpc_url = require_env("SOLANA_RPC_URL");
    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3001".to_string())
        .parse()
        .expect("invalid LISTEN_ADDR");

    let (state, ingest_rx) = AppState::new(1024, parse_rolling_window_minutes());

    let programs_str = std::env::var("MONITOR_PROGRAMS")
        .or_else(|_| std::env::var("MONITOR_PROGRAM"))
        .expect("Set MONITOR_PROGRAM or MONITOR_PROGRAMS in .env");
    for pid in programs_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        state.add_program(pid.clone()).await;
        info!(program_id = %pid, "Monitoring program");
    }

    let monitored = state.monitored_programs.read().await.clone();
    spawn_boot_idl_discovery(state.clone(), rpc_url.clone(), monitored.clone());

    let ingest_config = IngestConfig {
        ws_url,
        rpc_url,
        program_ids: monitored,
        max_backoff_secs: 60,
    };

    let ingest_client = SolanaIngestClient::new(ingest_config, state.ingest_tx.clone());
    let worker_handle = WorkerHandle::from(&state);

    let watched_accounts = parse_watched_accounts();
    let large_transfer_threshold = parse_large_transfer_threshold();
    if !watched_accounts.is_empty() && large_transfer_threshold != u64::MAX {
        info!(
            count = watched_accounts.len(),
            threshold = large_transfer_threshold,
            "Large-transfer detection armed"
        );
    }

    let correlation_min_programs = parse_correlation_min_programs();
    let correlation_window_secs = parse_correlation_window_secs();
    if correlation_min_programs > 1 {
        info!(
            min_programs = correlation_min_programs,
            window_secs = correlation_window_secs,
            "Cross-program correlation detection armed"
        );
    }

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
    tokio::spawn(async move { ingest_client.run().await });

    let mut alert_engine = AlertEngine::new(
        state.alert_rules.clone(),
        state.windows.clone(),
        state.alert_broadcast.clone(),
        30,
    );
    tokio::spawn(async move { alert_engine.run(Duration::from_secs(5)).await });

    tokio::spawn(run_alert_recorder(state.clone(), 500));

    info!("GulfWatch ready");
    gulfwatch_server::run_server(state, listen_addr)
        .await
        .expect("HTTP server failed");
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
