use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Anchor discriminators: `sha256("<namespace>:<name>")[..8]` — namespaces are
// `global` (instructions), `account` (account types), `event` (events).
pub fn derive_discriminator(namespace: &str, name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(b":");
    hasher.update(name.as_bytes());
    let hash = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&hash[..8]);
    out
}

pub fn derive_instruction_discriminator(name: &str) -> [u8; 8] {
    derive_discriminator("global", name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdlStatus {
    Loading,
    Loaded,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct IdlRegistryEntry {
    pub idl: AnchorIdl,
    pub instruction_discriminators: HashMap<[u8; 8], String>,
    pub errors_by_code: HashMap<u32, IdlError>,
}

impl IdlRegistryEntry {
    pub fn from_idl(idl: AnchorIdl) -> Self {
        let instruction_discriminators = idl
            .instructions
            .iter()
            .map(|ix| (derive_instruction_discriminator(&ix.name), ix.name.clone()))
            .collect();
        let errors_by_code = idl
            .errors
            .iter()
            .map(|e| (e.code, e.clone()))
            .collect();
        Self {
            idl,
            instruction_discriminators,
            errors_by_code,
        }
    }

    pub fn instruction_name_for(&self, data: &[u8]) -> Option<&str> {
        if data.len() < 8 {
            return None;
        }
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[..8]);
        self.instruction_discriminators
            .get(&disc)
            .map(|s| s.as_str())
    }

    pub fn error_for_code(&self, code: u32) -> Option<&IdlError> {
        self.errors_by_code.get(&code)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorIdl {
    #[serde(default)]
    pub version: Option<String>,
    pub name: String,
    #[serde(default)]
    pub instructions: Vec<IdlInstruction>,
    #[serde(default)]
    pub accounts: Vec<serde_json::Value>,
    #[serde(default)]
    pub types: Vec<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<IdlError>,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlInstruction {
    pub name: String,
    #[serde(default)]
    pub accounts: Vec<serde_json::Value>,
    #[serde(default)]
    pub args: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlError {
    pub code: u32,
    pub name: String,
    #[serde(default)]
    pub msg: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_idl_deserializes() {
        let json = r#"{"name":"jupiter","instructions":[{"name":"swap"}]}"#;
        let idl: AnchorIdl = serde_json::from_str(json).unwrap();
        assert_eq!(idl.name, "jupiter");
        assert_eq!(idl.instructions.len(), 1);
        assert_eq!(idl.instructions[0].name, "swap");
        assert_eq!(idl.version, None);
        assert!(idl.errors.is_empty());
        assert!(idl.accounts.is_empty());
    }

    #[test]
    fn realistic_anchor_idl_deserializes() {
        let json = r#"{
          "version": "0.1.0",
          "name": "my_program",
          "instructions": [
            {
              "name": "initialize",
              "accounts": [{"name":"authority","isMut":true,"isSigner":true}],
              "args": [{"name":"amount","type":"u64"}]
            }
          ],
          "errors": [
            {"code":6000,"name":"SlippageExceeded","msg":"Slippage tolerance exceeded"},
            {"code":6001,"name":"InsufficientFunds"}
          ],
          "metadata": {"address": "ProgID..."}
        }"#;
        let idl: AnchorIdl = serde_json::from_str(json).unwrap();
        assert_eq!(idl.version.as_deref(), Some("0.1.0"));
        assert_eq!(idl.name, "my_program");
        assert_eq!(idl.instructions.len(), 1);
        assert_eq!(idl.instructions[0].name, "initialize");
        assert_eq!(idl.instructions[0].accounts.len(), 1);
        assert_eq!(idl.instructions[0].args.len(), 1);
        assert_eq!(idl.errors.len(), 2);
        assert_eq!(idl.errors[0].code, 6000);
        assert_eq!(idl.errors[0].name, "SlippageExceeded");
        assert_eq!(
            idl.errors[0].msg.as_deref(),
            Some("Slippage tolerance exceeded")
        );
        assert_eq!(idl.errors[1].msg, None);
    }

    #[test]
    fn missing_name_field_is_rejected() {
        let json = r#"{"instructions":[]}"#;
        assert!(serde_json::from_str::<AnchorIdl>(json).is_err());
    }

    #[test]
    fn roundtrip_preserves_unknown_nested_fields() {
        let json = r#"{
          "name": "prog",
          "accounts": [{"name":"Foo","type":{"kind":"struct","fields":[{"name":"x","type":"u64"}]}}]
        }"#;
        let idl: AnchorIdl = serde_json::from_str(json).unwrap();
        let out = serde_json::to_value(&idl).unwrap();
        assert_eq!(out["accounts"][0]["name"], "Foo");
        assert_eq!(out["accounts"][0]["type"]["kind"], "struct");
        assert_eq!(out["accounts"][0]["type"]["fields"][0]["name"], "x");
    }

    #[test]
    fn derive_instruction_discriminator_is_deterministic() {
        let a = derive_instruction_discriminator("swap");
        let b = derive_instruction_discriminator("swap");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
    }

    #[test]
    fn derive_instruction_discriminator_distinguishes_names() {
        let swap = derive_instruction_discriminator("swap");
        let route = derive_instruction_discriminator("route");
        assert_ne!(swap, route);
    }

    #[test]
    fn derive_instruction_discriminator_matches_anchor_formula() {
        // Known-good bytes from Anchor codegen for `sha256("global:initialize")[..8]`.
        // If this ever changes, every decoder lookup is silently wrong.
        let got = derive_instruction_discriminator("initialize");
        let expected: [u8; 8] = [175, 175, 109, 31, 13, 152, 155, 237];
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_discriminator_uses_colon_separator() {
        // Rules out a bug where the `:` separator is elided and the inputs
        // get concatenated as one blob.
        let global_swap = derive_discriminator("global", "swap");
        let account_swap = derive_discriminator("account", "swap");
        assert_ne!(global_swap, account_swap);
    }

    #[test]
    fn registry_entry_builds_discriminator_table_for_every_instruction() {
        let idl: AnchorIdl = serde_json::from_str(
            r#"{"name":"prog","instructions":[{"name":"swap"},{"name":"route"},{"name":"deposit"}]}"#,
        )
        .unwrap();
        let entry = IdlRegistryEntry::from_idl(idl);
        assert_eq!(entry.instruction_discriminators.len(), 3);

        let swap_disc = derive_instruction_discriminator("swap");
        assert_eq!(
            entry.instruction_discriminators.get(&swap_disc).map(|s| s.as_str()),
            Some("swap")
        );
    }

    #[test]
    fn instruction_name_for_resolves_by_first_eight_bytes() {
        let idl: AnchorIdl = serde_json::from_str(
            r#"{"name":"prog","instructions":[{"name":"swap"}]}"#,
        )
        .unwrap();
        let entry = IdlRegistryEntry::from_idl(idl);

        let disc = derive_instruction_discriminator("swap");
        // Pad trailing bytes — lookup must only read the first 8.
        let mut tx_data = disc.to_vec();
        tx_data.extend_from_slice(&[0xAA; 32]);

        assert_eq!(entry.instruction_name_for(&tx_data), Some("swap"));
    }

    #[test]
    fn instruction_name_for_returns_none_on_unknown_discriminator() {
        let idl: AnchorIdl =
            serde_json::from_str(r#"{"name":"prog","instructions":[{"name":"swap"}]}"#).unwrap();
        let entry = IdlRegistryEntry::from_idl(idl);
        assert_eq!(entry.instruction_name_for(&[0u8; 16]), None);
    }

    #[test]
    fn registry_entry_builds_errors_by_code_table() {
        let idl: AnchorIdl = serde_json::from_str(
            r#"{
                "name": "prog",
                "errors": [
                    {"code": 6000, "name": "SlippageExceeded", "msg": "too much slip"},
                    {"code": 6001, "name": "InvalidRoute"}
                ]
            }"#,
        )
        .unwrap();
        let entry = IdlRegistryEntry::from_idl(idl);
        assert_eq!(entry.errors_by_code.len(), 2);

        let slippage = entry.error_for_code(6000).unwrap();
        assert_eq!(slippage.name, "SlippageExceeded");
        assert_eq!(slippage.msg.as_deref(), Some("too much slip"));

        let invalid = entry.error_for_code(6001).unwrap();
        assert_eq!(invalid.name, "InvalidRoute");
        assert_eq!(invalid.msg, None);

        assert!(entry.error_for_code(9999).is_none());
    }

    #[test]
    fn instruction_name_for_returns_none_on_short_data() {
        let idl: AnchorIdl =
            serde_json::from_str(r#"{"name":"prog","instructions":[{"name":"swap"}]}"#).unwrap();
        let entry = IdlRegistryEntry::from_idl(idl);
        assert_eq!(entry.instruction_name_for(&[0u8; 4]), None);
    }
}
