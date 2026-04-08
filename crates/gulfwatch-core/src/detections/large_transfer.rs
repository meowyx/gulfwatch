//! Fires when an SPL Token Transfer or TransferChecked moves an amount at
//! or above `threshold_amount` out of an account in `watched_accounts`.
//! Inert when either is unset (empty set or `u64::MAX` sentinel).

use std::collections::HashSet;

use crate::alert::AlertEvent;
use crate::detections::Detection;
use crate::transaction::{InstructionKind, ParsedInstruction, Transaction};

pub struct LargeTransferDetection {
    watched_accounts: HashSet<String>,
    threshold_amount: u64,
}

impl LargeTransferDetection {
    pub fn new(watched_accounts: HashSet<String>, threshold_amount: u64) -> Self {
        Self {
            watched_accounts,
            threshold_amount,
        }
    }

    pub fn is_inert(&self) -> bool {
        self.watched_accounts.is_empty() || self.threshold_amount == u64::MAX
    }

    fn source_of(ix: &ParsedInstruction) -> Option<&String> {
        ix.accounts.first()
    }

    // TransferChecked layout is [source, mint, destination, owner], so
    // destination sits at index 2 — not 1 like a plain Transfer.
    fn destination_of(ix: &ParsedInstruction) -> Option<&String> {
        match ix.kind {
            InstructionKind::TokenTransfer { .. } => ix.accounts.get(1),
            InstructionKind::TokenTransferChecked { .. } => ix.accounts.get(2),
            _ => None,
        }
    }
}

impl Detection for LargeTransferDetection {
    fn name(&self) -> &str {
        "large_transfer"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        if self.is_inert() {
            return None;
        }

        for ix in &tx.instructions {
            let amount = match ix.kind {
                InstructionKind::TokenTransfer { amount } => amount,
                InstructionKind::TokenTransferChecked { amount, .. } => amount,
                _ => continue,
            };

            if amount < self.threshold_amount {
                continue;
            }

            let source = match Self::source_of(ix) {
                Some(s) if self.watched_accounts.contains(s) => s.clone(),
                _ => continue,
            };

            let destination = Self::destination_of(ix)
                .cloned()
                .unwrap_or_else(|| "?".to_string());

            return Some(AlertEvent {
                rule_id: format!("large_transfer:{}:{}", source, tx.signature),
                rule_name: "Large transfer from watched account".to_string(),
                program_id: tx.program_id.clone(),
                metric: format!("{} → {}", source, destination),
                value: amount as f64,
                threshold: self.threshold_amount as f64,
                fired_at: tx.timestamp,
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ix_transfer(source: &str, dest: &str, amount: u64) -> ParsedInstruction {
        ParsedInstruction {
            program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
            kind: InstructionKind::TokenTransfer { amount },
            accounts: vec![source.to_string(), dest.to_string(), "owner".to_string()],
        }
    }

    fn ix_transfer_checked(
        source: &str,
        dest: &str,
        amount: u64,
        decimals: u8,
    ) -> ParsedInstruction {
        ParsedInstruction {
            program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
            kind: InstructionKind::TokenTransferChecked { amount, decimals },
            accounts: vec![
                source.to_string(),
                "mint".to_string(),
                dest.to_string(),
                "owner".to_string(),
            ],
        }
    }

    fn ix_other(name: &str) -> ParsedInstruction {
        ParsedInstruction {
            program_id: "raydium".to_string(),
            kind: InstructionKind::Other {
                name: name.to_string(),
            },
            accounts: vec![],
        }
    }

    fn tx_with(instructions: Vec<ParsedInstruction>) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
            block_slot: 100,
            timestamp: Utc::now(),
            success: true,
            instruction_type: Transaction::derive_instruction_type(&instructions),
            accounts: vec![],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions,
        }
    }

    fn detection_with(watched: &[&str], threshold: u64) -> LargeTransferDetection {
        let set: HashSet<String> = watched.iter().map(|s| s.to_string()).collect();
        LargeTransferDetection::new(set, threshold)
    }

    #[test]
    fn fires_on_transfer_from_watched_above_threshold() {
        let mut det = detection_with(&["VAULT_A"], 1_000);
        let tx = tx_with(vec![ix_transfer("VAULT_A", "ATTACKER", 5_000)]);

        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.value, 5_000.0);
        assert_eq!(event.threshold, 1_000.0);
        assert!(event.metric.contains("VAULT_A"));
        assert!(event.metric.contains("ATTACKER"));
        assert!(event.rule_id.contains("VAULT_A"));
    }

    #[test]
    fn fires_on_transfer_checked_from_watched_above_threshold() {
        let mut det = detection_with(&["VAULT_B"], 100);
        let tx = tx_with(vec![ix_transfer_checked("VAULT_B", "EXIT_WALLET", 500, 6)]);

        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.value, 500.0);
        // Asserts TransferChecked picked destination from index 2, not 1.
        assert!(event.metric.contains("EXIT_WALLET"));
        assert!(!event.metric.contains("mint"));
    }

    #[test]
    fn does_not_fire_below_threshold() {
        let mut det = detection_with(&["VAULT_A"], 10_000);
        let tx = tx_with(vec![ix_transfer("VAULT_A", "DST", 9_999)]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn does_not_fire_from_unwatched_source() {
        let mut det = detection_with(&["VAULT_A"], 100);
        let tx = tx_with(vec![ix_transfer("RANDOM_WALLET", "DST", 1_000_000)]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn does_not_fire_on_non_token_instructions() {
        let mut det = detection_with(&["VAULT_A"], 1);
        let tx = tx_with(vec![ix_other("swap"), ix_other("addLiquidity")]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn boundary_amount_exactly_equal_to_threshold_fires() {
        let mut det = detection_with(&["VAULT_A"], 1_000);
        let tx = tx_with(vec![ix_transfer("VAULT_A", "DST", 1_000)]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn inert_when_watched_accounts_empty() {
        let mut det = LargeTransferDetection::new(HashSet::new(), 100);
        assert!(det.is_inert());
        let tx = tx_with(vec![ix_transfer("VAULT_A", "DST", 9_999_999)]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn inert_when_threshold_is_max() {
        let mut det = detection_with(&["VAULT_A"], u64::MAX);
        assert!(det.is_inert());
        let tx = tx_with(vec![ix_transfer("VAULT_A", "DST", u64::MAX - 1)]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn fires_on_first_matching_instruction_in_a_multi_ix_tx() {
        let mut det = detection_with(&["VAULT_A"], 100);
        let tx = tx_with(vec![
            ix_other("swap"),
            ix_transfer("RANDOM", "X", 99_999_999),
            ix_transfer("VAULT_A", "EXIT", 5_000),
        ]);
        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.value, 5_000.0);
    }

    #[test]
    fn name_is_stable() {
        let det = detection_with(&["x"], 1);
        assert_eq!(det.name(), "large_transfer");
    }
}
