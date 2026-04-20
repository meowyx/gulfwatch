// Anchor stores IDLs at `create_with_seed(find_program_address(&[], pid).0, "anchor:idl", pid)`.
// Account layout: [0..8] discriminator, [8..40] authority, [40..44] data_len LE u32, [44..] zstd(json).

use base64::Engine as _;
use curve25519_dalek::edwards::CompressedEdwardsY;
use gulfwatch_core::{AnchorIdl, AppState, IdlStatus};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";
const IDL_SEED: &str = "anchor:idl";
const HEADER_LEN: usize = 8 + 32 + 4;

#[derive(Debug)]
pub enum DiscoverError {
    Rpc(String),
    AccountNotFound,
    BadHeader(String),
    Decompress(String),
    Parse(String),
    BadProgramId(String),
}

impl std::fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoverError::Rpc(m) => write!(f, "rpc error: {m}"),
            DiscoverError::AccountNotFound => write!(
                f,
                "IDL account not found (program did not publish via `anchor idl init`)"
            ),
            DiscoverError::BadHeader(m) => write!(f, "bad IDL account header: {m}"),
            DiscoverError::Decompress(m) => write!(f, "zstd decompress failed: {m}"),
            DiscoverError::Parse(m) => write!(f, "IDL JSON parse failed: {m}"),
            DiscoverError::BadProgramId(m) => write!(f, "invalid program_id: {m}"),
        }
    }
}

impl std::error::Error for DiscoverError {}

pub fn spawn_boot_idl_discovery(state: AppState, rpc_url: String, program_ids: Vec<String>) {
    for program_id in program_ids {
        let state = state.clone();
        let rpc_url = rpc_url.clone();
        tokio::spawn(async move {
            state.set_idl_status(&program_id, IdlStatus::Loading).await;
            match fetch_onchain_idl(&rpc_url, &program_id).await {
                Ok(idl) => {
                    let name = idl.name.clone();
                    let ix_count = idl.instructions.len();
                    let err_count = idl.errors.len();
                    state.upsert_idl(&program_id, idl).await;
                    info!(
                        program_id = %program_id,
                        name = %name,
                        instructions = ix_count,
                        errors = err_count,
                        "IDL auto-discovered from on-chain"
                    );
                }
                Err(e) => {
                    state
                        .set_idl_status(&program_id, IdlStatus::Unavailable)
                        .await;
                    // AccountNotFound is the expected state for programs that skipped
                    // `anchor idl init` and for all native programs — log at info so
                    // startup isn't noisy. Real RPC errors still surface as warn.
                    match &e {
                        DiscoverError::AccountNotFound => info!(
                            program_id = %program_id,
                            "No on-chain IDL — manual upload via POST /api/programs/{{id}}/idl is available"
                        ),
                        _ => warn!(program_id = %program_id, error = %e, "IDL discovery failed"),
                    }
                }
            }
        });
    }
}

pub async fn fetch_onchain_idl(
    rpc_url: &str,
    program_id: &str,
) -> Result<AnchorIdl, DiscoverError> {
    let program_id_bytes = decode_pubkey(program_id)?;
    let idl_addr_bytes = derive_idl_address(&program_id_bytes);
    let idl_addr_b58 = bs58::encode(idl_addr_bytes).into_string();

    let raw = fetch_account_data(rpc_url, &idl_addr_b58).await?;
    let compressed = parse_idl_account_payload(&raw)?;
    let json_bytes = zstd::decode_all(compressed)
        .map_err(|e| DiscoverError::Decompress(e.to_string()))?;
    serde_json::from_slice::<AnchorIdl>(&json_bytes)
        .map_err(|e| DiscoverError::Parse(e.to_string()))
}

pub fn derive_idl_address(program_id: &[u8; 32]) -> [u8; 32] {
    let (base, _bump) = find_pda_empty_seeds(program_id);
    create_with_seed(&base, IDL_SEED, program_id)
}

fn find_pda_empty_seeds(program_id: &[u8; 32]) -> ([u8; 32], u8) {
    for bump in (0u8..=u8::MAX).rev() {
        let mut hasher = Sha256::new();
        hasher.update([bump]);
        hasher.update(program_id);
        hasher.update(PDA_MARKER);
        let hash: [u8; 32] = hasher.finalize().into();
        if !is_on_curve(&hash) {
            return (hash, bump);
        }
    }
    panic!("find_program_address: no off-curve bump for program_id");
}

fn create_with_seed(base: &[u8; 32], seed: &str, owner: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(base);
    hasher.update(seed.as_bytes());
    hasher.update(owner);
    hasher.finalize().into()
}

fn is_on_curve(bytes: &[u8; 32]) -> bool {
    CompressedEdwardsY(*bytes).decompress().is_some()
}

fn decode_pubkey(b58: &str) -> Result<[u8; 32], DiscoverError> {
    let bytes = bs58::decode(b58)
        .into_vec()
        .map_err(|e| DiscoverError::BadProgramId(format!("base58 decode: {e}")))?;
    if bytes.len() != 32 {
        return Err(DiscoverError::BadProgramId(format!(
            "expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub fn parse_idl_account_payload(raw: &[u8]) -> Result<&[u8], DiscoverError> {
    if raw.len() < HEADER_LEN {
        return Err(DiscoverError::BadHeader(format!(
            "account data too short: {} bytes (need at least {HEADER_LEN})",
            raw.len()
        )));
    }
    let len_bytes: [u8; 4] = raw[40..44].try_into().expect("len slice is 4 bytes");
    let data_len = u32::from_le_bytes(len_bytes) as usize;
    let end = HEADER_LEN.checked_add(data_len).ok_or_else(|| {
        DiscoverError::BadHeader(format!("length overflow: data_len={data_len}"))
    })?;
    if end > raw.len() {
        return Err(DiscoverError::BadHeader(format!(
            "declared data_len {data_len} exceeds account size {}",
            raw.len() - HEADER_LEN
        )));
    }
    Ok(&raw[HEADER_LEN..end])
}

async fn fetch_account_data(rpc_url: &str, address_b58: &str) -> Result<Vec<u8>, DiscoverError> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAccountInfo",
        "params": [address_b58, { "encoding": "base64" }]
    });
    let resp: Value = reqwest::Client::new()
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| DiscoverError::Rpc(e.to_string()))?
        .json()
        .await
        .map_err(|e| DiscoverError::Rpc(e.to_string()))?;

    if let Some(err) = resp.get("error") {
        return Err(DiscoverError::Rpc(err.to_string()));
    }

    let value = resp.get("result").and_then(|r| r.get("value"));
    let value = match value {
        None | Some(Value::Null) => return Err(DiscoverError::AccountNotFound),
        Some(v) => v,
    };

    let data_arr = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| DiscoverError::Rpc("missing data array in getAccountInfo".to_string()))?;
    let b64 = data_arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| DiscoverError::Rpc("no base64 payload in data".to_string()))?;

    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| DiscoverError::Rpc(format!("base64 decode failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_idl_address_is_deterministic() {
        let program_id = [1u8; 32];
        let a = derive_idl_address(&program_id);
        let b = derive_idl_address(&program_id);
        assert_eq!(a, b);
    }

    #[test]
    fn derive_idl_address_differs_per_program() {
        let a = derive_idl_address(&[1u8; 32]);
        let b = derive_idl_address(&[2u8; 32]);
        assert_ne!(a, b);
    }

    #[test]
    fn find_pda_produces_off_curve_result() {
        let (pda, _bump) = find_pda_empty_seeds(&[7u8; 32]);
        assert!(!is_on_curve(&pda));
    }

    #[test]
    fn decode_pubkey_round_trips_through_bs58() {
        let bytes = [9u8; 32];
        let b58 = bs58::encode(bytes).into_string();
        let decoded = decode_pubkey(&b58).unwrap();
        assert_eq!(decoded, bytes);
    }

    #[test]
    fn decode_pubkey_rejects_wrong_length() {
        let short = bs58::encode([1u8; 20]).into_string();
        assert!(matches!(
            decode_pubkey(&short),
            Err(DiscoverError::BadProgramId(_))
        ));
    }

    #[test]
    fn parse_idl_account_payload_extracts_data_region() {
        let compressed = b"fake-zstd-bytes-go-here";
        let mut raw = Vec::with_capacity(HEADER_LEN + compressed.len());
        raw.extend_from_slice(&[0u8; 8]);
        raw.extend_from_slice(&[0u8; 32]);
        raw.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        raw.extend_from_slice(compressed);

        let payload = parse_idl_account_payload(&raw).unwrap();
        assert_eq!(payload, compressed);
    }

    #[test]
    fn parse_idl_account_payload_rejects_too_short() {
        let raw = [0u8; 20];
        assert!(matches!(
            parse_idl_account_payload(&raw),
            Err(DiscoverError::BadHeader(_))
        ));
    }

    #[test]
    fn parse_idl_account_payload_rejects_length_overshoot() {
        let mut raw = vec![0u8; HEADER_LEN + 5];
        raw[40..44].copy_from_slice(&(999u32).to_le_bytes());
        assert!(matches!(
            parse_idl_account_payload(&raw),
            Err(DiscoverError::BadHeader(_))
        ));
    }

    #[test]
    fn end_to_end_header_and_zstd_roundtrip() {
        let idl_json = r#"{"name":"synth","instructions":[{"name":"poke"}]}"#;
        let compressed = zstd::encode_all(idl_json.as_bytes(), 3).unwrap();

        let mut raw = Vec::new();
        raw.extend_from_slice(&[0u8; 8]);
        raw.extend_from_slice(&[0u8; 32]);
        raw.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        raw.extend_from_slice(&compressed);

        let payload = parse_idl_account_payload(&raw).unwrap();
        let decompressed = zstd::decode_all(payload).unwrap();
        let idl: AnchorIdl = serde_json::from_slice(&decompressed).unwrap();
        assert_eq!(idl.name, "synth");
        assert_eq!(idl.instructions.len(), 1);
        assert_eq!(idl.instructions[0].name, "poke");
    }
}
