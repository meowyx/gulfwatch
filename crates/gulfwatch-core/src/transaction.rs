use chrono::{DateTime, Utc};
use gulfwatch_classification::{ClassificationDebugTrace, TransactionClassification};
use serde::{Deserialize, Serialize};

use crate::balance_diff::BalanceDiff;
use crate::cu_attribution::CuProfile;
use crate::tx_error::TransactionError;

/// A single instruction inside a transaction, classified by the parser.
/// Detections pattern-match on this instead of re-parsing raw bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstructionKind {
    SetAuthority { authority_type: u8 },
    Upgrade,
    SystemTransfer { lamports: u64 },
    TokenTransfer { amount: u64 },
    TokenTransferChecked { amount: u64, decimals: u8 },
    StakeDelegate,
    StakeWithdraw,
    InitializeTransferHook,
    UpdateTransferHook,
    SetTransferFee,
    InitializePermanentDelegate,
    InitializeDefaultAccountState { frozen: bool },
    UpdateDefaultAccountState { frozen: bool },
    Other { name: String },
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedInstruction {
    pub program_id: String,
    pub kind: InstructionKind,
    pub accounts: Vec<String>,
    // Transient: parser stashes the first 8 bytes of instruction data here,
    // worker consumes it to resolve `anchor_name`. Not serialized.
    #[serde(skip)]
    pub discriminator: Option<[u8; 8]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_name: Option<String>,
}

impl ParsedInstruction {
    pub fn display_name(&self) -> Option<&str> {
        if let Some(name) = self.anchor_name.as_deref() {
            return Some(name);
        }
        match &self.kind {
            InstructionKind::SetAuthority { .. } => Some("setAuthority"),
            InstructionKind::Upgrade => Some("upgrade"),
            InstructionKind::SystemTransfer { .. } => Some("systemTransfer"),
            InstructionKind::TokenTransfer { .. } => Some("transfer"),
            InstructionKind::TokenTransferChecked { .. } => Some("transferChecked"),
            InstructionKind::StakeDelegate => Some("stakeDelegate"),
            InstructionKind::StakeWithdraw => Some("stakeWithdraw"),
            InstructionKind::InitializeTransferHook => Some("initializeTransferHook"),
            InstructionKind::UpdateTransferHook => Some("updateTransferHook"),
            InstructionKind::SetTransferFee => Some("setTransferFee"),
            InstructionKind::InitializePermanentDelegate => Some("initializePermanentDelegate"),
            InstructionKind::InitializeDefaultAccountState { .. } => {
                Some("initializeDefaultAccountState")
            }
            InstructionKind::UpdateDefaultAccountState { .. } => Some("updateDefaultAccountState"),
            InstructionKind::Other { name } => Some(name.as_str()),
            InstructionKind::Unknown => None,
        }
    }

    // Higher = more interesting. Security-relevant kinds outrank routine
    // ones so SetAuthority beats a swap for the tx's headline. Anchor-resolved
    // names get boosted over routine transfers so a Jupiter `route` wins
    // over its own inner Token `transferChecked` CPIs.
    pub fn headline_priority(&self) -> u8 {
        let kind_priority = match &self.kind {
            InstructionKind::SetAuthority { .. } => 100,
            InstructionKind::Upgrade => 99,
            InstructionKind::InitializePermanentDelegate => 98,
            InstructionKind::InitializeDefaultAccountState { .. } => 97,
            InstructionKind::UpdateDefaultAccountState { .. } => 96,
            InstructionKind::InitializeTransferHook => 95,
            InstructionKind::UpdateTransferHook => 94,
            InstructionKind::SetTransferFee => 93,
            InstructionKind::StakeWithdraw => 80,
            InstructionKind::StakeDelegate => 79,
            InstructionKind::TokenTransferChecked { .. } => 50,
            InstructionKind::TokenTransfer { .. } => 49,
            InstructionKind::SystemTransfer { .. } => 48,
            InstructionKind::Other { .. } => 10,
            InstructionKind::Unknown => 0,
        };
        if self.anchor_name.is_some() {
            kind_priority.max(60)
        } else {
            kind_priority
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub signature: String,
    pub program_id: String,
    pub block_slot: u64,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub instruction_type: Option<String>,
    pub accounts: Vec<String>,
    pub fee_lamports: u64,
    pub compute_units: u64,
    #[serde(default)]
    pub instructions: Vec<ParsedInstruction>,
    #[serde(default)]
    pub cu_profile: Option<CuProfile>,
    #[serde(default)]
    pub classification: Option<TransactionClassification>,
    #[serde(default)]
    pub classification_debug: Option<ClassificationDebugTrace>,
    // Raw `meta.logMessages` from getTransaction. Kept verbatim for the deep-dive
    // Logs tab; CU profiling consumes the same source upstream.
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub balance_diff: Option<BalanceDiff>,
    #[serde(default)]
    pub tx_error: Option<TransactionError>,
}

impl Transaction {
    pub fn derive_instruction_type(instructions: &[ParsedInstruction]) -> Option<String> {
        instructions
            .iter()
            .max_by_key(|i| i.headline_priority())
            .and_then(|i| i.display_name().map(|s| s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(program: &str, kind: InstructionKind) -> ParsedInstruction {
        ParsedInstruction {
            program_id: program.to_string(),
            kind,
            accounts: vec![],
            discriminator: None,
            anchor_name: None,
        }
    }

    #[test]
    fn headline_picks_set_authority_over_swap() {
        let instructions = vec![
            ix(
                "raydium",
                InstructionKind::Other {
                    name: "swap".to_string(),
                },
            ),
            ix("token", InstructionKind::SetAuthority { authority_type: 0 }),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("setAuthority".to_string())
        );
    }

    #[test]
    fn headline_picks_upgrade_over_transfer() {
        let instructions = vec![
            ix("token", InstructionKind::TokenTransfer { amount: 100 }),
            ix("loader", InstructionKind::Upgrade),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("upgrade".to_string())
        );
    }

    #[test]
    fn headline_picks_transfer_checked_over_transfer() {
        let instructions = vec![
            ix("token", InstructionKind::TokenTransfer { amount: 100 }),
            ix(
                "token",
                InstructionKind::TokenTransferChecked {
                    amount: 100,
                    decimals: 6,
                },
            ),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("transferChecked".to_string())
        );
    }

    #[test]
    fn anchor_name_beats_routine_transfer_for_headline() {
        let jupiter_route = ParsedInstruction {
            program_id: "Jup".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: Some([0; 8]),
            anchor_name: Some("route".to_string()),
        };
        let inner_transfer = ix(
            "token",
            InstructionKind::TokenTransferChecked {
                amount: 100,
                decimals: 6,
            },
        );
        assert_eq!(
            Transaction::derive_instruction_type(&[jupiter_route, inner_transfer]),
            Some("route".to_string()),
        );
    }

    #[test]
    fn anchor_name_does_not_override_security_critical_kind() {
        let routine_anchor = ParsedInstruction {
            program_id: "Jup".to_string(),
            kind: InstructionKind::Unknown,
            accounts: vec![],
            discriminator: Some([0; 8]),
            anchor_name: Some("route".to_string()),
        };
        let set_authority = ix("token", InstructionKind::SetAuthority { authority_type: 0 });
        assert_eq!(
            Transaction::derive_instruction_type(&[routine_anchor, set_authority]),
            Some("setAuthority".to_string()),
        );
    }

    #[test]
    fn display_name_prefers_anchor_over_kind() {
        let mut ix_with_anchor = ix(
            "token",
            InstructionKind::TokenTransfer { amount: 100 },
        );
        ix_with_anchor.anchor_name = Some("customSwap".to_string());
        assert_eq!(ix_with_anchor.display_name(), Some("customSwap"));
    }

    #[test]
    fn headline_falls_back_to_other_then_none_for_unknown_only() {
        let instructions = vec![ix("x", InstructionKind::Unknown)];
        assert_eq!(Transaction::derive_instruction_type(&instructions), None);

        let instructions = vec![
            ix("x", InstructionKind::Unknown),
            ix(
                "raydium",
                InstructionKind::Other {
                    name: "swap".to_string(),
                },
            ),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("swap".to_string())
        );
    }
}
