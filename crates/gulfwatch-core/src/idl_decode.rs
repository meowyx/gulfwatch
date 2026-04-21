// Scope: scalar types (uN/iN, bool, pubkey, len-prefixed string/bytes) in
// either plain-string or Codama typeNode form. Composite types halt decoding
// with an "unsupported type" placeholder so the caller keeps the prefix.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedArg {
    pub name: String,
    pub type_label: String,
    pub value_display: String,
    pub byte_offset: usize,
    pub byte_length: Option<usize>,
}

pub fn decode_args(
    args: &[Value],
    data: &[u8],
    discriminator_len: usize,
) -> Vec<DecodedArg> {
    let mut out = Vec::with_capacity(args.len());
    let mut cursor = discriminator_len.min(data.len());

    for arg in args {
        let obj = match arg.as_object() {
            Some(o) => o,
            None => continue,
        };
        let name = obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let type_val = match obj.get("type") {
            Some(t) => t,
            None => {
                out.push(DecodedArg {
                    name,
                    type_label: "<no type>".into(),
                    value_display: String::new(),
                    byte_offset: cursor,
                    byte_length: Some(0),
                });
                continue;
            }
        };

        let before = cursor;
        match decode_one(type_val, &data[cursor.min(data.len())..]) {
            Ok((type_label, value_display, consumed)) => {
                cursor = cursor.saturating_add(consumed).min(data.len());
                out.push(DecodedArg {
                    name,
                    type_label,
                    value_display,
                    byte_offset: before,
                    byte_length: Some(consumed),
                });
            }
            Err((type_label, reason)) => {
                // Can't advance the cursor past an unknown width, so stop.
                out.push(DecodedArg {
                    name,
                    type_label,
                    value_display: format!("<{reason}>"),
                    byte_offset: before,
                    byte_length: None,
                });
                break;
            }
        }
    }
    out
}

fn decode_one(type_val: &Value, data: &[u8]) -> Result<(String, String, usize), (String, String)> {
    let resolved = resolve_type(type_val);
    match resolved {
        ResolvedType::Scalar(kind) => decode_scalar(kind, data),
        ResolvedType::Unknown(label) => Err((label, "unsupported type".into())),
    }
}

enum ScalarKind {
    U8, U16, U32, U64, U128,
    I8, I16, I32, I64,
    Bool,
    Pubkey,
    StringType,
    Bytes,
}

enum ResolvedType {
    Scalar(ScalarKind),
    Unknown(String),
}

fn resolve_type(type_val: &Value) -> ResolvedType {
    if let Some(s) = type_val.as_str() {
        return scalar_from_name(s)
            .map(ResolvedType::Scalar)
            .unwrap_or_else(|| ResolvedType::Unknown(s.to_string()));
    }
    let Some(obj) = type_val.as_object() else {
        return ResolvedType::Unknown("<non-object type>".into());
    };
    let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "numberTypeNode" => {
            let format = obj.get("format").and_then(|f| f.as_str()).unwrap_or("");
            scalar_from_name(format)
                .map(ResolvedType::Scalar)
                .unwrap_or_else(|| ResolvedType::Unknown(format!("numberTypeNode[{format}]")))
        }
        "publicKeyTypeNode" => ResolvedType::Scalar(ScalarKind::Pubkey),
        "booleanTypeNode" => ResolvedType::Scalar(ScalarKind::Bool),
        "stringTypeNode" => ResolvedType::Scalar(ScalarKind::StringType),
        "bytesTypeNode" => ResolvedType::Scalar(ScalarKind::Bytes),
        _ => ResolvedType::Unknown(format!("kind={kind}")),
    }
}

fn scalar_from_name(name: &str) -> Option<ScalarKind> {
    Some(match name {
        "u8" => ScalarKind::U8,
        "u16" => ScalarKind::U16,
        "u32" => ScalarKind::U32,
        "u64" => ScalarKind::U64,
        "u128" => ScalarKind::U128,
        "i8" => ScalarKind::I8,
        "i16" => ScalarKind::I16,
        "i32" => ScalarKind::I32,
        "i64" => ScalarKind::I64,
        "bool" => ScalarKind::Bool,
        "pubkey" | "publicKey" => ScalarKind::Pubkey,
        "string" => ScalarKind::StringType,
        "bytes" => ScalarKind::Bytes,
        _ => return None,
    })
}

fn decode_scalar(kind: ScalarKind, data: &[u8]) -> Result<(String, String, usize), (String, String)> {
    match kind {
        ScalarKind::U8 => fixed(data, 1, "u8", |b| format!("{}", b[0])),
        ScalarKind::U16 => fixed(data, 2, "u16", |b| {
            format!("{}", u16::from_le_bytes([b[0], b[1]]))
        }),
        ScalarKind::U32 => fixed(data, 4, "u32", |b| {
            format!("{}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }),
        ScalarKind::U64 => fixed(data, 8, "u64", |b| {
            format!(
                "{}",
                u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            )
        }),
        ScalarKind::U128 => fixed(data, 16, "u128", |b| {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&b[..16]);
            format!("{}", u128::from_le_bytes(arr))
        }),
        ScalarKind::I8 => fixed(data, 1, "i8", |b| format!("{}", b[0] as i8)),
        ScalarKind::I16 => fixed(data, 2, "i16", |b| {
            format!("{}", i16::from_le_bytes([b[0], b[1]]))
        }),
        ScalarKind::I32 => fixed(data, 4, "i32", |b| {
            format!("{}", i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        }),
        ScalarKind::I64 => fixed(data, 8, "i64", |b| {
            format!(
                "{}",
                i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            )
        }),
        ScalarKind::Bool => fixed(data, 1, "bool", |b| match b[0] {
            0 => "false".to_string(),
            1 => "true".to_string(),
            n => format!("<invalid bool: {n}>"),
        }),
        ScalarKind::Pubkey => fixed(data, 32, "pubkey", |b| bs58::encode(b).into_string()),
        ScalarKind::StringType => decode_len_prefixed(data, "string", true),
        ScalarKind::Bytes => decode_len_prefixed(data, "bytes", false),
    }
}

fn fixed(
    data: &[u8],
    n: usize,
    label: &str,
    render: impl Fn(&[u8]) -> String,
) -> Result<(String, String, usize), (String, String)> {
    if data.len() < n {
        return Err((
            label.to_string(),
            format!("need {n} bytes, only {} available", data.len()),
        ));
    }
    Ok((label.to_string(), render(&data[..n]), n))
}

fn decode_len_prefixed(
    data: &[u8],
    label: &str,
    as_utf8: bool,
) -> Result<(String, String, usize), (String, String)> {
    if data.len() < 4 {
        return Err((
            label.to_string(),
            format!("need 4-byte length prefix, only {} available", data.len()),
        ));
    }
    let len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let total = 4usize.saturating_add(len);
    if data.len() < total {
        return Err((
            label.to_string(),
            format!("prefixed length {len} exceeds available {}", data.len() - 4),
        ));
    }
    let payload = &data[4..total];
    let display = if as_utf8 {
        match std::str::from_utf8(payload) {
            Ok(s) if s.chars().count() <= 60 => format!("\"{s}\""),
            Ok(s) => {
                let truncated: String = s.chars().take(57).collect();
                format!("\"{truncated}…\"")
            }
            Err(_) => format!("<invalid utf8, {} bytes>", payload.len()),
        }
    } else if payload.len() <= 16 {
        payload
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        format!("<{} bytes>", payload.len())
    };
    Ok((label.to_string(), display, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args_json(args: Value) -> Vec<Value> {
        args.as_array().unwrap().clone()
    }

    #[test]
    fn decodes_u64_little_endian() {
        let args = args_json(json!([{"name":"amount","type":"u64"}]));
        // 1,000,000 LE = 40 42 0f 00 00 00 00 00
        let data = vec![0x40, 0x42, 0x0f, 0, 0, 0, 0, 0];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "amount");
        assert_eq!(out[0].type_label, "u64");
        assert_eq!(out[0].value_display, "1000000");
        assert_eq!(out[0].byte_offset, 0);
        assert_eq!(out[0].byte_length, Some(8));
    }

    #[test]
    fn skips_discriminator_bytes() {
        let args = args_json(json!([{"name":"n","type":"u8"}]));
        let data = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x42];
        let out = decode_args(&args, &data, 8);
        assert_eq!(out[0].value_display, "66");
        assert_eq!(out[0].byte_offset, 8);
    }

    #[test]
    fn multiple_args_advance_cursor_correctly() {
        let args = args_json(json!([
            {"name":"a","type":"u8"},
            {"name":"b","type":"u32"},
            {"name":"c","type":"bool"}
        ]));
        let data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x01];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].byte_offset, 0);
        assert_eq!(out[0].byte_length, Some(1));
        assert_eq!(out[0].value_display, "1");
        assert_eq!(out[1].byte_offset, 1);
        assert_eq!(out[1].byte_length, Some(4));
        // LE [02 03 04 05] = 0x05040302
        assert_eq!(out[1].value_display, "84148994");
        assert_eq!(out[2].byte_offset, 5);
        assert_eq!(out[2].value_display, "true");
    }

    #[test]
    fn pubkey_renders_base58() {
        let args = args_json(json!([{"name":"auth","type":"pubkey"}]));
        let data = vec![0u8; 32];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out[0].type_label, "pubkey");
        // 32 zero bytes encode to "11111111111111111111111111111111" in base58.
        assert_eq!(out[0].value_display, "11111111111111111111111111111111");
        assert_eq!(out[0].byte_length, Some(32));
    }

    #[test]
    fn codama_number_type_node_is_recognized() {
        let args = args_json(json!([{
            "name":"n",
            "type":{"kind":"numberTypeNode","format":"u16","endian":"le"}
        }]));
        // 258 LE = 02 01
        let data = vec![0x02, 0x01];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out[0].type_label, "u16");
        assert_eq!(out[0].value_display, "258");
    }

    #[test]
    fn codama_public_key_type_node_is_recognized() {
        let args = args_json(json!([{
            "name":"who",
            "type":{"kind":"publicKeyTypeNode"}
        }]));
        let data = vec![0u8; 32];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out[0].type_label, "pubkey");
        assert_eq!(out[0].byte_length, Some(32));
    }

    #[test]
    fn unknown_type_halts_decoding_at_first_occurrence() {
        let args = args_json(json!([
            {"name":"a","type":"u8"},
            {"name":"b","type":"vec_of_something"},
            {"name":"c","type":"u8"}
        ]));
        let data = vec![0x10, 0x20, 0x30];
        let out = decode_args(&args, &data, 0);
        // First arg decodes; second hits unknown and stops; third is never emitted.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].value_display, "16");
        assert_eq!(out[1].byte_length, None);
        assert!(out[1].value_display.contains("unsupported"));
    }

    #[test]
    fn truncated_data_records_error_and_stops() {
        let args = args_json(json!([
            {"name":"a","type":"u32"},
            {"name":"b","type":"u32"}
        ]));
        // Only 4 bytes — second u32 will error.
        let data = vec![0x01, 0x02, 0x03, 0x04];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].value_display, "67305985");
        assert_eq!(out[1].byte_length, None);
        assert!(out[1].value_display.contains("need"));
    }

    #[test]
    fn string_renders_utf8_with_quotes() {
        let args = args_json(json!([{"name":"s","type":"string"}]));
        let text = "hello";
        let mut data = (text.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(text.as_bytes());
        let out = decode_args(&args, &data, 0);
        assert_eq!(out[0].type_label, "string");
        assert_eq!(out[0].value_display, "\"hello\"");
    }

    #[test]
    fn signed_integers_work() {
        let args = args_json(json!([
            {"name":"a","type":"i8"},
            {"name":"b","type":"i32"}
        ]));
        let data = vec![
            0xFF, // i8 = -1
            0x00, 0x00, 0x00, 0x80, // i32 LE = i32::MIN
        ];
        let out = decode_args(&args, &data, 0);
        assert_eq!(out[0].value_display, "-1");
        assert_eq!(out[1].value_display, i32::MIN.to_string());
    }
}
