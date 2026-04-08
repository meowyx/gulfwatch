use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single instruction inside a transaction, classified by the parser.
/// Detections pattern-match on this instead of re-parsing raw bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstructionKind {
    SetAuthority,
    Upgrade,
    TokenTransfer { amount: u64 },
    TokenTransferChecked { amount: u64, decimals: u8 },
    Other { name: String },
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedInstruction {
    pub program_id: String,
    pub kind: InstructionKind,
    pub accounts: Vec<String>,
}

impl ParsedInstruction {
    pub fn display_name(&self) -> Option<&str> {
        match &self.kind {
            InstructionKind::SetAuthority => Some("setAuthority"),
            InstructionKind::Upgrade => Some("upgrade"),
            InstructionKind::TokenTransfer { .. } => Some("transfer"),
            InstructionKind::TokenTransferChecked { .. } => Some("transferChecked"),
            InstructionKind::Other { name } => Some(name.as_str()),
            InstructionKind::Unknown => None,
        }
    }

    // Higher = more interesting. Security-relevant instructions outrank
    // routine ones so a tx containing both a swap and a SetAuthority
    // reports SetAuthority as its headline instruction_type.
    pub fn headline_priority(&self) -> u8 {
        match &self.kind {
            InstructionKind::SetAuthority => 100,
            InstructionKind::Upgrade => 99,
            InstructionKind::TokenTransferChecked { .. } => 50,
            InstructionKind::TokenTransfer { .. } => 49,
            InstructionKind::Other { .. } => 10,
            InstructionKind::Unknown => 0,
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
        }
    }

    #[test]
    fn headline_picks_set_authority_over_swap() {
        let instructions = vec![
            ix("raydium", InstructionKind::Other { name: "swap".to_string() }),
            ix("token", InstructionKind::SetAuthority),
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
            ix("token", InstructionKind::TokenTransferChecked { amount: 100, decimals: 6 }),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("transferChecked".to_string())
        );
    }

    #[test]
    fn headline_falls_back_to_other_then_none_for_unknown_only() {
        let instructions = vec![ix("x", InstructionKind::Unknown)];
        assert_eq!(Transaction::derive_instruction_type(&instructions), None);

        let instructions = vec![
            ix("x", InstructionKind::Unknown),
            ix("raydium", InstructionKind::Other { name: "swap".to_string() }),
        ];
        assert_eq!(
            Transaction::derive_instruction_type(&instructions),
            Some("swap".to_string())
        );
    }
}
