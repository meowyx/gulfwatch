// Anchor stores IDLs at `create_with_seed(find_program_address(&[], pid).0, "anchor:idl", pid)`.
// Account layout: [0..8] discriminator, [8..40] authority, [40..44] data_len LE u32, [44..] zstd(json).

use std::io::Read as _;

use base64::Engine as _;
use curve25519_dalek::edwards::CompressedEdwardsY;
use flate2::read::ZlibDecoder;
use gulfwatch_core::{parse_idl_json, AppState, IdlDocument, IdlStatus};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use std::collections::HashMap;
use std::sync::Arc;

use crate::idl_registry::load_idl_registry;

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
    let registry: HashMap<String, gulfwatch_core::IdlDocument> = load_idl_registry();
    info!(
        count = registry.len(),
        user_dir = ?crate::idl_registry::user_idl_dir(),
        "runtime IDL registry loaded"
    );
    let registry = Arc::new(registry);

    for program_id in program_ids {
        let state = state.clone();
        let rpc_url = rpc_url.clone();
        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            state.set_idl_status(&program_id, IdlStatus::Loading).await;

            match fetch_onchain_idl(&rpc_url, &program_id).await {
                Ok(idl) => {
                    register_idl(&state, &program_id, idl, "on-chain").await;
                }
                Err(on_chain_err) => {
                    if let Some(idl) = registry.get(&program_id).cloned() {
                        register_idl(&state, &program_id, idl, "bundled").await;
                    } else {
                        record_failure(&state, &program_id, &on_chain_err).await;
                    }
                }
            }
        });
    }
}

async fn register_idl(state: &AppState, program_id: &str, idl: IdlDocument, source: &str) {
    let name = idl.name.clone();
    let ix_count = idl.instructions.len();
    let err_count = idl.errors.len();
    state.upsert_idl(program_id, idl).await;
    info!(
        program_id = %program_id,
        source = %source,
        name = %name,
        instructions = ix_count,
        errors = err_count,
        "IDL registered"
    );
}

async fn record_failure(state: &AppState, program_id: &str, err: &DiscoverError) {
    state.set_idl_failure(program_id, err.to_string()).await;
    match err {
        // Expected for native programs — keep startup quiet, warn on real errors.
        DiscoverError::AccountNotFound => info!(
            program_id = %program_id,
            "No on-chain IDL — manual upload via POST /api/programs/{{id}}/idl is available"
        ),
        _ => warn!(program_id = %program_id, error = %err, "IDL discovery failed"),
    }
}

pub async fn fetch_onchain_idl(
    rpc_url: &str,
    program_id: &str,
) -> Result<IdlDocument, DiscoverError> {
    let program_id_bytes = decode_pubkey(program_id)?;
    let idl_addr_bytes = derive_idl_address(&program_id_bytes);
    let idl_addr_b58 = bs58::encode(idl_addr_bytes).into_string();

    let raw = fetch_account_data(rpc_url, &idl_addr_b58).await?;
    let compressed = parse_idl_account_payload(&raw)?;
    let json_bytes = decompress_idl_payload(compressed)?;
    parse_idl_json(&json_bytes).map_err(|e| DiscoverError::Parse(e.to_string()))
}

// Anchor 0.29 = zstd (magic 28 B5 2F FD). Anchor 0.30+ = zlib (magic 78 ..).
// Dispatch on magic bytes; mis-dispatch would silently truncate the IDL.
pub fn decompress_idl_payload(compressed: &[u8]) -> Result<Vec<u8>, DiscoverError> {
    if compressed.len() >= 4 && compressed[..4] == [0x28, 0xB5, 0x2F, 0xFD] {
        return zstd::decode_all(compressed)
            .map_err(|e| DiscoverError::Decompress(format!("zstd: {e}")));
    }
    if compressed.len() >= 2
        && compressed[0] == 0x78
        && matches!(compressed[1], 0x01 | 0x5E | 0x9C | 0xDA)
    {
        let mut decoder = ZlibDecoder::new(compressed);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| DiscoverError::Decompress(format!("zlib: {e}")))?;
        return Ok(out);
    }
    Err(DiscoverError::Decompress(format!(
        "unknown compression magic: {:02x?}",
        &compressed[..compressed.len().min(4)]
    )))
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

    // Run with:
    //   RUST_LOG=info cargo test -p gulfwatch-ingest probe_onchain_idl -- --ignored --nocapture
    // Dumps the on-chain IDL account bytes for a handful of programs whose
    // accounts exist but fail our zstd decode. Optional: set PROBE_RPC_URL to
    // point at a private endpoint; defaults to api.mainnet-beta.solana.com.
    #[tokio::test]
    #[ignore]
    async fn probe_onchain_idl_layouts() {
        let rpc = std::env::var("PROBE_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

        let targets: &[(&str, &str)] = &[
            ("Jupiter V6", "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"),
            ("Raydium CLMM", "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK"),
            ("Raydium CPMM", "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"),
        ];

        for (label, pid) in targets {
            println!("\n=== {label} ({pid}) ===");
            let pid_bytes = decode_pubkey(pid).unwrap();
            let idl_addr = derive_idl_address(&pid_bytes);
            let idl_b58 = bs58::encode(idl_addr).into_string();
            println!("IDL PDA: {idl_b58}");

            match fetch_account_data(&rpc, &idl_b58).await {
                Ok(raw) => {
                    println!("account size: {} bytes", raw.len());
                    let show = raw.len().min(96);
                    println!("first {show} bytes (hex):");
                    for chunk in raw[..show].chunks(16) {
                        let hex: Vec<String> =
                            chunk.iter().map(|b| format!("{:02x}", b)).collect();
                        println!("  {}", hex.join(" "));
                    }
                    if raw.len() >= 44 {
                        let header_disc = &raw[0..8];
                        let authority = &raw[8..40];
                        let data_len =
                            u32::from_le_bytes(raw[40..44].try_into().unwrap()) as usize;
                        println!(
                            "  interpreted (legacy layout): disc={:02x?} auth_first4={:02x?} data_len(LE)={}",
                            header_disc,
                            &authority[..4],
                            data_len
                        );
                        if raw.len() >= 48 {
                            println!("  payload first 4: {:02x?}", &raw[44..48]);
                            println!("  (zstd magic is 28 b5 2f fd; ASCII '{{' is 7b)");
                        }
                    }
                }
                Err(e) => println!("fetch failed: {e}"),
            }
        }
    }

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
    fn decompress_dispatches_zstd_by_magic() {
        let original = b"{\"name\":\"legacy\",\"instructions\":[]}";
        let compressed = zstd::encode_all(&original[..], 3).unwrap();
        assert_eq!(&compressed[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
        let out = decompress_idl_payload(&compressed).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn decompress_dispatches_zlib_by_magic() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        let original = b"{\"metadata\":{\"name\":\"new\",\"spec\":\"0.1.0\"}}";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();
        assert_eq!(compressed[0], 0x78);
        let out = decompress_idl_payload(&compressed).unwrap();
        assert_eq!(out, original);
    }

    #[test]
    fn decompress_rejects_unknown_magic() {
        let junk = b"\xAA\xBB\xCC\xDDnot compressed";
        let err = decompress_idl_payload(junk).unwrap_err();
        assert!(matches!(err, DiscoverError::Decompress(_)));
        assert!(err.to_string().contains("unknown compression magic"));
    }

    // Run with:
    //   RUST_LOG=info cargo test -p gulfwatch-ingest fetch_onchain_idl_end_to_end_jupiter_v6 \
    //     -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn fetch_onchain_idl_end_to_end_jupiter_v6() {
        let rpc = std::env::var("PROBE_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
        let idl = fetch_onchain_idl(&rpc, "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4")
            .await
            .expect("Jupiter V6 IDL should fetch + decompress + parse end-to-end");
        println!(
            "jupiter v6: name={} format={:?} ix={} errors={}",
            idl.name,
            idl.format,
            idl.instructions.len(),
            idl.errors.len()
        );
        assert_eq!(idl.name, "jupiter");
        assert!(idl.instructions.len() >= 10);
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
        let idl = parse_idl_json(&decompressed).unwrap();
        assert_eq!(idl.name, "synth");
        assert_eq!(idl.instructions.len(), 1);
        assert_eq!(idl.instructions[0].name, "poke");
    }
}
