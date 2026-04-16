use std::collections::HashMap;
use std::sync::Arc;

use gulfwatch_classification::{
    ClassificationContext, ClassificationService, InstructionInput, InstructionInputKind,
};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use crate::alert::{AlertEvent, AlertRule};
use crate::detections::Detection;
use crate::rolling_window::RollingWindow;
use crate::transaction::{InstructionKind, Transaction};

#[derive(Clone)]
pub struct AppState {
    pub windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
    pub monitored_programs: Arc<RwLock<Vec<String>>>,
    pub tx_broadcast: broadcast::Sender<Transaction>,
    pub alert_broadcast: broadcast::Sender<AlertEvent>,
    pub alert_rules: Arc<RwLock<Vec<AlertRule>>>,
    pub ingest_tx: mpsc::Sender<Transaction>,
    pub window_minutes: i64,
}

impl AppState {
    pub fn new(channel_capacity: usize, window_minutes: i64) -> (Self, mpsc::Receiver<Transaction>) {
        let (ingest_tx, ingest_rx) = mpsc::channel(channel_capacity);
        let (tx_broadcast, _) = broadcast::channel(channel_capacity);
        let (alert_broadcast, _) = broadcast::channel(channel_capacity);

        let state = Self {
            windows: Arc::new(RwLock::new(HashMap::new())),
            monitored_programs: Arc::new(RwLock::new(Vec::new())),
            tx_broadcast,
            alert_broadcast,
            alert_rules: Arc::new(RwLock::new(Vec::new())),
            ingest_tx,
            window_minutes,
        };

        (state, ingest_rx)
    }

    pub async fn add_program(&self, program_id: String) {
        let mut programs = self.monitored_programs.write().await;
        if !programs.contains(&program_id) {
            programs.push(program_id.clone());
        }

        let mut windows = self.windows.write().await;
        windows
            .entry(program_id)
            .or_insert_with(|| RollingWindow::new(self.window_minutes));
    }

    pub async fn remove_program(&self, program_id: &str) {
        let mut programs = self.monitored_programs.write().await;
        programs.retain(|p| p != program_id);

        let mut windows = self.windows.write().await;
        windows.remove(program_id);
    }
}

pub struct WorkerHandle {
    pub windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
    pub monitored_programs: Arc<RwLock<Vec<String>>>,
    pub tx_broadcast: broadcast::Sender<Transaction>,
    pub alert_broadcast: broadcast::Sender<AlertEvent>,
}

impl From<&AppState> for WorkerHandle {
    fn from(state: &AppState) -> Self {
        Self {
            windows: Arc::clone(&state.windows),
            monitored_programs: Arc::clone(&state.monitored_programs),
            tx_broadcast: state.tx_broadcast.clone(),
            alert_broadcast: state.alert_broadcast.clone(),
        }
    }
}

pub async fn run_processing_worker(
    handle: WorkerHandle,
    mut ingest_rx: mpsc::Receiver<Transaction>,
    mut detections: Vec<Box<dyn Detection>>,
) {
    info!(
        detection_count = detections.len(),
        "Processing worker started"
    );

    let mut dead_letter_count: u64 = 0;
    let classification_service = ClassificationService::new();

    while let Some(mut tx) = ingest_rx.recv().await {
        let program_id = tx.program_id.clone();

        let is_monitored = {
            let programs = handle.monitored_programs.read().await;
            programs.contains(&program_id)
        };

        if !is_monitored {
            dead_letter_count += 1;
            if dead_letter_count % 100 == 1 {
                warn!(
                    program_id = %program_id,
                    dead_letter_count,
                    "Transaction for unmonitored program (dead-lettered)"
                );
            }
            continue;
        }

        let classification_instructions = to_classification_instructions(&tx);
        let classification_context = ClassificationContext {
            instruction_type: tx.instruction_type.as_deref(),
            success: tx.success,
            compute_units: tx.compute_units,
            fee_lamports: tx.fee_lamports,
            accounts: &tx.accounts,
            instructions: &classification_instructions,
        };
        let classification = classification_service.classify(&classification_context);
        tx.classification = Some(classification.classification);
        tx.classification_debug = Some(classification.debug_trace);

        // Worker is single-task, so detections can hold &mut state without locks.
        for detection in detections.iter_mut() {
            if let Some(event) = detection.evaluate(&tx) {
                info!(
                    detection = detection.name(),
                    signature = %tx.signature,
                    program_id = %tx.program_id,
                    "Security detection fired"
                );
                let _ = handle.alert_broadcast.send(event);
            }
        }

        {
            let mut windows = handle.windows.write().await;
            if let Some(window) = windows.get_mut(&program_id) {
                window.push(tx.clone());
            }
        }

        let _ = handle.tx_broadcast.send(tx);
    }

    error!("Processing worker stopped — ingest channel closed");
}

fn to_classification_instructions(tx: &Transaction) -> Vec<InstructionInput> {
    tx.instructions
        .iter()
        .map(|instruction| InstructionInput {
            program_id: instruction.program_id.clone(),
            kind: match &instruction.kind {
                InstructionKind::SetAuthority { .. } => InstructionInputKind::SetAuthority,
                InstructionKind::Upgrade => InstructionInputKind::Upgrade,
                InstructionKind::SystemTransfer { lamports } => {
                    InstructionInputKind::SystemTransfer { lamports: *lamports }
                }
                InstructionKind::TokenTransfer { amount } => {
                    InstructionInputKind::TokenTransfer { amount: *amount }
                }
                InstructionKind::TokenTransferChecked { amount, decimals } => {
                    InstructionInputKind::TokenTransferChecked {
                        amount: *amount,
                        decimals: *decimals,
                    }
                }
                InstructionKind::StakeDelegate => InstructionInputKind::StakeDelegate,
                InstructionKind::StakeWithdraw => InstructionInputKind::StakeWithdraw,
                InstructionKind::InitializeTransferHook => InstructionInputKind::Other {
                    name: "initializeTransferHook".to_string(),
                },
                InstructionKind::UpdateTransferHook => InstructionInputKind::Other {
                    name: "updateTransferHook".to_string(),
                },
                InstructionKind::SetTransferFee => InstructionInputKind::Other {
                    name: "setTransferFee".to_string(),
                },
                InstructionKind::InitializePermanentDelegate => InstructionInputKind::Other {
                    name: "initializePermanentDelegate".to_string(),
                },
                InstructionKind::InitializeDefaultAccountState { .. } => {
                    InstructionInputKind::Other {
                        name: "initializeDefaultAccountState".to_string(),
                    }
                }
                InstructionKind::UpdateDefaultAccountState { .. } => InstructionInputKind::Other {
                    name: "updateDefaultAccountState".to_string(),
                },
                InstructionKind::Other { name } => {
                    InstructionInputKind::Other { name: name.clone() }
                }
                InstructionKind::Unknown => InstructionInputKind::Unknown,
            },
            accounts: instruction.accounts.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_tx(program_id: &str, success: bool) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: program_id.to_string(),
            block_slot: 100,
            timestamp: Utc::now(),
            success,
            instruction_type: Some("swap".to_string()),
            accounts: vec![],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions: vec![],
            cu_profile: None,
            classification: None,
            classification_debug: None,
            logs: vec![],
            balance_diff: None,
            tx_error: None,
        }
    }

    #[tokio::test]
    async fn processing_worker_routes_to_correct_window() {
        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("prog_a".to_string()).await;
        state.add_program("prog_b".to_string()).await;

        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);

        // Spawn the worker (handle does NOT hold an mpsc::Sender)
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        // Send transactions
        sender.send(make_tx("prog_a", true)).await.unwrap();
        sender.send(make_tx("prog_a", false)).await.unwrap();
        sender.send(make_tx("prog_b", true)).await.unwrap();
        sender.send(make_tx("unknown", true)).await.unwrap(); // dead letter

        // Give the worker time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Check windows
        {
            let windows = state.windows.read().await;
            let a_summary = windows.get("prog_a").unwrap().summary("prog_a");
            assert_eq!(a_summary.tx_count, 2);
            assert_eq!(a_summary.error_count, 1);

            let recent_a = windows.get("prog_a").unwrap().recent(1);
            assert_eq!(recent_a.len(), 1);
            assert!(recent_a[0].classification.is_some());
            assert!(recent_a[0].classification_debug.is_some());

            let b_summary = windows.get("prog_b").unwrap().summary("prog_b");
            assert_eq!(b_summary.tx_count, 1);

            assert!(windows.get("unknown").is_none());
        }

        // Drop all senders so the worker exits
        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn broadcast_reaches_subscribers() {
        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;

        let mut rx1 = state.tx_broadcast.subscribe();
        let mut rx2 = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);

        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(make_tx("prog", true)).await.unwrap();

        let received1 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx1.recv(),
        )
        .await
        .unwrap()
        .unwrap();

        let received2 = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx2.recv(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(received1.program_id, "prog");
        assert_eq!(received2.program_id, "prog");

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }
}
