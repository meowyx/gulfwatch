use crate::alert::AlertEvent;
use crate::detections::Detection;
use crate::transaction::{InstructionKind, Transaction};

// Values from spl-token-2022 interface/src/instruction.rs AuthorityType.
const AUTHORITY_TYPE_TRANSFER_FEE_CONFIG: u8 = 4;
const AUTHORITY_TYPE_PERMANENT_DELEGATE: u8 = 8;
const AUTHORITY_TYPE_TRANSFER_HOOK_PROGRAM_ID: u8 = 10;

fn fire(rule: &str, label: &str, tx: &Transaction) -> AlertEvent {
    AlertEvent {
        rule_id: format!("{rule}:{}", tx.signature),
        rule_name: label.to_string(),
        program_id: tx.program_id.clone(),
        metric: rule.to_string(),
        value: 1.0,
        threshold: 0.0,
        fired_at: tx.timestamp,
    }
}

pub struct TransferHookUpgradeDetection;

impl Detection for TransferHookUpgradeDetection {
    fn name(&self) -> &str {
        "transfer_hook_upgrade"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let matched = tx.instructions.iter().any(|ix| {
            matches!(
                ix.kind,
                InstructionKind::InitializeTransferHook
                    | InstructionKind::UpdateTransferHook
                    | InstructionKind::SetAuthority {
                        authority_type: AUTHORITY_TYPE_TRANSFER_HOOK_PROGRAM_ID
                    }
            )
        });
        matched.then(|| fire("transfer_hook_upgrade", "Transfer hook upgraded", tx))
    }
}

pub struct PermanentDelegateDetection;

impl Detection for PermanentDelegateDetection {
    fn name(&self) -> &str {
        "permanent_delegate"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let matched = tx.instructions.iter().any(|ix| {
            matches!(
                ix.kind,
                InstructionKind::InitializePermanentDelegate
                    | InstructionKind::SetAuthority {
                        authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE
                    }
            )
        });
        matched.then(|| fire("permanent_delegate", "Permanent delegate set or changed", tx))
    }
}

pub struct TransferFeeAuthorityChangeDetection;

impl Detection for TransferFeeAuthorityChangeDetection {
    fn name(&self) -> &str {
        "transfer_fee_authority_change"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let matched = tx.instructions.iter().any(|ix| {
            matches!(
                ix.kind,
                InstructionKind::SetAuthority {
                    authority_type: AUTHORITY_TYPE_TRANSFER_FEE_CONFIG
                }
            )
        });
        matched.then(|| {
            fire(
                "transfer_fee_authority_change",
                "Transfer fee authority changed",
                tx,
            )
        })
    }
}

pub struct DefaultAccountStateFrozenDetection;

impl Detection for DefaultAccountStateFrozenDetection {
    fn name(&self) -> &str {
        "default_account_state_frozen"
    }

    fn evaluate(&mut self, tx: &Transaction) -> Option<AlertEvent> {
        let matched = tx.instructions.iter().any(|ix| {
            matches!(
                ix.kind,
                InstructionKind::InitializeDefaultAccountState { frozen: true }
                    | InstructionKind::UpdateDefaultAccountState { frozen: true }
            )
        });
        matched.then(|| {
            fire(
                "default_account_state_frozen",
                "Mint default account state flipped to frozen",
                tx,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::ParsedInstruction;
    use chrono::Utc;

    fn ix(kind: InstructionKind) -> ParsedInstruction {
        ParsedInstruction {
            program_id: "token_2022".to_string(),
            kind,
            accounts: vec![],
            discriminator: None,
            data: vec![],
            anchor_name: None,
        }
    }

    fn tx_with(instructions: Vec<ParsedInstruction>) -> Transaction {
        Transaction {
            signature: "test_sig".to_string(),
            program_id: "monitored_mint".to_string(),
            block_slot: 42,
            timestamp: Utc::now(),
            success: true,
            instruction_type: Transaction::derive_instruction_type(&instructions),
            accounts: vec![],
            fee_lamports: 5_000,
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

    // ---- TransferHookUpgradeDetection ----

    #[test]
    fn transfer_hook_fires_on_initialize() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![ix(InstructionKind::InitializeTransferHook)]);
        let ev = det.evaluate(&tx).expect("should fire");
        assert_eq!(ev.metric, "transfer_hook_upgrade");
        assert_eq!(ev.rule_name, "Transfer hook upgraded");
        assert!(ev.rule_id.contains("test_sig"));
    }

    #[test]
    fn transfer_hook_fires_on_update() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![ix(InstructionKind::UpdateTransferHook)]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn transfer_hook_fires_on_set_authority_hook_program() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority {
            authority_type: AUTHORITY_TYPE_TRANSFER_HOOK_PROGRAM_ID,
        })]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn transfer_hook_ignores_unrelated_set_authority() {
        let mut det = TransferHookUpgradeDetection;
        // AuthorityType 0 = MintTokens — unrelated to hook program changes
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority { authority_type: 0 })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn transfer_hook_ignores_plain_transfer() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![ix(InstructionKind::TokenTransfer { amount: 1_000 })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn transfer_hook_fires_when_mixed_with_transfers() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![
            ix(InstructionKind::TokenTransfer { amount: 100 }),
            ix(InstructionKind::UpdateTransferHook),
        ]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn transfer_hook_ignores_empty_tx() {
        let mut det = TransferHookUpgradeDetection;
        let tx = tx_with(vec![]);
        assert!(det.evaluate(&tx).is_none());
    }

    // ---- PermanentDelegateDetection ----

    #[test]
    fn permanent_delegate_fires_on_initialize() {
        let mut det = PermanentDelegateDetection;
        let tx = tx_with(vec![ix(InstructionKind::InitializePermanentDelegate)]);
        let ev = det.evaluate(&tx).expect("should fire");
        assert_eq!(ev.metric, "permanent_delegate");
        assert_eq!(ev.rule_name, "Permanent delegate set or changed");
    }

    #[test]
    fn permanent_delegate_fires_on_set_authority_permanent_delegate() {
        let mut det = PermanentDelegateDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority {
            authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE,
        })]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn permanent_delegate_ignores_mint_authority_change() {
        let mut det = PermanentDelegateDetection;
        // AuthorityType 0 = MintTokens — not the permanent delegate
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority { authority_type: 0 })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn permanent_delegate_ignores_hook_authority_change() {
        let mut det = PermanentDelegateDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority {
            authority_type: AUTHORITY_TYPE_TRANSFER_HOOK_PROGRAM_ID,
        })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn permanent_delegate_fires_when_mixed_with_transfer() {
        let mut det = PermanentDelegateDetection;
        let tx = tx_with(vec![
            ix(InstructionKind::TokenTransfer { amount: 1_000 }),
            ix(InstructionKind::InitializePermanentDelegate),
        ]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn permanent_delegate_ignores_empty_tx() {
        let mut det = PermanentDelegateDetection;
        let tx = tx_with(vec![]);
        assert!(det.evaluate(&tx).is_none());
    }

    // ---- TransferFeeAuthorityChangeDetection ----

    #[test]
    fn fee_authority_fires_on_set_authority_transfer_fee_config() {
        let mut det = TransferFeeAuthorityChangeDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority {
            authority_type: AUTHORITY_TYPE_TRANSFER_FEE_CONFIG,
        })]);
        let ev = det.evaluate(&tx).expect("should fire");
        assert_eq!(ev.metric, "transfer_fee_authority_change");
    }

    #[test]
    fn fee_authority_ignores_set_transfer_fee_instruction() {
        // SetTransferFee is a fee-rate update, not an authority change.
        let mut det = TransferFeeAuthorityChangeDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetTransferFee)]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn fee_authority_ignores_unrelated_set_authority() {
        let mut det = TransferFeeAuthorityChangeDetection;
        let tx = tx_with(vec![ix(InstructionKind::SetAuthority {
            authority_type: AUTHORITY_TYPE_PERMANENT_DELEGATE,
        })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn fee_authority_ignores_empty_tx() {
        let mut det = TransferFeeAuthorityChangeDetection;
        let tx = tx_with(vec![]);
        assert!(det.evaluate(&tx).is_none());
    }

    // ---- DefaultAccountStateFrozenDetection ----

    #[test]
    fn default_state_frozen_fires_on_initialize_frozen() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![ix(InstructionKind::InitializeDefaultAccountState {
            frozen: true,
        })]);
        let ev = det.evaluate(&tx).expect("should fire");
        assert_eq!(ev.metric, "default_account_state_frozen");
    }

    #[test]
    fn default_state_frozen_fires_on_update_frozen() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![ix(InstructionKind::UpdateDefaultAccountState {
            frozen: true,
        })]);
        assert!(det.evaluate(&tx).is_some());
    }

    #[test]
    fn default_state_frozen_ignores_initialize_initialized() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![ix(InstructionKind::InitializeDefaultAccountState {
            frozen: false,
        })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn default_state_frozen_ignores_update_initialized() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![ix(InstructionKind::UpdateDefaultAccountState {
            frozen: false,
        })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn default_state_frozen_ignores_unrelated_instruction() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![ix(InstructionKind::TokenTransfer { amount: 100 })]);
        assert!(det.evaluate(&tx).is_none());
    }

    #[test]
    fn default_state_frozen_ignores_empty_tx() {
        let mut det = DefaultAccountStateFrozenDetection;
        let tx = tx_with(vec![]);
        assert!(det.evaluate(&tx).is_none());
    }

    // ---- Cross-rule sanity ----

    #[test]
    fn detection_names_are_stable() {
        assert_eq!(TransferHookUpgradeDetection.name(), "transfer_hook_upgrade");
        assert_eq!(PermanentDelegateDetection.name(), "permanent_delegate");
        assert_eq!(
            TransferFeeAuthorityChangeDetection.name(),
            "transfer_fee_authority_change"
        );
        assert_eq!(
            DefaultAccountStateFrozenDetection.name(),
            "default_account_state_frozen"
        );
    }
}
