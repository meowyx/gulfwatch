use chrono::{DateTime, TimeZone, Utc};
use gulfwatch_core::{
    parse_logs, BalanceDiff, InstructionKind, ParsedInstruction, SolDelta, TokenDelta, Transaction,
    TransactionError,
};
use serde_json::Value;

use crate::program_ids::{
    ASSOCIATED_TOKEN_PROGRAM, BPF_LOADER_UPGRADEABLE, COMPUTE_BUDGET_PROGRAM, MEMO_V1_PROGRAM,
    SPL_MEMO_PROGRAM, SPL_TOKEN_PROGRAM, STAKE_POOL_PROGRAM, STAKE_PROGRAM, SYSTEM_PROGRAM,
    TOKEN_2022_PROGRAM,
};

pub fn parse_transaction(
    raw: &Value,
    signature: &str,
    monitored: &[String],
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

    let (instructions, top_level_count) = extract_all_instructions(message, meta, &accounts);
    let top_level = &instructions[..top_level_count];

    let program_id = top_level
        .iter()
        .find(|i| monitored.iter().any(|m| m == &i.program_id))
        .or_else(|| top_level.first())
        .map(|i| i.program_id.clone())
        .unwrap_or_default();

    let instruction_type = Transaction::derive_instruction_type(&instructions);

    let balance_diff = extract_balance_diff(meta, &accounts);
    let tx_error = extract_tx_error(meta);

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
        logs: log_messages,
        balance_diff,
        tx_error,
    })
}

fn extract_tx_error(meta: &Value) -> Option<TransactionError> {
    let err = meta.get("err")?;
    if err.is_null() {
        return None;
    }
    let raw = serde_json::to_string(err).unwrap_or_else(|_| "<unserializable>".to_string());

    // Top-level string variant — e.g. "BlockhashNotFound", "AlreadyProcessed".
    if let Some(s) = err.as_str() {
        return Some(TransactionError {
            instruction_index: None,
            kind: s.to_string(),
            custom_code: None,
            raw,
        });
    }

    // {"InstructionError": [<idx>, <inner>]} — the common per-instruction shape.
    if let Some(arr) = err.get("InstructionError").and_then(|v| v.as_array()) {
        let idx = arr.first().and_then(|v| v.as_u64()).map(|n| n as usize);
        let (kind, custom_code) = match arr.get(1) {
            Some(Value::String(s)) => (s.clone(), None),
            Some(Value::Object(map)) => match map.iter().next() {
                Some((k, v)) if k == "Custom" => {
                    (k.clone(), v.as_u64().map(|n| n as u32))
                }
                Some((k, _)) => (k.clone(), None),
                None => ("UnknownInstructionError".to_string(), None),
            },
            _ => ("UnknownInstructionError".to_string(), None),
        };
        return Some(TransactionError {
            instruction_index: idx,
            kind,
            custom_code,
            raw,
        });
    }

    // Fallback — top-level single-key object like {"InsufficientFundsForRent": {...}}.
    if let Some((k, _)) = err.as_object().and_then(|m| m.iter().next()) {
        return Some(TransactionError {
            instruction_index: None,
            kind: k.clone(),
            custom_code: None,
            raw,
        });
    }

    None
}

fn extract_balance_diff(meta: &Value, accounts: &[String]) -> Option<BalanceDiff> {
    let pre = meta.get("preBalances").and_then(|v| v.as_array());
    let post = meta.get("postBalances").and_then(|v| v.as_array());

    let mut sol = Vec::new();
    if let (Some(pre), Some(post)) = (pre, post) {
        for (i, (p, q)) in pre.iter().zip(post.iter()).enumerate() {
            let pre_v = p.as_u64().unwrap_or(0);
            let post_v = q.as_u64().unwrap_or(0);
            if pre_v != post_v {
                sol.push(SolDelta {
                    account: accounts.get(i).cloned().unwrap_or_default(),
                    account_index: i,
                    pre_lamports: pre_v,
                    post_lamports: post_v,
                    delta_lamports: post_v as i128 - pre_v as i128,
                });
            }
        }
    }

    let tokens = extract_token_diffs(meta, accounts);

    if sol.is_empty() && tokens.is_empty() {
        return None;
    }
    Some(BalanceDiff { sol, tokens })
}

fn extract_token_diffs(meta: &Value, accounts: &[String]) -> Vec<TokenDelta> {
    use std::collections::BTreeMap;

    let pre = meta
        .get("preTokenBalances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let post = meta
        .get("postTokenBalances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Join key: (account_index, mint). Token Programs constrain a token account to
    // exactly one mint, but the JSON is sparse on both sides so we still map both.
    let mut map: BTreeMap<(usize, String), (Option<&Value>, Option<&Value>)> = BTreeMap::new();

    for entry in &pre {
        if let Some(key) = token_balance_key(entry) {
            map.entry(key).or_insert((None, None)).0 = Some(entry);
        }
    }
    for entry in &post {
        if let Some(key) = token_balance_key(entry) {
            map.entry(key).or_insert((None, None)).1 = Some(entry);
        }
    }

    let mut out = Vec::with_capacity(map.len());
    for ((account_index, mint), (pre_v, post_v)) in map {
        let pre_amount = pre_v.and_then(token_amount).unwrap_or(0);
        let post_amount = post_v.and_then(token_amount).unwrap_or(0);
        if pre_amount == post_amount {
            continue;
        }
        let decimals = pre_v
            .and_then(token_decimals)
            .or_else(|| post_v.and_then(token_decimals))
            .unwrap_or(0);
        let owner = pre_v
            .and_then(token_owner)
            .or_else(|| post_v.and_then(token_owner));
        out.push(TokenDelta {
            account: accounts.get(account_index).cloned().unwrap_or_default(),
            account_index,
            mint,
            owner,
            pre_amount,
            post_amount,
            delta: post_amount as i128 - pre_amount as i128,
            decimals,
        });
    }
    out
}

fn token_balance_key(entry: &Value) -> Option<(usize, String)> {
    let idx = entry.get("accountIndex")?.as_u64()? as usize;
    let mint = entry.get("mint")?.as_str()?.to_string();
    Some((idx, mint))
}

fn token_amount(entry: &Value) -> Option<u64> {
    entry
        .get("uiTokenAmount")
        .and_then(|v| v.get("amount"))
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
}

fn token_decimals(entry: &Value) -> Option<u8> {
    entry
        .get("uiTokenAmount")
        .and_then(|v| v.get("decimals"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u8)
}

fn token_owner(entry: &Value) -> Option<String> {
    entry.get("owner").and_then(|v| v.as_str()).map(String::from)
}

// Walks both top-level and inner instructions; top-level first, then inners
// in the order `getTransaction` emits them.
fn extract_all_instructions(
    message: &Value,
    meta: &Value,
    account_keys: &[String],
) -> (Vec<ParsedInstruction>, usize) {
    let mut out = Vec::new();

    if let Some(instructions) = message.get("instructions").and_then(|v| v.as_array()) {
        for ix in instructions {
            if let Some(parsed) = parse_single_instruction(ix, account_keys) {
                out.push(parsed);
            }
        }
    }

    let top_level_count = out.len();

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

    (out, top_level_count)
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

    if program_id == SPL_TOKEN_PROGRAM || program_id == TOKEN_2022_PROGRAM {
        if program_id == TOKEN_2022_PROGRAM {
            if let Some(kind) = classify_token_2022_extension(data) {
                return kind;
            }
        }
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

    if program_id == ASSOCIATED_TOKEN_PROGRAM {
        return classify_associated_token_program(data);
    }

    if program_id == COMPUTE_BUDGET_PROGRAM {
        return classify_compute_budget_program(data);
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
        6 => {
            let authority_type = data.get(1).copied().unwrap_or(u8::MAX);
            InstructionKind::SetAuthority { authority_type }
        }
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

// Tags from spl-token-2022 interface/src/instruction.rs TokenInstruction.
fn classify_token_2022_extension(data: &[u8]) -> Option<InstructionKind> {
    match data[0] {
        26 => match data.get(1)? {
            0 | 5 => Some(InstructionKind::SetTransferFee),
            _ => None,
        },
        28 => {
            let sub = *data.get(1)?;
            let state = *data.get(2)?;
            let frozen = state == 2;
            match sub {
                0 => Some(InstructionKind::InitializeDefaultAccountState { frozen }),
                1 => Some(InstructionKind::UpdateDefaultAccountState { frozen }),
                _ => None,
            }
        }
        35 => Some(InstructionKind::InitializePermanentDelegate),
        36 => match data.get(1)? {
            0 => Some(InstructionKind::InitializeTransferHook),
            1 => Some(InstructionKind::UpdateTransferHook),
            _ => None,
        },
        _ => None,
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

fn classify_associated_token_program(data: &[u8]) -> InstructionKind {
    let name = match data.first().copied() {
        None => "createAta",
        Some(0) => "createAta",
        Some(1) => "createAtaIdempotent",
        Some(2) => "recoverNestedAta",
        Some(tag) => return InstructionKind::Other { name: format!("ata_ix_{tag}") },
    };
    InstructionKind::Other { name: name.to_string() }
}

fn classify_compute_budget_program(data: &[u8]) -> InstructionKind {
    let tag = match data.first().copied() {
        Some(t) => t,
        None => return InstructionKind::Unknown,
    };
    let name = match tag {
        0 => "requestUnitsDeprecated".to_string(),
        1 if data.len() >= 5 => {
            let bytes = u32::from_le_bytes(data[1..5].try_into().unwrap_or([0; 4]));
            format!("requestHeapFrame({bytes})")
        }
        2 if data.len() >= 5 => {
            let units = u32::from_le_bytes(data[1..5].try_into().unwrap_or([0; 4]));
            format!("setComputeUnitLimit({units})")
        }
        3 if data.len() >= 9 => {
            let micro_lamports = u64::from_le_bytes(data[1..9].try_into().unwrap_or([0; 8]));
            format!("setComputeUnitPrice({micro_lamports})")
        }
        4 if data.len() >= 5 => {
            let bytes = u32::from_le_bytes(data[1..5].try_into().unwrap_or([0; 4]));
            format!("setLoadedAccountsDataSizeLimit({bytes})")
        }
        tag => format!("compute_budget_ix_{tag}"),
    };
    InstructionKind::Other { name }
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
            &["675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string()],
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

        let tx = parse_transaction(&raw, "err_sig", &["program123".to_string()]).unwrap();
        assert!(!tx.success);
    }

    #[test]
    fn parse_transaction_preserves_log_messages_for_deep_dive() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 200_000,
                    "logMessages": [
                        "Program 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 invoke [1]",
                        "Program log: ray_log: ABC=",
                        "Program 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 consumed 12345 of 200000 compute units",
                        "Program 675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8 success"
                    ]
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"],
                        "instructions": [{ "programIdIndex": 1, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(
            &raw,
            "log_sig",
            &["675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string()],
        )
        .unwrap();

        assert_eq!(tx.logs.len(), 4);
        assert!(tx.logs[1].contains("ray_log"));
        // Logs are kept verbatim and CU profile derived from the same source.
        assert!(tx.cu_profile.is_some());
    }

    #[test]
    fn parse_transaction_extracts_custom_instruction_error() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": { "InstructionError": [2, { "Custom": 6004 }] },
                    "fee": 5000,
                    "computeUnitsConsumed": 0
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "prog"],
                        "instructions": [{ "programIdIndex": 1, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "custom_err_sig", &["prog".to_string()]).unwrap();
        assert!(!tx.success);
        let err = tx.tx_error.expect("error metadata should be parsed");
        assert_eq!(err.instruction_index, Some(2));
        assert_eq!(err.kind, "Custom");
        assert_eq!(err.custom_code, Some(6004));
    }

    #[test]
    fn parse_transaction_extracts_named_instruction_error() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": { "InstructionError": [0, "AccountNotFound"] },
                    "fee": 5000,
                    "computeUnitsConsumed": 0
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "prog"],
                        "instructions": [{ "programIdIndex": 1, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "named_err_sig", &["prog".to_string()]).unwrap();
        let err = tx.tx_error.expect("error metadata should be parsed");
        assert_eq!(err.instruction_index, Some(0));
        assert_eq!(err.kind, "AccountNotFound");
        assert!(err.custom_code.is_none());
    }

    #[test]
    fn parse_transaction_extracts_top_level_string_error() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": "BlockhashNotFound",
                    "fee": 5000,
                    "computeUnitsConsumed": 0
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "prog"],
                        "instructions": [{ "programIdIndex": 1, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "top_err_sig", &["prog".to_string()]).unwrap();
        let err = tx.tx_error.expect("error metadata should be parsed");
        assert!(err.instruction_index.is_none());
        assert_eq!(err.kind, "BlockhashNotFound");
    }

    #[test]
    fn parse_transaction_no_tx_error_when_success() {
        // The first existing test (parse_transaction_basic_success_fields) already
        // covers success → success=true; here we confirm the new field is None.
        let raw = build_raw_with_ix(
            vec!["wallet", "prog"],
            1,
            vec![0],
            &[9, 0, 0, 0, 0, 0, 0, 0],
        );
        let tx = parse_transaction(&raw, "ok_sig", &["prog".to_string()]).unwrap();
        assert!(tx.tx_error.is_none());
    }

    #[test]
    fn parse_transaction_extracts_sol_balance_diff_when_changed() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 0,
                    "preBalances": [1_000_000_000u64, 500_000_000u64, 0u64],
                    "postBalances": [999_995_000u64, 500_000_000u64, 0u64]
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["payer", "untouched", "system"],
                        "instructions": [{ "programIdIndex": 2, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "sol_diff_sig", &["system".to_string()]).unwrap();
        let diff = tx.balance_diff.expect("balance diff should be present");
        assert_eq!(diff.sol.len(), 1, "only the changed account is included");
        assert_eq!(diff.sol[0].account, "payer");
        assert_eq!(diff.sol[0].account_index, 0);
        assert_eq!(diff.sol[0].delta_lamports, -5_000);
        assert!(diff.tokens.is_empty());
    }

    #[test]
    fn parse_transaction_extracts_token_balance_diff_with_join() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 0,
                    "preBalances": [1, 1, 1],
                    "postBalances": [1, 1, 1],
                    "preTokenBalances": [
                        {
                            "accountIndex": 1,
                            "mint": "MINT_USDC",
                            "owner": "alice",
                            "uiTokenAmount": { "amount": "1000000", "decimals": 6, "uiAmount": 1.0, "uiAmountString": "1" }
                        }
                    ],
                    "postTokenBalances": [
                        {
                            "accountIndex": 1,
                            "mint": "MINT_USDC",
                            "owner": "alice",
                            "uiTokenAmount": { "amount": "500000", "decimals": 6, "uiAmount": 0.5, "uiAmountString": "0.5" }
                        },
                        {
                            "accountIndex": 2,
                            "mint": "MINT_USDC",
                            "owner": "bob",
                            "uiTokenAmount": { "amount": "500000", "decimals": 6, "uiAmount": 0.5, "uiAmountString": "0.5" }
                        }
                    ]
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["payer", "alice_ata", "bob_ata"],
                        "instructions": [{ "programIdIndex": 0, "accounts": [], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "tok_diff_sig", &["payer".to_string()]).unwrap();
        let diff = tx.balance_diff.expect("balance diff should be present");
        assert_eq!(diff.tokens.len(), 2);

        let alice = diff.tokens.iter().find(|t| t.account == "alice_ata").unwrap();
        assert_eq!(alice.delta, -500_000);
        assert_eq!(alice.decimals, 6);
        assert_eq!(alice.owner.as_deref(), Some("alice"));

        let bob = diff.tokens.iter().find(|t| t.account == "bob_ata").unwrap();
        assert_eq!(bob.delta, 500_000);
        assert_eq!(bob.pre_amount, 0, "newly created account has no pre");
        assert_eq!(bob.post_amount, 500_000);
    }

    #[test]
    fn parse_transaction_balance_diff_none_when_nothing_changes() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": {
                    "err": null,
                    "fee": 5000,
                    "computeUnitsConsumed": 0,
                    "preBalances": [1_000, 2_000],
                    "postBalances": [1_000, 2_000]
                },
                "transaction": {
                    "message": {
                        "accountKeys": ["a", "b"],
                        "instructions": [{ "programIdIndex": 0, "accounts": [], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "noop_sig", &["a".to_string()]).unwrap();
        assert!(tx.balance_diff.is_none(), "no changes → no diff struct");
    }

    #[test]
    fn parse_transaction_logs_default_to_empty_when_missing() {
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 100,
                "blockTime": 1712000000,
                "meta": { "err": null, "fee": 5000, "computeUnitsConsumed": 0 },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "prog"],
                        "instructions": [{ "programIdIndex": 1, "accounts": [0], "data": "" }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "no_logs_sig", &["prog".to_string()]).unwrap();
        assert!(tx.logs.is_empty());
        assert!(tx.cu_profile.is_none());
    }

    #[test]
    fn classify_spl_token_set_authority() {
        // SetAuthority: tag=6, authority_type=2, option=0 (no new authority)
        let raw = build_raw_with_ix(vec!["mint", SPL_TOKEN_PROGRAM], 1, vec![0], &[6, 2, 0]);

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert_eq!(tx.instructions.len(), 1);
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::SetAuthority { authority_type: 2 }
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

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
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

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
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

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
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

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
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

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
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
            parse_transaction(&raw, "sig", &["675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string()]).unwrap();
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

    #[test]
    fn top_level_program_wins_over_cpi_when_both_monitored() {
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

        let monitored = vec![
            TOKEN_2022_PROGRAM.to_string(),
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string(),
            SPL_TOKEN_PROGRAM.to_string(),
        ];
        let tx = parse_transaction(&raw, "sig", &monitored).unwrap();
        assert_eq!(
            tx.program_id,
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
        );
    }

    #[test]
    fn top_level_program_selected_when_none_monitored_match() {
        let swap_b58 = bs58::encode(&[9u8, 0, 0, 0, 0, 0, 0, 0]).into_string();
        let raw = json!({
            "jsonrpc": "2.0",
            "result": {
                "slot": 1,
                "blockTime": 1712000000,
                "meta": { "err": null, "fee": 0, "computeUnitsConsumed": 0 },
                "transaction": {
                    "message": {
                        "accountKeys": ["wallet", "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"],
                        "instructions": [{
                            "programIdIndex": 1,
                            "accounts": [0],
                            "data": swap_b58
                        }]
                    }
                }
            }
        });

        let monitored = vec!["someOtherProgram".to_string()];
        let tx = parse_transaction(&raw, "sig", &monitored).unwrap();
        assert_eq!(
            tx.program_id,
            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
        );
    }

    #[test]
    fn classify_token_2022_base_transfer_uses_spl_token_path() {
        // Transfer: tag=3, amount=500 LE
        let amount: u64 = 500;
        let mut data = vec![3u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let raw = build_raw_with_ix(
            vec!["src", "dst", TOKEN_2022_PROGRAM],
            2,
            vec![0, 1],
            &data,
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::TokenTransfer { amount: 500 }
        ));
    }

    #[test]
    fn classify_token_2022_set_authority_carries_type_byte() {
        // SetAuthority: tag=6, authority_type=8 (PermanentDelegate)
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[6, 8, 0],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::SetAuthority { authority_type: 8 }
        ));
    }

    #[test]
    fn classify_token_2022_initialize_transfer_hook() {
        // TransferHookExtension (tag 36), sub-instruction 0 = Initialize
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[36, 0],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::InitializeTransferHook
        ));
    }

    #[test]
    fn classify_token_2022_update_transfer_hook() {
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[36, 1],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::UpdateTransferHook
        ));
    }

    #[test]
    fn classify_token_2022_initialize_permanent_delegate() {
        // InitializePermanentDelegate (tag 35), top-level with no sub tag
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[35],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::InitializePermanentDelegate
        ));
    }

    #[test]
    fn classify_token_2022_set_transfer_fee_from_extension() {
        // TransferFeeExtension (tag 26), sub 5 = SetTransferFee
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[26, 5],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::SetTransferFee
        ));
    }

    #[test]
    fn classify_token_2022_default_state_update_frozen() {
        // DefaultAccountStateExtension (tag 28), sub 1 = Update, state 2 = Frozen
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[28, 1, 2],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::UpdateDefaultAccountState { frozen: true }
        ));
    }

    #[test]
    fn classify_token_2022_default_state_initialize_not_frozen() {
        // DefaultAccountStateExtension, sub 0 = Initialize, state 1 = Initialized
        let raw = build_raw_with_ix(
            vec!["mint", TOKEN_2022_PROGRAM],
            1,
            vec![0],
            &[28, 0, 1],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        assert!(matches!(
            tx.instructions[0].kind,
            InstructionKind::InitializeDefaultAccountState { frozen: false }
        ));
    }

    #[test]
    fn classify_associated_token_create() {
        let raw = build_raw_with_ix(
            vec!["payer", ASSOCIATED_TOKEN_PROGRAM],
            1,
            vec![0],
            &[0],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        match &tx.instructions[0].kind {
            InstructionKind::Other { name } => assert_eq!(name, "createAta"),
            other => panic!("expected createAta, got {other:?}"),
        }
    }

    #[test]
    fn classify_associated_token_create_idempotent() {
        let raw = build_raw_with_ix(
            vec!["payer", ASSOCIATED_TOKEN_PROGRAM],
            1,
            vec![0],
            &[1],
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        match &tx.instructions[0].kind {
            InstructionKind::Other { name } => assert_eq!(name, "createAtaIdempotent"),
            other => panic!("expected createAtaIdempotent, got {other:?}"),
        }
    }

    #[test]
    fn classify_compute_budget_set_compute_unit_limit() {
        // SetComputeUnitLimit (tag 2), u32 LE = 200_000
        let limit: u32 = 200_000;
        let mut data = vec![2u8];
        data.extend_from_slice(&limit.to_le_bytes());

        let raw = build_raw_with_ix(
            vec!["payer", COMPUTE_BUDGET_PROGRAM],
            1,
            vec![0],
            &data,
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        match &tx.instructions[0].kind {
            InstructionKind::Other { name } => assert_eq!(name, "setComputeUnitLimit(200000)"),
            other => panic!("expected setComputeUnitLimit, got {other:?}"),
        }
    }

    #[test]
    fn classify_compute_budget_set_compute_unit_price() {
        // SetComputeUnitPrice (tag 3), u64 LE = 5_000 micro-lamports
        let price: u64 = 5_000;
        let mut data = vec![3u8];
        data.extend_from_slice(&price.to_le_bytes());

        let raw = build_raw_with_ix(
            vec!["payer", COMPUTE_BUDGET_PROGRAM],
            1,
            vec![0],
            &data,
        );

        let tx = parse_transaction(&raw, "sig", &["any_target".to_string()]).unwrap();
        match &tx.instructions[0].kind {
            InstructionKind::Other { name } => assert_eq!(name, "setComputeUnitPrice(5000)"),
            other => panic!("expected setComputeUnitPrice, got {other:?}"),
        }
    }
}
