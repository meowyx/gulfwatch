use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

// Anchor instruction/account/event discriminator: sha256("<namespace>:<name>")[..8].
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdlFormat {
    Legacy,
    AnchorV030,
    Codama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlDocument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default)]
    pub instructions: Vec<IdlInstruction>,
    #[serde(default)]
    pub accounts: Vec<IdlTypeRef>,
    #[serde(default)]
    pub events: Vec<IdlTypeRef>,
    #[serde(default)]
    pub errors: Vec<IdlError>,
    #[serde(default)]
    pub types: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub format: IdlFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlInstruction {
    pub name: String,
    // Variable length: 8 bytes for Anchor, 1 byte for Shank/native enum tags,
    // arbitrary for Codama.
    pub discriminator: Vec<u8>,
    #[serde(default)]
    pub accounts: Vec<Value>,
    #[serde(default)]
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlTypeRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdlError {
    pub code: u32,
    pub name: String,
    #[serde(default)]
    pub msg: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IdlRegistryEntry {
    pub idl: IdlDocument,
    pub instruction_discriminators: HashMap<Vec<u8>, String>,
    // Sorted descending, deduped. Longest-prefix-first lookup so 8-byte Anchor
    // matches win over 1-byte Shank collisions.
    pub discriminator_lengths: Vec<usize>,
    pub errors_by_code: HashMap<u32, IdlError>,
}

impl IdlRegistryEntry {
    pub fn from_idl(idl: IdlDocument) -> Self {
        let instruction_discriminators: HashMap<Vec<u8>, String> = idl
            .instructions
            .iter()
            .map(|ix| (ix.discriminator.clone(), ix.name.clone()))
            .collect();
        let mut discriminator_lengths: Vec<usize> = instruction_discriminators
            .keys()
            .map(|k| k.len())
            .collect();
        discriminator_lengths.sort_by(|a, b| b.cmp(a));
        discriminator_lengths.dedup();
        let errors_by_code = idl.errors.iter().map(|e| (e.code, e.clone())).collect();
        Self {
            idl,
            instruction_discriminators,
            discriminator_lengths,
            errors_by_code,
        }
    }

    pub fn instruction_name_for(&self, data: &[u8]) -> Option<&str> {
        for &len in &self.discriminator_lengths {
            if data.len() < len {
                continue;
            }
            if let Some(name) = self.instruction_discriminators.get(&data[..len].to_vec()) {
                return Some(name.as_str());
            }
        }
        None
    }

    pub fn instruction_for(&self, data: &[u8]) -> Option<(&IdlInstruction, usize)> {
        for &len in &self.discriminator_lengths {
            if data.len() < len {
                continue;
            }
            let key = data[..len].to_vec();
            if let Some(name) = self.instruction_discriminators.get(&key) {
                if let Some(ix) = self.idl.instructions.iter().find(|ix| &ix.name == name) {
                    return Some((ix, len));
                }
            }
        }
        None
    }

    pub fn error_for_code(&self, code: u32) -> Option<&IdlError> {
        self.errors_by_code.get(&code)
    }
}

#[derive(Debug)]
pub enum IdlParseError {
    NotJson(serde_json::Error),
    UnknownFormat,
    MissingName,
    BadInstruction { index: usize, reason: String },
    BadAccount { index: usize, reason: String },
    BadEvent { index: usize, reason: String },
    BadError { index: usize, reason: String },
}

impl fmt::Display for IdlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdlParseError::NotJson(e) => write!(f, "not valid JSON: {e}"),
            IdlParseError::UnknownFormat => write!(
                f,
                "unrecognized IDL format: need root-level 'name' (legacy) or 'metadata.spec' (Anchor 0.30+ / Codama)"
            ),
            IdlParseError::MissingName => write!(f, "IDL has no program name"),
            IdlParseError::BadInstruction { index, reason } => {
                write!(f, "instruction #{index}: {reason}")
            }
            IdlParseError::BadAccount { index, reason } => write!(f, "account #{index}: {reason}"),
            IdlParseError::BadEvent { index, reason } => write!(f, "event #{index}: {reason}"),
            IdlParseError::BadError { index, reason } => {
                write!(f, "error entry #{index}: {reason}")
            }
        }
    }
}

impl std::error::Error for IdlParseError {}

pub fn detect_format(v: &Value) -> Option<IdlFormat> {
    if v.get("kind").and_then(|k| k.as_str()) == Some("rootNode")
        || v.get("standard").and_then(|s| s.as_str()) == Some("codama")
    {
        return Some(IdlFormat::Codama);
    }
    if v.get("metadata")
        .and_then(|m| m.get("spec"))
        .and_then(|s| s.as_str())
        .is_some()
    {
        return Some(IdlFormat::AnchorV030);
    }
    if v.get("name").and_then(|n| n.as_str()).is_some() {
        return Some(IdlFormat::Legacy);
    }
    None
}

pub fn parse_idl_json(bytes: &[u8]) -> Result<IdlDocument, IdlParseError> {
    let value: Value = serde_json::from_slice(bytes).map_err(IdlParseError::NotJson)?;
    parse_idl_value(value)
}

pub fn parse_idl_value(value: Value) -> Result<IdlDocument, IdlParseError> {
    match detect_format(&value) {
        Some(IdlFormat::AnchorV030) => parse_v030(value),
        Some(IdlFormat::Legacy) => parse_legacy(value),
        Some(IdlFormat::Codama) => parse_codama(value),
        None => Err(IdlParseError::UnknownFormat),
    }
}

fn parse_legacy(mut value: Value) -> Result<IdlDocument, IdlParseError> {
    let obj = value.as_object_mut().ok_or(IdlParseError::UnknownFormat)?;
    let name = obj
        .remove("name")
        .and_then(|n| n.as_str().map(|s| s.to_string()))
        .ok_or(IdlParseError::MissingName)?;
    let version = obj
        .remove("version")
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let instructions = parse_legacy_instructions(obj.remove("instructions").unwrap_or_default())?;
    let accounts = parse_type_refs(obj.remove("accounts").unwrap_or_default(), false)?;
    let events = parse_type_refs(obj.remove("events").unwrap_or_default(), false)?;
    let errors = parse_errors(obj.remove("errors").unwrap_or_default())?;
    let types = match obj.remove("types") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    let metadata = obj.remove("metadata");
    let address = metadata
        .as_ref()
        .and_then(|m| m.get("address"))
        .and_then(|a| a.as_str())
        .map(|s| s.to_string());

    Ok(IdlDocument {
        name,
        version,
        address,
        instructions,
        accounts,
        events,
        errors,
        types,
        metadata,
        format: IdlFormat::Legacy,
    })
}

fn parse_v030(mut value: Value) -> Result<IdlDocument, IdlParseError> {
    let obj = value.as_object_mut().ok_or(IdlParseError::UnknownFormat)?;
    let address = obj
        .remove("address")
        .and_then(|a| a.as_str().map(|s| s.to_string()));
    let metadata = obj.remove("metadata");
    let (name, version) = match metadata.as_ref() {
        Some(Value::Object(m)) => {
            let n = m
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or(IdlParseError::MissingName)?
                .to_string();
            let v = m
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (n, v)
        }
        _ => return Err(IdlParseError::MissingName),
    };

    let instructions = parse_v030_instructions(obj.remove("instructions").unwrap_or_default())?;
    let accounts = parse_type_refs(obj.remove("accounts").unwrap_or_default(), true)?;
    let events = parse_type_refs(obj.remove("events").unwrap_or_default(), true)?;
    let errors = parse_errors(obj.remove("errors").unwrap_or_default())?;
    let types = match obj.remove("types") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };

    Ok(IdlDocument {
        name,
        version,
        address,
        instructions,
        accounts,
        events,
        errors,
        types,
        metadata,
        format: IdlFormat::AnchorV030,
    })
}

fn parse_codama(mut value: Value) -> Result<IdlDocument, IdlParseError> {
    let obj = value.as_object_mut().ok_or(IdlParseError::UnknownFormat)?;
    let program_val = obj
        .remove("program")
        .ok_or(IdlParseError::UnknownFormat)?;
    let mut program = match program_val {
        Value::Object(m) => m,
        _ => return Err(IdlParseError::UnknownFormat),
    };

    let name = program
        .remove("name")
        .and_then(|n| n.as_str().map(|s| s.to_string()))
        .ok_or(IdlParseError::MissingName)?;
    let address = program
        .remove("publicKey")
        .and_then(|a| a.as_str().map(|s| s.to_string()));
    let version = program
        .remove("version")
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let instructions = parse_codama_instructions(program.remove("instructions").unwrap_or_default())?;
    let accounts = parse_codama_type_refs(program.remove("accounts").unwrap_or_default());
    let errors = parse_codama_errors(program.remove("errors").unwrap_or_default())?;

    Ok(IdlDocument {
        name,
        version,
        address,
        instructions,
        accounts,
        events: Vec::new(),
        errors,
        types: Vec::new(),
        metadata: None,
        format: IdlFormat::Codama,
    })
}

fn parse_codama_instructions(v: Value) -> Result<Vec<IdlInstruction>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let mut obj = match item {
            Value::Object(m) => m,
            _ => continue,
        };
        let name = obj
            .remove("name")
            .and_then(|n| n.as_str().map(|s| s.to_string()))
            .ok_or_else(|| IdlParseError::BadInstruction {
                index: i,
                reason: "missing name".into(),
            })?;

        let discriminators = obj.remove("discriminators").unwrap_or_default();
        let arguments = obj.remove("arguments").unwrap_or(Value::Array(Vec::new()));
        let discriminator = extract_codama_discriminator(&name, &discriminators, &arguments)
            .map_err(|r| IdlParseError::BadInstruction {
                index: i,
                reason: format!("instruction '{name}' {r}"),
            })?;

        let args = match arguments {
            Value::Array(a) => a,
            _ => Vec::new(),
        };

        out.push(IdlInstruction {
            name,
            discriminator,
            accounts: Vec::new(),
            args,
        });
    }
    Ok(out)
}

// Supports fieldDiscriminatorNode at contiguous offsets starting at 0, each
// referencing a numberTypeNode or bytesTypeNode argument. Multi-node
// discriminators (e.g. Token-2022's extension-byte + sub-instruction byte)
// concatenate in order.
fn extract_codama_discriminator(
    _ix_name: &str,
    discriminators: &Value,
    arguments: &Value,
) -> Result<Vec<u8>, String> {
    let disc_array = match discriminators {
        Value::Array(a) if !a.is_empty() => a,
        _ => return Err("has no discriminators".into()),
    };

    let args_by_name: HashMap<&str, &Value> = match arguments {
        Value::Array(a) => a
            .iter()
            .filter_map(|arg| {
                let obj = arg.as_object()?;
                let name = obj.get("name").and_then(|n| n.as_str())?;
                Some((name, arg))
            })
            .collect(),
        _ => HashMap::new(),
    };

    let mut bytes = Vec::new();
    for (idx, node) in disc_array.iter().enumerate() {
        let obj = node
            .as_object()
            .ok_or_else(|| format!("discriminator[{idx}] is not an object"))?;
        let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if kind != "fieldDiscriminatorNode" {
            return Err(format!(
                "discriminator[{idx}] kind '{kind}' not supported (only fieldDiscriminatorNode)"
            ));
        }
        let offset = obj.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
        if offset != bytes.len() {
            return Err(format!(
                "discriminator[{idx}] offset {offset} doesn't continue from byte {} (gap or overlap not supported)",
                bytes.len()
            ));
        }
        let field_name = obj
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("discriminator[{idx}] missing 'name'"))?;
        let arg = args_by_name
            .get(field_name)
            .ok_or_else(|| format!("discriminator[{idx}] references unknown argument '{field_name}'"))?;
        encode_codama_default_value(arg, &mut bytes)
            .map_err(|r| format!("discriminator[{idx}] '{field_name}': {r}"))?;
    }

    if bytes.is_empty() {
        return Err("produced zero-length discriminator".into());
    }
    Ok(bytes)
}

fn encode_codama_default_value(arg: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    let obj = arg.as_object().ok_or("argument is not an object")?;
    let default = obj
        .get("defaultValue")
        .ok_or("missing defaultValue (not a constant discriminator)")?;
    let default_obj = default
        .as_object()
        .ok_or("defaultValue is not an object")?;
    let value_kind = default_obj
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("");

    let type_obj = obj
        .get("type")
        .and_then(|t| t.as_object())
        .ok_or("argument has no type")?;
    let type_kind = type_obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");

    match (value_kind, type_kind) {
        ("numberValueNode", "numberTypeNode") => {
            let number = default_obj
                .get("number")
                .and_then(|n| n.as_u64())
                .ok_or("numberValueNode has no numeric 'number'")?;
            let format = type_obj
                .get("format")
                .and_then(|f| f.as_str())
                .unwrap_or("u8");
            // All numeric IDL discriminators are little-endian on Solana.
            match format {
                "u8" => {
                    if number > u8::MAX as u64 {
                        return Err(format!("u8 discriminator {number} out of range"));
                    }
                    out.push(number as u8);
                }
                "u16" => {
                    if number > u16::MAX as u64 {
                        return Err(format!("u16 discriminator {number} out of range"));
                    }
                    out.extend_from_slice(&(number as u16).to_le_bytes());
                }
                "u32" => {
                    if number > u32::MAX as u64 {
                        return Err(format!("u32 discriminator {number} out of range"));
                    }
                    out.extend_from_slice(&(number as u32).to_le_bytes());
                }
                "u64" => {
                    out.extend_from_slice(&number.to_le_bytes());
                }
                other => return Err(format!("unsupported numberTypeNode format '{other}'")),
            }
            Ok(())
        }
        ("bytesValueNode", "bytesTypeNode") => {
            let data = default_obj
                .get("data")
                .and_then(|d| d.as_str())
                .ok_or("bytesValueNode missing string 'data'")?;
            let encoding = default_obj
                .get("encoding")
                .and_then(|e| e.as_str())
                .unwrap_or("base16");
            match encoding {
                "base16" | "hex" => decode_base16(data, out),
                other => Err(format!("unsupported bytesValueNode encoding '{other}'")),
            }
        }
        (vk, tk) => Err(format!(
            "unsupported discriminator shape: value={vk}, type={tk}"
        )),
    }
}

fn decode_base16(s: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return Err(format!("base16 payload has odd length {}", s.len()));
    }
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(())
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("not a hex digit: {}", b as char)),
    }
}

fn parse_codama_type_refs(v: Value) -> Vec<IdlTypeRef> {
    let arr = match v {
        Value::Array(a) => a,
        _ => return Vec::new(),
    };
    arr.into_iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let name = obj.get("name").and_then(|n| n.as_str())?.to_string();
            Some(IdlTypeRef {
                name,
                discriminator: None,
            })
        })
        .collect()
}

fn parse_codama_errors(v: Value) -> Result<Vec<IdlError>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let obj = match item {
            Value::Object(m) => m,
            _ => continue,
        };
        let code = obj
            .get("code")
            .and_then(|c| c.as_u64())
            .ok_or_else(|| IdlParseError::BadError {
                index: i,
                reason: "missing or invalid code".into(),
            })? as u32;
        let name = obj
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| IdlParseError::BadError {
                index: i,
                reason: "missing name".into(),
            })?
            .to_string();
        let msg = obj
            .get("message")
            .or_else(|| obj.get("msg"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        out.push(IdlError { code, name, msg });
    }
    Ok(out)
}

fn parse_legacy_instructions(v: Value) -> Result<Vec<IdlInstruction>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => return Ok(Vec::new()),
        _ => {
            return Err(IdlParseError::BadInstruction {
                index: 0,
                reason: "instructions must be an array".into(),
            })
        }
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let mut obj = match item {
            Value::Object(m) => m,
            _ => {
                return Err(IdlParseError::BadInstruction {
                    index: i,
                    reason: "expected object".into(),
                })
            }
        };
        let name = obj
            .remove("name")
            .and_then(|n| n.as_str().map(|s| s.to_string()))
            .ok_or_else(|| IdlParseError::BadInstruction {
                index: i,
                reason: "missing name".into(),
            })?;
        let discriminator = derive_instruction_discriminator(&name).to_vec();
        let accounts = match obj.remove("accounts") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        let args = match obj.remove("args") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        out.push(IdlInstruction {
            name,
            discriminator,
            accounts,
            args,
        });
    }
    Ok(out)
}

fn parse_v030_instructions(v: Value) -> Result<Vec<IdlInstruction>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => return Ok(Vec::new()),
        _ => {
            return Err(IdlParseError::BadInstruction {
                index: 0,
                reason: "instructions must be an array".into(),
            })
        }
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let mut obj = match item {
            Value::Object(m) => m,
            _ => {
                return Err(IdlParseError::BadInstruction {
                    index: i,
                    reason: "expected object".into(),
                })
            }
        };
        let name = obj
            .remove("name")
            .and_then(|n| n.as_str().map(|s| s.to_string()))
            .ok_or_else(|| IdlParseError::BadInstruction {
                index: i,
                reason: "missing name".into(),
            })?;
        let disc_val = obj
            .remove("discriminator")
            .ok_or_else(|| IdlParseError::BadInstruction {
                index: i,
                reason: format!("instruction '{name}' missing discriminator"),
            })?;
        let discriminator = parse_disc_array(disc_val).map_err(|r| IdlParseError::BadInstruction {
            index: i,
            reason: format!("instruction '{name}' has {r}"),
        })?;
        let accounts = match obj.remove("accounts") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        let args = match obj.remove("args") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        out.push(IdlInstruction {
            name,
            discriminator,
            accounts,
            args,
        });
    }
    Ok(out)
}

fn parse_type_refs(v: Value, discriminator_required: bool) -> Result<Vec<IdlTypeRef>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => return Ok(Vec::new()),
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let mut obj = match item {
            Value::Object(m) => m,
            _ => {
                return Err(IdlParseError::BadAccount {
                    index: i,
                    reason: "expected object".into(),
                })
            }
        };
        let name = match obj.remove("name").and_then(|n| n.as_str().map(|s| s.to_string())) {
            Some(n) => n,
            None => continue,
        };
        let discriminator = match obj.remove("discriminator") {
            Some(d) => Some(parse_disc_array(d).map_err(|r| IdlParseError::BadAccount {
                index: i,
                reason: r,
            })?),
            None if discriminator_required => None,
            None => None,
        };
        out.push(IdlTypeRef {
            name,
            discriminator,
        });
    }
    Ok(out)
}

fn parse_errors(v: Value) -> Result<Vec<IdlError>, IdlParseError> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => return Ok(Vec::new()),
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.into_iter().enumerate() {
        let obj = match item {
            Value::Object(m) => m,
            _ => {
                return Err(IdlParseError::BadError {
                    index: i,
                    reason: "expected object".into(),
                })
            }
        };
        let code = obj
            .get("code")
            .and_then(|c| c.as_u64())
            .ok_or_else(|| IdlParseError::BadError {
                index: i,
                reason: "missing or invalid code".into(),
            })? as u32;
        let name = obj
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| IdlParseError::BadError {
                index: i,
                reason: "missing name".into(),
            })?
            .to_string();
        let msg = obj
            .get("msg")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string());
        out.push(IdlError { code, name, msg });
    }
    Ok(out)
}

// v030 IDLs always store 8-byte discriminator arrays. Codama discriminators go
// through extract_codama_discriminator instead.
fn parse_disc_array(v: Value) -> Result<Vec<u8>, String> {
    let arr = match v {
        Value::Array(a) => a,
        _ => return Err("discriminator must be an array".into()),
    };
    if arr.len() != 8 {
        return Err(format!("discriminator must be 8 bytes, got {}", arr.len()));
    }
    let mut out = Vec::with_capacity(8);
    for (i, b) in arr.into_iter().enumerate() {
        let n = b
            .as_u64()
            .ok_or_else(|| format!("discriminator byte {i} is not a number"))?;
        if n > 255 {
            return Err(format!("discriminator byte {i} = {n} overflows u8"));
        }
        out.push(n as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_instruction_discriminator_matches_anchor_formula() {
        // Known-good bytes from Anchor codegen for `sha256("global:initialize")[..8]`.
        // If this ever changes, every legacy decoder lookup is silently wrong.
        let got = derive_instruction_discriminator("initialize");
        let expected: [u8; 8] = [175, 175, 109, 31, 13, 152, 155, 237];
        assert_eq!(got, expected);
    }

    #[test]
    fn derive_discriminator_uses_colon_separator() {
        assert_ne!(
            derive_discriminator("global", "swap"),
            derive_discriminator("account", "swap")
        );
    }

    #[test]
    fn detect_format_spots_v030_by_metadata_spec() {
        let v: Value = serde_json::from_str(r#"{"metadata":{"name":"x","spec":"0.1.0"}}"#).unwrap();
        assert_eq!(detect_format(&v), Some(IdlFormat::AnchorV030));
    }

    #[test]
    fn detect_format_falls_back_to_legacy_root_name() {
        let v: Value = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(detect_format(&v), Some(IdlFormat::Legacy));
    }

    #[test]
    fn detect_format_rejects_unrecognized_shape() {
        let v: Value = serde_json::from_str(r#"{"metadata":{}}"#).unwrap();
        assert_eq!(detect_format(&v), None);
    }

    #[test]
    fn parse_legacy_minimal() {
        let doc = parse_idl_json(br#"{"name":"jupiter","instructions":[{"name":"swap"}]}"#).unwrap();
        assert_eq!(doc.name, "jupiter");
        assert_eq!(doc.format, IdlFormat::Legacy);
        assert_eq!(doc.instructions.len(), 1);
        assert_eq!(doc.instructions[0].name, "swap");
        assert_eq!(
            doc.instructions[0].discriminator,
            derive_instruction_discriminator("swap")
        );
    }

    #[test]
    fn parse_legacy_realistic() {
        let json = br#"{
          "version": "0.1.0",
          "name": "my_program",
          "instructions": [
            { "name": "initialize", "accounts": [{"name":"authority","isMut":true}], "args": [{"name":"amount","type":"u64"}] }
          ],
          "errors": [
            {"code":6000,"name":"SlippageExceeded","msg":"Slippage tolerance exceeded"},
            {"code":6001,"name":"InsufficientFunds"}
          ]
        }"#;
        let doc = parse_idl_json(json).unwrap();
        assert_eq!(doc.version.as_deref(), Some("0.1.0"));
        assert_eq!(doc.errors.len(), 2);
        assert_eq!(doc.errors[0].msg.as_deref(), Some("Slippage tolerance exceeded"));
        assert_eq!(doc.errors[1].msg, None);
    }

    #[test]
    fn parse_legacy_missing_name_errors() {
        let err = parse_idl_json(br#"{"instructions":[]}"#).unwrap_err();
        // Unknown format because root has no `name` and no `metadata.spec`.
        assert!(matches!(err, IdlParseError::UnknownFormat));
    }

    #[test]
    fn parse_v030_minimal() {
        let json = br#"{
          "address":"Prog",
          "metadata":{"name":"x","version":"0.1.0","spec":"0.1.0"},
          "instructions":[{"name":"swap","discriminator":[1,2,3,4,5,6,7,8]}]
        }"#;
        let doc = parse_idl_json(json).unwrap();
        assert_eq!(doc.name, "x");
        assert_eq!(doc.format, IdlFormat::AnchorV030);
        assert_eq!(doc.address.as_deref(), Some("Prog"));
        assert_eq!(doc.instructions[0].discriminator, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn parse_v030_missing_discriminator_is_named_in_error() {
        let json = br#"{
          "metadata":{"name":"x","spec":"0.1.0"},
          "instructions":[{"name":"route"}]
        }"#;
        let err = parse_idl_json(json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("route"), "error should name the bad instruction: {msg}");
        assert!(msg.contains("discriminator"), "error should say what's missing: {msg}");
    }

    #[test]
    fn parse_v030_wrong_discriminator_length_errors() {
        let json = br#"{
          "metadata":{"name":"x","spec":"0.1.0"},
          "instructions":[{"name":"swap","discriminator":[1,2,3]}]
        }"#;
        let err = parse_idl_json(json).unwrap_err();
        assert!(err.to_string().contains("8 bytes"));
    }

    #[test]
    fn detect_format_spots_codama_by_kind_rootnode() {
        let v: Value = serde_json::from_str(r#"{"kind":"rootNode","program":{"name":"x"}}"#).unwrap();
        assert_eq!(detect_format(&v), Some(IdlFormat::Codama));
    }

    #[test]
    fn detect_format_spots_codama_by_standard_field() {
        let v: Value =
            serde_json::from_str(r#"{"standard":"codama","program":{"name":"x"}}"#).unwrap();
        assert_eq!(detect_format(&v), Some(IdlFormat::Codama));
    }

    #[test]
    fn parse_codama_shank_u8_discriminator() {
        let json = br#"{
          "kind": "rootNode",
          "standard": "codama",
          "program": {
            "kind": "programNode",
            "name": "token-2022",
            "publicKey": "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
            "version": "3.0.2",
            "instructions": [
              {
                "kind": "instructionNode",
                "name": "initializeMint",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "discriminator", "offset": 0 }
                ],
                "arguments": [
                  {
                    "kind": "instructionArgumentNode",
                    "name": "discriminator",
                    "type": { "kind": "numberTypeNode", "format": "u8", "endian": "le" },
                    "defaultValue": { "kind": "numberValueNode", "number": 0 }
                  },
                  { "name": "decimals", "type": { "kind": "numberTypeNode", "format": "u8" } }
                ]
              },
              {
                "kind": "instructionNode",
                "name": "initializeAccount",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "discriminator", "offset": 0 }
                ],
                "arguments": [
                  {
                    "name": "discriminator",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 1 }
                  }
                ]
              }
            ]
          }
        }"#;
        let doc = parse_idl_json(json).unwrap();
        assert_eq!(doc.format, IdlFormat::Codama);
        assert_eq!(doc.name, "token-2022");
        assert_eq!(
            doc.address.as_deref(),
            Some("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
        );
        assert_eq!(doc.instructions.len(), 2);
        assert_eq!(doc.instructions[0].name, "initializeMint");
        assert_eq!(doc.instructions[0].discriminator, vec![0u8]);
        assert_eq!(doc.instructions[1].discriminator, vec![1u8]);
    }

    #[test]
    fn parse_codama_u16_and_u32_discriminators_are_little_endian() {
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "wide",
            "publicKey": "X",
            "instructions": [
              {
                "name": "u16Ix",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "d", "offset": 0 }
                ],
                "arguments": [
                  { "name": "d",
                    "type": { "kind": "numberTypeNode", "format": "u16" },
                    "defaultValue": { "kind": "numberValueNode", "number": 258 } }
                ]
              },
              {
                "name": "u32Ix",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "d", "offset": 0 }
                ],
                "arguments": [
                  { "name": "d",
                    "type": { "kind": "numberTypeNode", "format": "u32" },
                    "defaultValue": { "kind": "numberValueNode", "number": 4294967295 } }
                ]
              }
            ]
          }
        }"#;
        let doc = parse_idl_json(json).unwrap();
        // 258 as LE u16 = [0x02, 0x01]
        assert_eq!(doc.instructions[0].discriminator, vec![0x02, 0x01]);
        // u32::MAX as LE = [0xff; 4]
        assert_eq!(doc.instructions[1].discriminator, vec![0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn parse_codama_errors_map_message_field() {
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "p",
            "publicKey": "X",
            "errors": [
              { "kind": "errorNode", "code": 0, "name": "NotEnoughFunds",
                "message": "Lamport balance below rent-exempt threshold" },
              { "kind": "errorNode", "code": 1, "name": "InsufficientLamports" }
            ]
          }
        }"#;
        let doc = parse_idl_json(json).unwrap();
        assert_eq!(doc.errors.len(), 2);
        assert_eq!(doc.errors[0].name, "NotEnoughFunds");
        assert_eq!(
            doc.errors[0].msg.as_deref(),
            Some("Lamport balance below rent-exempt threshold")
        );
        assert_eq!(doc.errors[1].msg, None);
    }

    #[test]
    fn parse_codama_rejects_non_zero_offset_on_first_discriminator() {
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "p",
            "publicKey": "X",
            "instructions": [
              { "name": "ix",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "d", "offset": 8 }
                ],
                "arguments": [
                  { "name": "d",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 0 } }
                ] }
            ]
          }
        }"#;
        let err = parse_idl_json(json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("offset 8"), "should name the offset: {msg}");
        assert!(
            msg.contains("byte 0"),
            "should say we expected byte 0: {msg}"
        );
    }

    #[test]
    fn parse_codama_concatenates_consecutive_discriminator_nodes() {
        // Token-2022 extension pattern: byte 0 = extension type (e.g. 26),
        // byte 1 = sub-instruction within that extension.
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "t22",
            "publicKey": "X",
            "instructions": [
              { "name": "initializeTransferFeeConfig",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "ext", "offset": 0 },
                  { "kind": "fieldDiscriminatorNode", "name": "sub", "offset": 1 }
                ],
                "arguments": [
                  { "name": "ext",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 26 } },
                  { "name": "sub",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 0 } }
                ] }
            ]
          }
        }"#;
        let doc = parse_idl_json(json).unwrap();
        assert_eq!(doc.instructions[0].discriminator, vec![26u8, 0u8]);
    }

    #[test]
    fn parse_codama_rejects_gap_between_discriminator_nodes() {
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "p",
            "publicKey": "X",
            "instructions": [
              { "name": "ix",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "a", "offset": 0 },
                  { "kind": "fieldDiscriminatorNode", "name": "b", "offset": 3 }
                ],
                "arguments": [
                  { "name": "a",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 1 } },
                  { "name": "b",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 2 } }
                ] }
            ]
          }
        }"#;
        let err = parse_idl_json(json).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("offset 3"), "should name the bad offset: {msg}");
        assert!(msg.contains("byte 1"), "should say we expected byte 1: {msg}");
    }

    #[test]
    fn codama_registry_entry_resolves_shank_u8_dispatch_from_full_ix_data() {
        // End-to-end: Codama IDL → IdlRegistryEntry → instruction_name_for
        // with realistic 8-byte-captured instruction data (byte 0 is the tag,
        // the rest is arg data). instruction_name_for tries length 1 after
        // no 8-byte prefix matches.
        let json = br#"{
          "kind": "rootNode",
          "program": {
            "name": "tok",
            "publicKey": "X",
            "instructions": [
              { "name": "initializeMint",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "d", "offset": 0 }
                ],
                "arguments": [
                  { "name": "d",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 0 } }
                ] },
              { "name": "transfer",
                "discriminators": [
                  { "kind": "fieldDiscriminatorNode", "name": "d", "offset": 0 }
                ],
                "arguments": [
                  { "name": "d",
                    "type": { "kind": "numberTypeNode", "format": "u8" },
                    "defaultValue": { "kind": "numberValueNode", "number": 3 } }
                ] }
            ]
          }
        }"#;
        let doc = parse_idl_json(json).unwrap();
        let entry = IdlRegistryEntry::from_idl(doc);
        assert_eq!(entry.discriminator_lengths, vec![1]);

        // Simulate a transfer ix where byte 0 is 3, followed by 7 bytes of args
        let data = [3u8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];
        assert_eq!(entry.instruction_name_for(&data), Some("transfer"));

        let data_init = [0u8; 8];
        assert_eq!(entry.instruction_name_for(&data_init), Some("initializeMint"));

        let data_unknown = [99u8; 8];
        assert_eq!(entry.instruction_name_for(&data_unknown), None);
    }

    #[test]
    fn instruction_name_for_resolves_by_first_eight_bytes() {
        let doc = parse_idl_json(br#"{"name":"prog","instructions":[{"name":"swap"}]}"#).unwrap();
        let entry = IdlRegistryEntry::from_idl(doc);
        let disc = derive_instruction_discriminator("swap");
        let mut data = disc.to_vec();
        data.extend_from_slice(&[0xAA; 32]);
        assert_eq!(entry.instruction_name_for(&data), Some("swap"));
    }

    #[test]
    fn instruction_name_for_returns_none_on_unknown_discriminator() {
        let doc = parse_idl_json(br#"{"name":"prog","instructions":[{"name":"swap"}]}"#).unwrap();
        let entry = IdlRegistryEntry::from_idl(doc);
        assert_eq!(entry.instruction_name_for(&[0u8; 16]), None);
    }

    #[test]
    fn instruction_name_for_returns_none_on_short_data() {
        let doc = parse_idl_json(br#"{"name":"prog","instructions":[{"name":"swap"}]}"#).unwrap();
        let entry = IdlRegistryEntry::from_idl(doc);
        assert_eq!(entry.instruction_name_for(&[0u8; 4]), None);
    }

    #[test]
    fn errors_by_code_table_is_built() {
        let doc = parse_idl_json(
            br#"{"name":"prog","errors":[{"code":6000,"name":"A","msg":"msg"},{"code":6001,"name":"B"}]}"#,
        )
        .unwrap();
        let entry = IdlRegistryEntry::from_idl(doc);
        assert_eq!(entry.error_for_code(6000).unwrap().name, "A");
        assert_eq!(entry.error_for_code(6000).unwrap().msg.as_deref(), Some("msg"));
        assert_eq!(entry.error_for_code(6001).unwrap().msg, None);
        assert!(entry.error_for_code(9999).is_none());
    }

    #[test]
    fn not_json_input_surfaces_serde_error() {
        let err = parse_idl_json(b"not-json-at-all").unwrap_err();
        assert!(matches!(err, IdlParseError::NotJson(_)));
    }
}
