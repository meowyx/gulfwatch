//! Fires on any SPL Token `SetAuthority` or BPF Loader Upgradeable `Upgrade`
//! instruction. Stateless — one alert per matching transaction.

use crate::alert::AlertEvent;
use crate::detections::Detection;
use crate::transaction::{InstructionKind, Transaction};

pub struct AuthorityChangeDetection;

impl Detection for AuthorityChangeDetection {
    fn name(&self) -> &str {
        "authority_change"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let trigger = tx.instructions.iter().find_map(|ix| match ix.kind {
            InstructionKind::SetAuthority { .. } => Some("set_authority"),
            InstructionKind::Upgrade => Some("upgrade"),
            _ => None,
        })?;

        Some(AlertEvent {
            rule_id: format!("authority_change:{}", tx.signature),
            rule_name: "Authority change detected".to_string(),
            program_id: tx.program_id.clone(),
            metric: trigger.to_string(),
            value: 1.0,
            threshold: 0.0,
            fired_at: tx.timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::ParsedInstruction;
    use chrono::Utc;

    fn ix(program: &str, kind: InstructionKind) -> ParsedInstruction {
        ParsedInstruction {
            program_id: program.to_string(),
            kind,
            accounts: vec![],
            discriminator: None,
            anchor_name: None,
        }
    }

    fn tx_with(instructions: Vec<ParsedInstruction>) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: "monitored_prog".to_string(),
            block_slot: 100,
            timestamp: Utc::now(),
            success: true,
            instruction_type: Transaction::derive_instruction_type(&instructions),
            accounts: vec![],
            fee_lamports: 5000,
            compute_units: 200_000,
            instructions,
            cu_profile: None,
            classification: None,
            classification_debug: None,
            logs: vec![],
            balance_diff: None,
            tx_error: None,
        }
    }

    #[test]
    fn fires_on_set_authority() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![ix(
            "token",
            InstructionKind::SetAuthority { authority_type: 0 },
        )]);

        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.metric, "set_authority");
        assert_eq!(event.program_id, "monitored_prog");
        assert_eq!(event.rule_name, "Authority change detected");
        assert!(event.rule_id.contains("test_sig"));
        assert_eq!(event.value, 1.0);
    }

    #[test]
    fn fires_on_upgrade() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![ix("loader", InstructionKind::Upgrade)]);

        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.metric, "upgrade");
        assert_eq!(event.program_id, "monitored_prog");
    }

    #[test]
    fn does_not_fire_on_swap() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![ix(
            "raydium",
            InstructionKind::Other {
                name: "swap".to_string(),
            },
        )]);

        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn does_not_fire_on_token_transfer() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![ix(
            "token",
            InstructionKind::TokenTransfer { amount: 1_000_000 },
        )]);

        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn does_not_fire_on_empty_instructions() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn fires_when_set_authority_mixed_with_swap() {
        let mut det = AuthorityChangeDetection;
        let tx = tx_with(vec![
            ix(
                "raydium",
                InstructionKind::Other {
                    name: "swap".to_string(),
                },
            ),
            ix("token", InstructionKind::SetAuthority { authority_type: 0 }),
        ]);

        let event = det.evaluate(&tx).expect("should fire");
        assert_eq!(event.metric, "set_authority");
    }

    #[test]
    fn name_is_stable() {
        let det = AuthorityChangeDetection;
        assert_eq!(det.name(), "authority_change");
    }
}
