use chrono::{DateTime, TimeZone, Utc};
use gulfwatch_core::{parse_logs, InstructionKind, ParsedInstruction, Transaction};
use serde_json::Value;

use crate::program_ids::{
    BPF_LOADER_UPGRADEABLE, MEMO_V1_PROGRAM, SPL_MEMO_PROGRAM, SPL_TOKEN_PROGRAM,
    STAKE_POOL_PROGRAM, STAKE_PROGRAM, SYSTEM_PROGRAM,
};

pub fn parse_transaction(
    raw: &Value,
    signature: &str,
    target_program: &str,
) -> Option<Transaction> {
    let result = raw.get("result")?;
    if result.is_null() {
        return None;
    }

    let block_slot = result.get("slot")?.as_u64()?;
    let timestamp: DateTime<Utc> = result
        .get("blockTime")
        .and_then(|v| v.as_i64())
        .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
        .unwrap_or_else(Utc::now);

    let meta = result.get("meta")?;
    let transaction = result.get("transaction")?;
    let message = transaction.get("message")?;

    let success = meta.get("err").map_or(true, |e| e.is_null());
    let fee_lamports = meta.get("fee").and_then(|v| v.as_u64()).unwrap_or(0);
    let compute_units = meta
        .get("computeUnitsConsumed")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let log_messages: Vec<String> = meta
        .get("logMessages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let cu_profile = if log_messages.is_empty() {
        None
    } else {
        Some(parse_logs(&log_messages, compute_units))
    };

    let accounts: Vec<String> = message
        .get("accountKeys")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let instructions = extract_all_instructions(message, meta, &accounts);

    // Prefer the target program if it appears in any instruction.
    let program_id = instructions
        .iter()
        .find(|i| i.program_id == target_program)
        .or_else(|| instructions.first())
        .map(|i| i.program_id.clone())
        .unwrap_or_else(|| target_program.to_string());

    let instruction_type = Transaction::derive_instruction_type(&instructions);

    Some(Transaction {
        signature: signature.to_string(),
        program_id,
        block_slot,
        timestamp,
        success,
        instruction_type,
        accounts,
        fee_lamports,
        compute_units,
        instructions,
        cu_profile,
        classification: None,
        classification_debug: None,
    })
}

// Walks both top-level and inner instructions; top-level first, then inners
// in the order `getTransaction` emits them.
fn extract_all_instructions(
    message: &Value,
    meta: &Value,
    account_keys: &[String],
) -> Vec<ParsedInstruction> {
    let mut out = Vec::new();

    if let Some(instructions) = message.get("instructions").and_then(|v| v.as_array()) {
        for ix in instructions {
            if let Some(parsed) = parse_single_instruction(ix, account_keys) {
                out.push(parsed);
            }
        }
    }

    if let Some(inner_groups) = meta.get("innerInstructions").and_then(|v| v.as_array()) {
        for group in inner_groups {
            if let Some(ixs) = group.get("instructions").and_then(|v| v.as_array()) {
                for ix in ixs {
                    if let Some(parsed) = parse_single_instruction(ix, account_keys) {
                        out.push(parsed);
                    }
                }
            }
        }
    }

    out
}

fn parse_single_instruction(ix: &Value, account_keys: &[String]) -> Option<ParsedInstruction> {
    let program_id_index = ix.get("programIdIndex").and_then(|v| v.as_u64())? as usize;
    let program_id = account_keys.get(program_id_index)?.clone();

    let resolved_accounts: Vec<String> = ix
        .get("accounts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_u64())
                .filter_map(|i| account_keys.get(i as usize).cloned())
                .collect()
        })
        .unwrap_or_default();

    let data_bytes = ix
        .get("data")
        .and_then(|v| v.as_str())
        .and_then(|data| bs58::decode(data).into_vec().ok())
        .unwrap_or_default();

    let kind = classify_instruction(&program_id, &data_bytes);

    Some(ParsedInstruction {
        program_id,
        kind,
        accounts: resolved_accounts,
    })
}

fn classify_instruction(program_id: &str, data: &[u8]) -> InstructionKind {
    if data.is_empty() {
        return InstructionKind::Unknown;
    }

    if program_id == SPL_TOKEN_PROGRAM {
        return classify_spl_token(data);
    }

    if program_id == SYSTEM_PROGRAM {
        return classify_system_program(data);
    }

    if program_id == STAKE_PROGRAM || program_id == STAKE_POOL_PROGRAM {
        return classify_stake_program(data);
    }

    if program_id == SPL_MEMO_PROGRAM || program_id == MEMO_V1_PROGRAM {
        return InstructionKind::Other {
            name: "memo".to_string(),
        };
    }

    if program_id == BPF_LOADER_UPGRADEABLE {
        return classify_bpf_loader_upgradeable(data);
    }

    if program_id.starts_with("675kPX") && data.len() >= 8 {
        let disc = &data[..8];
        let name = match disc {
            [9, ..] => "swap".to_string(),
            [1, ..] => "initialize".to_string(),
            [3, ..] => "addLiquidity".to_string(),
            [4, ..] => "removeLiquidity".to_string(),
            _ => format!("unknown_0x{}", hex_prefix(disc)),
        };
        return InstructionKind::Other { name };
    }

    if program_id.starts_with("JUP") && data.len() >= 8 {
        let disc = &data[..8];
        let name = match disc {
            [229, 23, 203, 151, 122, 227, 173, 42] => "route".to_string(),
            [193, 32, 155, 51, 65, 214, 156, 129] => "sharedAccountsRoute".to_string(),
            _ => format!("unknown_0x{}", hex_prefix(disc)),
        };
        return InstructionKind::Other { name };
    }

    InstructionKind::Other {
        name: format!("ix_{}", data[0]),
    }
}

// SPL Token instructions are tagged by the first byte. Layouts:
//   Transfer        (3):  tag(1) | amount(8)
//   SetAuthority    (6):  tag(1) | authority_type(1) | option<new_authority>
//   TransferChecked (12): tag(1) | amount(8) | decimals(1)
fn classify_spl_token(data: &[u8]) -> InstructionKind {
    match data[0] {
        3 if data.len() >= 9 => {
            let amount = u64::from_le_bytes(data[1..9].try_into().unwrap_or([0; 8]));
            InstructionKind::TokenTransfer { amount }
        }
        6 => InstructionKind::SetAuthority,
        12 if data.len() >= 10 => {
            let amount = u64::from_le_bytes(data[1..9].try_into().unwrap_or([0; 8]));
            let decimals = data[9];
            InstructionKind::TokenTransferChecked { amount, decimals }
        }
        tag => InstructionKind::Other {
            name: format!("token_ix_{}", tag),
        },
    }
}

fn classify_system_program(data: &[u8]) -> InstructionKind {
    if data.len() >= 12 {
        let disc = u32::from_le_bytes(data[..4].try_into().unwrap_or([0; 4]));
        if disc == 2 {
            let lamports = u64::from_le_bytes(data[4..12].try_into().unwrap_or([0; 8]));
            return InstructionKind::SystemTransfer { lamports };
        }
    }

    InstructionKind::Other {
        name: format!("system_ix_{}", data[0]),
    }
}

fn classify_stake_program(data: &[u8]) -> InstructionKind {
    if data.len() >= 4 {
        let disc = u32::from_le_bytes(data[..4].try_into().unwrap_or([0; 4]));
        return match disc {
            2 => InstructionKind::StakeDelegate,
            4 => InstructionKind::StakeWithdraw,
            _ => InstructionKind::Other {
                name: format!("stake_ix_{}", disc),
            },
        };
    }

    InstructionKind::Unknown
}

fn classify_bpf_loader_upgradeable(data: &[u8]) -> InstructionKind {
    if data.len() >= 4 {
        let disc = u32::from_le_bytes(data[..4].try_into().unwrap_or([0; 4]));
        if disc == 3 {
            return InstructionKind::Upgrade;
        }
    }
    InstructionKind::Other {
        name: format!("loader_ix_{}", data[0]),
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().take(4).map(|b| format!("{:02x}", b)).collect()
}

pub fn parse_log_signature(notification: &Value) -> Option<String> {
    let params = notification.get("params")?;
    let result = params.get("result")?;
    let value = result.get("value")?;
    let signature = value.get("signature")?.as_str()?;
    Some(signature.to_string())
}

pub fn log_has_error(notification: &Value) -> Option<bool> {
    let params = notification.get("params")?;
    let result = params.get("result")?;
    let value = result.get("value")?;
    let err = value.get("err");
    Some(err.map_or(false, |e| !e.is_null()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a minimal getTransaction-shaped JSON with one top-level instruction.
    /// `data_bytes` is encoded as base58 to mimic the wire format.
    fn build_raw_with_ix(
        account_keys: Vec<&str>,
        program_id_index: usize,
        ix_account_indices: Vec<u64>,
        data_bytes: &[u8],
    ) -> Value {
        let data_b58 = bs58::encode(data_bytes).into_string();
        json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 281234567,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 200000
                },
                "transaction": {
                    "message": {
                        "accountKeys": account_keys,
                        "instructions": [{
                            "programIdIndex": program_id_index,
                            "accounts": ix_account_indices,
                            "data": data_b58
                        }]
                    }
                }
            }
        })
    }

    #[test]
    fn parse_log_signature_extracts_sig() {
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "logsNotification",
            "params": {
                "result": {
                    "context": { "slot": 123456 },
                    "value": {
                        "signature": "5UBdK1aPsEjR3hQuWqXBfKzE5bY3FhpRLpCaG8MJbkKv",
                        "err": null,
                        "logs": ["Program log: Instruction: Swap"]
                    }
                },
                "subscription": 0
            }
        });

        let sig = parse_log_signature(&notif).unwrap();
        assert_eq!(sig, "5UBdK1aPsEjR3hQuWqXBfKzE5bY3FhpRLpCaG8MJbkKv");
    }

    #[test]
    fn parse_transaction_basic_success_fields() {
        // Raydium swap discriminator [9,..]
        let raw = build_raw_with_ix(
            vec![
                "Wallet111111111111111111111111111111111111",
                "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
            ],
            1,
            vec![0],
            &[9, 0, 0, 0, 0, 0, 0, 0],
        );

        let tx = parse_transaction(
            &raw,
            "test_sig",
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
        )
        .unwrap();
        assert_eq!(tx.signature, "test_sig");
        assert_eq!(
            tx.program_id,
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
        );
        assert_eq!(tx.block_slot, 281234567);
        assert!(tx.success);
        assert_eq!(tx.fee_lamports, 5000);
        assert_eq!(tx.compute_units, 200000);
        assert_eq!(tx.accounts.len(), 2);
        assert_eq!(tx.instructions.len(), 1);
        assert_eq!(tx.instruction_type.as_deref(), Some("swap"));
    }

    #[test]
    fn parse_transaction_with_error_flag() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": { "InstructionError": [0, "Custom"] },
                    "fee": 5000,
                    "computeUnitsConsumed": 50000
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "program123"],
                        "instructions": [{
                            "programIdIndex": 1,
                            "accounts": [0],
                            "data": ""
                        }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "err_sig", "program123").unwrap();
        assert!(!tx.success);
    }

    #[test]
    fn classify_spl_token_set_authority() {
        // SetAuthority: tag=6, authority_type=2, option=0 (no new authority)
        let raw = build_raw_with_ix(vec!["mint", SPL_TOKEN_PROGRAM], 1, vec![0], &[6, 2, 0]);

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        assert_eq!(tx.instructions.len(), 1);
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::SetAuthority
        ));
        assert_eq!(tx.instruction_type.as_deref(), Some("setAuthority"));
    }

    #[test]
    fn classify_spl_token_transfer_extracts_amount() {
        // Transfer: tag=3, amount=1_000_000_000 (LE)
        let amount: u64 = 1_000_000_000;
        let mut data = vec![3u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let raw = build_raw_with_ix(vec!["src", "dst", SPL_TOKEN_PROGRAM], 2, vec![0, 1], &data);

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        assert_eq!(tx.instructions.len(), 1);
        match &tx.instructions[0].kind {
            InstructionKind::TokenTransfer { amount: a } => assert_eq!(*a, amount),
            other => panic!("expected TokenTransfer, got {:?}", other),
        }
        // Source/destination should be resolved as the first two account pubkeys
        assert_eq!(
            tx.instructions[0].accounts,
            vec!["src".to_string(), "dst".to_string()]
        );
    }

    #[test]
    fn classify_spl_token_transfer_checked_extracts_amount_and_decimals() {
        // TransferChecked: tag=12, amount=42_000_000 (LE), decimals=6
        let amount: u64 = 42_000_000;
        let mut data = vec![12u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(6);

        let raw = build_raw_with_ix(
            vec!["src", "mint", "dst", SPL_TOKEN_PROGRAM],
            3,
            vec![0, 1, 2],
            &data,
        );

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        assert_eq!(tx.instructions.len(), 1);
        match &tx.instructions[0].kind {
            InstructionKind::TokenTransferChecked {
                amount: a,
                decimals: d,
            } => {
                assert_eq!(*a, amount);
                assert_eq!(*d, 6);
            }
            other => panic!("expected TokenTransferChecked, got {:?}", other),
        }
    }

    #[test]
    fn classify_bpf_loader_upgradeable_upgrade() {
        // Upgrade: u32 LE = 3
        let data = [3u8, 0, 0, 0];
        let raw = build_raw_with_ix(vec!["payer", BPF_LOADER_UPGRADEABLE], 1, vec![0], &data);

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        assert_eq!(tx.instructions.len(), 1);
        assert!(matches!(tx.instructions[0].kind, InstructionKind::Upgrade));
        assert_eq!(tx.instruction_type.as_deref(), Some("upgrade"));
    }

    #[test]
    fn classify_unknown_program_falls_back_to_ix_tag() {
        let raw = build_raw_with_ix(
            vec!["wallet", "SomeUnknownProgram111111111111111111111111"],
            1,
            vec![0],
            &[42, 1, 2, 3],
        );

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        match &tx.instructions[0].kind {
            InstructionKind::Other { name } => assert_eq!(name, "ix_42"),
            other => panic!("expected Other, got {:?}", other),
        }
    }

    #[test]
    fn empty_data_classifies_as_unknown() {
        let raw = build_raw_with_ix(
            vec!["wallet", "Anything1111111111111111111111111111111111"],
            1,
            vec![0],
            &[],
        );

        let tx = parse_transaction(&raw, "sig", "any_target").unwrap();
        assert!(matches!(tx.instructions[0].kind, InstructionKind::Unknown));
        assert!(tx.instruction_type.is_none());
    }

    #[test]
    fn inner_instructions_are_extracted() {
        // Top-level: a Raydium swap. Inner: an SPL Token transfer.
        let amount: u64 = 500;
        let mut transfer_data = vec![3u8];
        transfer_data.extend_from_slice(&amount.to_le_bytes());
        let transfer_b58 = bs58::encode(&transfer_data).into_string();

        let swap_b58 = bs58::encode(&[9u8, 0, 0, 0, 0, 0, 0, 0]).into_string();

        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 1,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 1000,
                    "innerInstructions": [{
                        "index": 0,
                        "instructions": [{
                            "programIdIndex": 2,
                            "accounts": [0, 1],
                            "data": transfer_b58
                        }]
                    }]
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["src", "dst", SPL_TOKEN_PROGRAM, "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"],
                        "instructions": [{
                            "programIdIndex": 3,
                            "accounts": [0, 1],
                            "data": swap_b58
                        }]
                    }
                }
            }
        });

        let tx =
            parse_transaction(&raw, "sig", "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
        assert_eq!(tx.instructions.len(), 2);
        // First the top-level swap
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::Other { .. }
        ));
        // Then the inner Token transfer
        assert!(matches!(
            tx.instructions[1].kind,
            InstructionKind::TokenTransfer { .. }
        ));
    }
}
