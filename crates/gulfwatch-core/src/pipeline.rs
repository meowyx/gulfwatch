use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gulfwatch_classification::{
    ClassificationContext, ClassificationService, InstructionInput, InstructionInputKind,
};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use crate::alert::{AlertEvent, AlertRule};
use crate::detections::Detection;
use crate::idl::{IdlDocument, IdlRegistryEntry, IdlStatus};
use crate::rolling_window::RollingWindow;
use crate::transaction::{InstructionKind, Transaction};

#[derive(Clone)]
pub struct AppState {
    pub windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
    pub monitored_programs: Arc<RwLock<Vec<String>>>,
    pub tx_broadcast: broadcast::Sender<Transaction>,
    pub alert_broadcast: broadcast::Sender<AlertEvent>,
    pub alert_rules: Arc<RwLock<Vec<AlertRule>>>,
    pub recent_alerts: Arc<RwLock<VecDeque<AlertEvent>>>,
    pub idls: Arc<RwLock<HashMap<String, IdlRegistryEntry>>>,
    pub idl_status: Arc<RwLock<HashMap<String, IdlStatus>>>,
    pub idl_failures: Arc<RwLock<HashMap<String, String>>>,
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
            recent_alerts: Arc::new(RwLock::new(VecDeque::new())),
            idls: Arc::new(RwLock::new(HashMap::new())),
            idl_status: Arc::new(RwLock::new(HashMap::new())),
            idl_failures: Arc::new(RwLock::new(HashMap::new())),
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

    pub async fn upsert_idl(&self, program_id: &str, idl: IdlDocument) {
        let entry = IdlRegistryEntry::from_idl(idl);
        {
            let mut idls = self.idls.write().await;
            idls.insert(program_id.to_string(), entry);
        }
        {
            let mut failures = self.idl_failures.write().await;
            failures.remove(program_id);
        }
        self.set_idl_status(program_id, IdlStatus::Loaded).await;
    }

    pub async fn set_idl_status(&self, program_id: &str, status: IdlStatus) {
        let mut statuses = self.idl_status.write().await;
        statuses.insert(program_id.to_string(), status);
    }

    pub async fn get_idl_status(&self, program_id: &str) -> Option<IdlStatus> {
        let statuses = self.idl_status.read().await;
        statuses.get(program_id).copied()
    }

    pub async fn set_idl_failure(&self, program_id: &str, reason: impl Into<String>) {
        {
            let mut failures = self.idl_failures.write().await;
            failures.insert(program_id.to_string(), reason.into());
        }
        self.set_idl_status(program_id, IdlStatus::Unavailable).await;
    }

    pub async fn get_idl_failure(&self, program_id: &str) -> Option<String> {
        let failures = self.idl_failures.read().await;
        failures.get(program_id).cloned()
    }

    pub async fn get_idl(&self, program_id: &str) -> Option<IdlDocument> {
        let idls = self.idls.read().await;
        idls.get(program_id).map(|entry| entry.idl.clone())
    }

    pub async fn idl_entry(&self, program_id: &str) -> Option<IdlRegistryEntry> {
        let idls = self.idls.read().await;
        idls.get(program_id).cloned()
    }

    pub async fn remove_idl(&self, program_id: &str) -> bool {
        let mut idls = self.idls.write().await;
        idls.remove(program_id).is_some()
    }
}

pub async fn run_alert_recorder(state: AppState, capacity: usize) {
    let mut rx = state.alert_broadcast.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let mut buf = state.recent_alerts.write().await;
                buf.push_back(event);
                while buf.len() > capacity {
                    buf.pop_front();
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub struct WorkerHandle {
    pub windows: Arc<RwLock<HashMap<String, RollingWindow>>>,
    pub monitored_programs: Arc<RwLock<Vec<String>>>,
    pub tx_broadcast: broadcast::Sender<Transaction>,
    pub alert_broadcast: broadcast::Sender<AlertEvent>,
    pub idls: Arc<RwLock<HashMap<String, IdlRegistryEntry>>>,
}

impl From<&AppState> for WorkerHandle {
    fn from(state: &AppState) -> Self {
        Self {
            windows: Arc::clone(&state.windows),
            monitored_programs: Arc::clone(&state.monitored_programs),
            tx_broadcast: state.tx_broadcast.clone(),
            alert_broadcast: state.alert_broadcast.clone(),
            idls: Arc::clone(&state.idls),
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

        {
            let idls = handle.idls.read().await;

            if !idls.is_empty() {
                for ix in tx.instructions.iter_mut() {
                    if ix.anchor_name.is_some() {
                        continue;
                    }
                    let Some(disc) = ix.discriminator else {
                        continue;
                    };
                    if let Some(entry) = idls.get(&ix.program_id) {
                        if let Some(name) = entry.instruction_name_for(&disc) {
                            ix.anchor_name = Some(name.to_string());
                        }
                    }
                }

                // Resolve against the failing instruction's program_id, not the
                // outer tx program_id — custom errors bubble up from CPI'd
                // programs (a Token error on a Jupiter swap carries the Token id).
                if let Some(err) = tx.tx_error.as_mut() {
                    if err.anchor_error_name.is_none() {
                        if let Some(code) = err.custom_code {
                            let failing_program = err
                                .instruction_index
                                .and_then(|idx| tx.instructions.get(idx))
                                .map(|ix| ix.program_id.as_str());
                            if let Some(program_id) = failing_program {
                                if let Some(entry) = idls.get(program_id) {
                                    if let Some(idl_err) = entry.errors_by_code.get(&code) {
                                        err.anchor_error_name = Some(idl_err.name.clone());
                                        err.anchor_error_msg = idl_err.msg.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // IDL names stay authoritative; this only fills instructions that
            // had no IDL hit. Matches by program_id in execution order, one
            // log-derived name consumed per instruction.
            if let Some(cu) = tx.cu_profile.as_ref() {
                let mut names_by_program: HashMap<&str, VecDeque<&str>> = HashMap::new();
                for inv in &cu.invocations {
                    if let Some(name) = inv.instruction_name.as_deref() {
                        names_by_program
                            .entry(inv.program_id.as_str())
                            .or_default()
                            .push_back(name);
                    }
                }
                for ix in tx.instructions.iter_mut() {
                    if ix.anchor_name.is_some() {
                        continue;
                    }
                    if let Some(queue) = names_by_program.get_mut(ix.program_id.as_str()) {
                        if let Some(name) = queue.pop_front() {
                            ix.anchor_name = Some(name.to_string());
                        }
                    }
                }
            }
        }
        // Re-derive after anchor_name fills in so the headline reflects the
        // resolved outer instruction instead of an inner transfer CPI.
        tx.instruction_type = Transaction::derive_instruction_type(&tx.instructions);

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

    #[tokio::test]
    async fn worker_resolves_anchor_name_for_matching_discriminator() {
        use crate::idl::{derive_instruction_discriminator, parse_idl_json};
        use crate::transaction::{InstructionKind, ParsedInstruction};

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("JUP6LkbZ".to_string()).await;

        let idl = parse_idl_json(
            br#"{"version":"0.1.0","name":"jupiter","instructions":[{"name":"route"}]}"#,
        )
        .unwrap();
        state.upsert_idl("JUP6LkbZ", idl).await;

        let route_disc = derive_instruction_discriminator("route");
        let mut tx = make_tx("JUP6LkbZ", true);
        tx.instructions = vec![
            ParsedInstruction {
                program_id: "JUP6LkbZ".to_string(),
                kind: InstructionKind::Unknown,
                accounts: vec![],
                discriminator: Some(route_disc),
                data: vec![],
                anchor_name: None,
            },
            ParsedInstruction {
                program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
                kind: InstructionKind::TokenTransferChecked {
                    amount: 100,
                    decimals: 6,
                },
                accounts: vec![],
                discriminator: None,
                data: vec![],
                anchor_name: None,
            },
        ];

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received.instructions[0].anchor_name.as_deref(), Some("route"));
        assert_eq!(received.instructions[1].anchor_name, None);
        assert_eq!(received.instruction_type.as_deref(), Some("route"));

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn upsert_idl_flips_status_to_loaded() {
        use crate::idl::{parse_idl_json, IdlStatus};

        let (state, _rx) = AppState::new(100, 10);
        let idl = parse_idl_json(br#"{"name":"prog"}"#).unwrap();
        assert_eq!(state.get_idl_status("prog").await, None);
        state.upsert_idl("prog", idl).await;
        assert_eq!(state.get_idl_status("prog").await, Some(IdlStatus::Loaded));
    }

    #[tokio::test]
    async fn set_idl_failure_marks_unavailable_and_records_reason() {
        use crate::idl::{parse_idl_json, IdlStatus};

        let (state, _rx) = AppState::new(100, 10);
        state
            .set_idl_failure("prog", "missing field `name`")
            .await;
        assert_eq!(
            state.get_idl_status("prog").await,
            Some(IdlStatus::Unavailable)
        );
        assert_eq!(
            state.get_idl_failure("prog").await.as_deref(),
            Some("missing field `name`")
        );

        // A successful upsert must clear the stale failure reason.
        let idl = parse_idl_json(br#"{"name":"prog"}"#).unwrap();
        state.upsert_idl("prog", idl).await;
        assert_eq!(state.get_idl_failure("prog").await, None);
        assert_eq!(state.get_idl_status("prog").await, Some(IdlStatus::Loaded));
    }

    #[tokio::test]
    async fn set_idl_status_exposes_loading_and_unavailable_transitions() {
        use crate::idl::IdlStatus;

        let (state, _rx) = AppState::new(100, 10);
        state.set_idl_status("prog", IdlStatus::Loading).await;
        assert_eq!(state.get_idl_status("prog").await, Some(IdlStatus::Loading));
        state
            .set_idl_status("prog", IdlStatus::Unavailable)
            .await;
        assert_eq!(
            state.get_idl_status("prog").await,
            Some(IdlStatus::Unavailable)
        );
    }

    #[tokio::test]
    async fn worker_resolves_anchor_error_name_from_failing_instructions_program() {
        use crate::idl::parse_idl_json;
        use crate::transaction::{InstructionKind, ParsedInstruction};
        use crate::tx_error::TransactionError;

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("JUP6LkbZ".to_string()).await;

        // IDL is registered on the inner program (the one that emits the error),
        // not on the outer tx program_id — this exercises the CPI resolution path.
        let inner_idl = parse_idl_json(
            br#"{
                "name":"jupiter_inner",
                "errors":[{"code":6000,"name":"SlippageToleranceExceeded","msg":"The slippage tolerance was exceeded"}]
            }"#,
        )
        .unwrap();
        state.upsert_idl("INNER_PROGRAM", inner_idl).await;

        let mut tx = make_tx("JUP6LkbZ", false);
        tx.instructions = vec![
            ParsedInstruction {
                program_id: "JUP6LkbZ".to_string(),
                kind: InstructionKind::Unknown,
                accounts: vec![],
                discriminator: None,
                data: vec![],
                anchor_name: None,
            },
            ParsedInstruction {
                program_id: "INNER_PROGRAM".to_string(),
                kind: InstructionKind::Unknown,
                accounts: vec![],
                discriminator: None,
                data: vec![],
                anchor_name: None,
            },
        ];
        tx.tx_error = Some(TransactionError {
            instruction_index: Some(1),
            kind: "Custom".to_string(),
            custom_code: Some(6000),
            raw: r#"{"InstructionError":[1,{"Custom":6000}]}"#.to_string(),
            anchor_error_name: None,
            anchor_error_msg: None,
        });

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let err = received.tx_error.unwrap();
        assert_eq!(err.anchor_error_name.as_deref(), Some("SlippageToleranceExceeded"));
        assert_eq!(
            err.anchor_error_msg.as_deref(),
            Some("The slippage tolerance was exceeded"),
        );

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn worker_leaves_anchor_error_none_when_code_not_in_idl() {
        use crate::idl::parse_idl_json;
        use crate::transaction::{InstructionKind, ParsedInstruction};
        use crate::tx_error::TransactionError;

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;

        let idl = parse_idl_json(
            br#"{"name":"prog","errors":[{"code":6000,"name":"KnownError"}]}"#,
        )
        .unwrap();
        state.upsert_idl("prog", idl).await;

        let mut tx = make_tx("prog", false);
        tx.instructions = vec![ParsedInstruction {
            program_id: "prog".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: None,
            data: vec![],
            anchor_name: None,
        }];
        tx.tx_error = Some(TransactionError {
            instruction_index: Some(0),
            kind: "Custom".to_string(),
            custom_code: Some(9999), // not in IDL
            raw: "{}".to_string(),
            anchor_error_name: None,
            anchor_error_msg: None,
        });

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        let err = received.tx_error.unwrap();
        assert_eq!(err.anchor_error_name, None);
        assert_eq!(err.anchor_error_msg, None);

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn worker_leaves_anchor_name_none_when_no_idl_registered() {
        use crate::idl::derive_instruction_discriminator;
        use crate::transaction::{InstructionKind, ParsedInstruction};

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("prog".to_string()).await;

        // No IDL registered. Tx carries a valid-looking discriminator anyway.
        let mut tx = make_tx("prog", true);
        tx.instructions = vec![ParsedInstruction {
            program_id: "prog".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: Some(derive_instruction_discriminator("swap")),
            data: vec![],
            anchor_name: None,
        }];

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received.instructions[0].anchor_name, None);

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn log_fallback_names_instructions_when_no_idl_registered() {
        use crate::cu_attribution::parse_logs;
        use crate::transaction::{InstructionKind, ParsedInstruction};

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("RAYDIUM".to_string()).await;

        let logs: Vec<String> = [
            "Program RAYDIUM invoke [1]",
            "Program log: Instruction: Swap",
            "Program RAYDIUM consumed 5000 of 200000 compute units",
            "Program RAYDIUM success",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let profile = parse_logs(&logs, 5000);

        let mut tx = make_tx("RAYDIUM", true);
        tx.instructions = vec![ParsedInstruction {
            program_id: "RAYDIUM".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: None,
            data: vec![],
            anchor_name: None,
        }];
        tx.logs = logs;
        tx.cu_profile = Some(profile);

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received.instructions[0].anchor_name.as_deref(), Some("Swap"));

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn idl_name_wins_over_log_name_when_both_available() {
        use crate::cu_attribution::parse_logs;
        use crate::idl::{derive_instruction_discriminator, parse_idl_json};
        use crate::transaction::{InstructionKind, ParsedInstruction};

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("JUP".to_string()).await;

        // IDL says "route" (lowercase). Log says "Route" (capitalised).
        // If IDL precedence is broken, the test would see "Route".
        let idl = parse_idl_json(br#"{"name":"jupiter","instructions":[{"name":"route"}]}"#)
            .unwrap();
        state.upsert_idl("JUP", idl).await;

        let logs: Vec<String> = [
            "Program JUP invoke [1]",
            "Program log: Instruction: Route",
            "Program JUP success",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let profile = parse_logs(&logs, 5000);

        let mut tx = make_tx("JUP", true);
        tx.instructions = vec![ParsedInstruction {
            program_id: "JUP".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: Some(derive_instruction_discriminator("route")),
            data: vec![],
            anchor_name: None,
        }];
        tx.logs = logs;
        tx.cu_profile = Some(profile);

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received.instructions[0].anchor_name.as_deref(), Some("route"));

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }

    #[tokio::test]
    async fn log_fallback_only_fills_gaps_leaving_idl_resolved_alone() {
        use crate::cu_attribution::parse_logs;
        use crate::idl::{derive_instruction_discriminator, parse_idl_json};
        use crate::transaction::{InstructionKind, ParsedInstruction};

        let (state, ingest_rx) = AppState::new(100, 10);
        state.add_program("JUP".to_string()).await;

        let idl = parse_idl_json(br#"{"name":"jupiter","instructions":[{"name":"route"}]}"#)
            .unwrap();
        state.upsert_idl("JUP", idl).await;

        // Two instructions: Jupiter (IDL-resolvable) + native Raydium (no IDL).
        let logs: Vec<String> = [
            "Program JUP invoke [1]",
            "Program log: Instruction: Route",
            "Program JUP success",
            "Program RAYDIUM invoke [1]",
            "Program log: Instruction: Swap",
            "Program RAYDIUM success",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let profile = parse_logs(&logs, 10_000);

        let mut tx = make_tx("JUP", true);
        tx.instructions = vec![
            ParsedInstruction {
                program_id: "JUP".to_string(),
                kind: InstructionKind::Unknown,
                accounts: vec![],
                discriminator: Some(derive_instruction_discriminator("route")),
                data: vec![],
                anchor_name: None,
            },
            ParsedInstruction {
                program_id: "RAYDIUM".to_string(),
                kind: InstructionKind::Unknown,
                accounts: vec![],
                discriminator: None,
                data: vec![],
                anchor_name: None,
            },
        ];
        tx.logs = logs;
        tx.cu_profile = Some(profile);

        let mut rx = state.tx_broadcast.subscribe();
        let sender = state.ingest_tx.clone();
        let handle = WorkerHandle::from(&state);
        let worker = tokio::spawn(run_processing_worker(handle, ingest_rx, vec![]));

        sender.send(tx).await.unwrap();
        let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        // IDL wins for Jupiter (stays lowercase "route"), log fills Raydium.
        assert_eq!(received.instructions[0].anchor_name.as_deref(), Some("route"));
        assert_eq!(received.instructions[1].anchor_name.as_deref(), Some("Swap"));

        drop(sender);
        drop(state);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), worker).await;
    }
}
