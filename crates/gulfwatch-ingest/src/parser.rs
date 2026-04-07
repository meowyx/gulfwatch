use chrono::{DateTime, TimeZone, Utc};
use gulfwatch_core::Transaction;
use serde_json::Value;

pub fn parse_transaction(raw: &Value, signature: &str, target_program: &str) -> Option<Transaction> {
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

    let accounts: Vec<String> = message
        .get("accountKeys")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|a| a.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let (program_id, instruction_type) = extract_instruction_info(message, meta, &accounts, target_program);
    let program_id = program_id.unwrap_or_else(|| target_program.to_string());

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
    })
}

fn extract_instruction_info(
    message: &Value,
    meta: &Value,
    account_keys: &[String],
    target_program: &str,
) -> (Option<String>, Option<String>) {
    if let Some(instructions) = message.get("instructions").and_then(|v| v.as_array()) {
        for ix in instructions {
            if let Some((pid, itype)) = parse_single_instruction(ix, account_keys) {
                if pid == target_program {
                    return (Some(pid), itype);
                }
            }
        }
    }

    if let Some(inner_ixs) = meta.get("innerInstructions").and_then(|v| v.as_array()) {
        for group in inner_ixs {
            if let Some(ixs) = group.get("instructions").and_then(|v| v.as_array()) {
                for ix in ixs {
                    if let Some((pid, itype)) = parse_single_instruction(ix, account_keys) {
                        if pid == target_program {
                            return (Some(pid), itype);
                        }
                    }
                }
            }
        }
    }

    if let Some(instructions) = message.get("instructions").and_then(|v| v.as_array()) {
        for ix in instructions {
            if let Some((pid, itype)) = parse_single_instruction(ix, account_keys) {
                return (Some(pid), itype);
            }
        }
    }

    (None, None)
}

fn parse_single_instruction(
    ix: &Value,
    account_keys: &[String],
) -> Option<(String, Option<String>)> {
    let program_id_index = ix.get("programIdIndex").and_then(|v| v.as_u64())? as usize;
    let program_id = account_keys.get(program_id_index)?.clone();
    let instruction_type = ix
        .get("data")
        .and_then(|v| v.as_str())
        .and_then(|data| classify_instruction(&program_id, data));
    Some((program_id, instruction_type))
}

fn classify_instruction(program_id: &str, data: &str) -> Option<String> {
    let bytes = bs58::decode(data).into_vec().ok()?;
    if bytes.len() < 8 {
        return None;
    }

    let disc = &bytes[..8];

    if program_id.starts_with("675kPX") {
        return match disc {
            [9, ..] => Some("swap".to_string()),
            [1, ..] => Some("initialize".to_string()),
            [3, ..] => Some("addLiquidity".to_string()),
            [4, ..] => Some("removeLiquidity".to_string()),
            _ => Some(format!("unknown_0x{}", hex_prefix(disc))),
        };
    }

    if program_id.starts_with("JUP") {
        return match disc {
            [229, 23, 203, 151, 122, 227, 173, 42] => Some("route".to_string()),
            [193, 32, 155, 51, 65, 214, 156, 129] => Some("sharedAccountsRoute".to_string()),
            _ => Some(format!("unknown_0x{}", hex_prefix(disc))),
        };
    }

    Some(format!("ix_{}", disc[0]))
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(4)
        .map(|b| format!("{:02x}", b))
        .collect()
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
    fn parse_transaction_success() {
        let raw = json!({
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
                        "accountKeys": [
                            "Wallet111111111111111111111111111111111111",
                            "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8"
                        ],
                        "instructions": [{
                            "programIdIndex": 1,
                            "data": "2ZjT3bDMij7d"
                        }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "test_sig", "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
        assert_eq!(tx.signature, "test_sig");
        assert_eq!(tx.program_id, "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8");
        assert_eq!(tx.block_slot, 281234567);
        assert!(tx.success);
        assert_eq!(tx.fee_lamports, 5000);
        assert_eq!(tx.compute_units, 200000);
        assert_eq!(tx.accounts.len(), 2);
    }

    #[test]
    fn parse_transaction_with_error() {
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
                            "data": "2ZjT3bDMij7d"
                        }]
                    }
                }
            }
        });

        let tx = parse_transaction(&raw, "err_sig", "program123").unwrap();
        assert!(!tx.success);
    }
}
