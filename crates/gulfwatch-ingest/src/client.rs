use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gulfwatch_core::Transaction;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::parser;

#[derive(Debug, Clone)]
pub struct IngestConfig {
    pub ws_url: String,
    pub program_ids: Vec<String>,
    pub max_backoff_secs: u64,
    pub rpc_url: String,
}

pub struct SolanaIngestClient {
    config: IngestConfig,
    tx_sender: mpsc::Sender<Transaction>,
}

impl SolanaIngestClient {
    pub fn new(config: IngestConfig, tx_sender: mpsc::Sender<Transaction>) -> Self {
        Self { config, tx_sender }
    }

    pub async fn run(&self) {
        let mut backoff_secs = 1u64;

        loop {
            info!("Connecting to Solana WebSocket RPC: {}", self.config.ws_url);

            match self.connect_and_stream().await {
                Ok(()) => {
                    info!("WebSocket stream ended cleanly, reconnecting...");
                    backoff_secs = 1;
                }
                Err(e) => {
                    error!("WebSocket error: {e}, reconnecting in {backoff_secs}s...");
                }
            }

            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(self.config.max_backoff_secs);
        }
    }

    async fn connect_and_stream(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (ws_stream, _response) = connect_async(&self.config.ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        info!("Connected to Solana WebSocket RPC");

        for (i, program_id) in self.config.program_ids.iter().enumerate() {
            let subscribe_msg = json!({
                "jsonrpc": "2.0",
                "id": i + 1,
                "method": "logsSubscribe",
                "params": [
                    { "mentions": [program_id] },
                    { "commitment": "confirmed" }
                ]
            });

            write
                .send(Message::Text(subscribe_msg.to_string().into()))
                .await?;
            info!("Subscribed to logs for program: {program_id}");
        }

        while let Some(msg) = read.next().await {
            let msg = msg?;

            match msg {
                Message::Text(text) => {
                    self.handle_message(&text).await;
                }
                Message::Ping(data) => {
                    write.send(Message::Pong(data)).await?;
                }
                Message::Close(_) => {
                    info!("WebSocket closed by server");
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_message(&self, text: &str) {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to parse WebSocket message: {e}");
                return;
            }
        };

        let method = msg.get("method").and_then(|v| v.as_str());
        if method != Some("logsNotification") {
            return;
        }

        let signature = match parser::parse_log_signature(&msg) {
            Some(sig) => sig,
            None => {
                warn!("logsNotification missing signature");
                return;
            }
        };

        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut attempts = 0;
        loop {
            attempts += 1;
            match self.fetch_transaction(&signature).await {
                Ok(raw) => {
                    if raw.get("result").map_or(false, |r| r.is_null()) {
                        if attempts < 3 {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                        warn!(signature = %signature, "Transaction not found after retries");
                        break;
                    }

                    match parser::parse_transaction(&raw, &signature, &self.config.program_ids) {
                        Some(tx) => {
                            if self.tx_sender.send(tx).await.is_err() {
                                error!("mpsc channel closed, stopping ingest");
                            }
                        }
                        None => {
                            warn!(signature = %signature, "Failed to parse transaction details");
                        }
                    }
                    break;
                }
                Err(e) => {
                    warn!(signature = %signature, "Failed to fetch transaction: {e}");
                    break;
                }
            }
        }
    }

    async fn fetch_transaction(
        &self,
        signature: &str,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Ok(crate::rpc::fetch_transaction(&self.config.rpc_url, signature).await?)
    }
}
