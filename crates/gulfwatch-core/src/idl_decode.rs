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
    types: &[Value],
) -> Vec<DecodedArg> {
    let mut out = Vec::with_capacity(args.len());
    let mut cursor = discriminator_len.min(data.len());
    let ctx = DecodeCtx { types, depth: 0 };

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
        match decode_one(type_val, &data[cursor.min(data.len())..], ctx) {
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

#[derive(Clone, Copy)]
struct DecodeCtx<'a> {
    types: &'a [Value],
    depth: usize,
}

impl<'a> DecodeCtx<'a> {
    fn deeper(self) -> DecodeCtx<'a> {
        DecodeCtx {
            types: self.types,
            depth: self.depth + 1,
        }
    }
}

const MAX_DEPTH: usize = 8;

fn decode_one(
    type_val: &Value,
    data: &[u8],
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    if ctx.depth > MAX_DEPTH {
        return Err((
            "<deep>".to_string(),
            format!("max recursion depth {MAX_DEPTH} exceeded"),
        ));
    }

    if let Some(s) = type_val.as_str() {
        return match scalar_from_name(s) {
            Some(kind) => decode_scalar(kind, data),
            None => Err((s.to_string(), "unsupported type".into())),
        };
    }
    let Some(obj) = type_val.as_object() else {
        return Err(("<non-object>".to_string(), "unsupported type".into()));
    };

    // Anchor defined reference: {"defined":"X"} or {"defined":{"name":"X"}}.
    if let Some(name) = extract_defined_name(obj) {
        return resolve_and_decode_defined(&name, data, ctx);
    }

    // Anchor composites
    if let Some(inner) = obj.get("vec") {
        return decode_vec(inner, data, 4, ctx);
    }
    if let Some(inner) = obj.get("option") {
        return decode_option(inner, data, 1, ctx);
    }
    if let Some(arr_def) = obj.get("array").and_then(|v| v.as_array()) {
        let item = arr_def
            .first()
            .ok_or_else(|| ("array".to_string(), "missing item type".into()))?;
        let n = arr_def
            .get(1)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ("array".to_string(), "missing count".into()))?
            as usize;
        return decode_fixed_array(item, data, n, ctx);
    }

    // Codama type nodes
    let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match kind {
        "numberTypeNode" => {
            let format = obj.get("format").and_then(|f| f.as_str()).unwrap_or("");
            match scalar_from_name(format) {
                Some(k) => decode_scalar(k, data),
                None => Err((
                    format!("numberTypeNode[{format}]"),
                    "unsupported type".into(),
                )),
            }
        }
        "publicKeyTypeNode" => decode_scalar(ScalarKind::Pubkey, data),
        "booleanTypeNode" => decode_scalar(ScalarKind::Bool, data),
        "stringTypeNode" => decode_scalar(ScalarKind::StringType, data),
        "bytesTypeNode" => decode_scalar(ScalarKind::Bytes, data),
        "optionTypeNode" => {
            let inner = obj
                .get("item")
                .ok_or_else(|| ("optionTypeNode".to_string(), "missing item".into()))?;
            let prefix_bytes = codama_number_size(obj.get("prefix")).unwrap_or(1);
            decode_option(inner, data, prefix_bytes, ctx)
        }
        "arrayTypeNode" => {
            let item = obj
                .get("item")
                .ok_or_else(|| ("arrayTypeNode".to_string(), "missing item".into()))?;
            let count = obj
                .get("count")
                .and_then(|c| c.as_object())
                .ok_or_else(|| ("arrayTypeNode".to_string(), "missing count".into()))?;
            let count_kind = count.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            match count_kind {
                "fixedCountNode" => {
                    let n = count
                        .get("value")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    decode_fixed_array(item, data, n, ctx)
                }
                "prefixedCountNode" => {
                    let prefix_bytes = codama_number_size(count.get("prefix")).unwrap_or(4);
                    decode_vec(item, data, prefix_bytes, ctx)
                }
                other => Err((
                    "arrayTypeNode".to_string(),
                    format!("unsupported count kind '{other}'"),
                )),
            }
        }
        "definedTypeLinkNode" => {
            let name = obj
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| ("definedTypeLinkNode".to_string(), "missing name".into()))?
                .to_string();
            resolve_and_decode_defined(&name, data, ctx)
        }
        _ => Err((format!("kind={kind}"), "unsupported type".into())),
    }
}

fn extract_defined_name(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let defined = obj.get("defined")?;
    if let Some(s) = defined.as_str() {
        return Some(s.to_string());
    }
    if let Some(inner_obj) = defined.as_object() {
        if let Some(name) = inner_obj.get("name").and_then(|n| n.as_str()) {
            return Some(name.to_string());
        }
    }
    None
}

fn resolve_and_decode_defined(
    name: &str,
    data: &[u8],
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    let entry = ctx
        .types
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        .ok_or_else(|| (format!("defined<{name}>"), "type not found".into()))?;
    let body = entry
        .get("type")
        .ok_or_else(|| (format!("defined<{name}>"), "type entry has no body".into()))?;
    let body_obj = body
        .as_object()
        .ok_or_else(|| (format!("defined<{name}>"), "type body is not an object".into()))?;
    let body_kind = body_obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    match body_kind {
        "struct" | "structTypeNode" => decode_struct(name, body_obj, data, ctx.deeper()),
        "enum" | "enumTypeNode" => decode_enum(name, body_obj, data, ctx.deeper()),
        other => Err((
            format!("defined<{name}>"),
            format!("unsupported body kind '{other}'"),
        )),
    }
}

fn decode_struct(
    name: &str,
    body: &serde_json::Map<String, Value>,
    data: &[u8],
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    let fields = body
        .get("fields")
        .and_then(|f| f.as_array())
        .ok_or_else(|| (name.to_string(), "struct has no fields".into()))?;
    let mut cursor = 0usize;
    let mut displays = Vec::with_capacity(fields.len().min(4));
    for (i, field) in fields.iter().enumerate() {
        let field_obj = field
            .as_object()
            .ok_or_else(|| (name.to_string(), format!("field {i} is not an object")))?;
        let field_name = field_obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("_");
        let field_type = field_obj
            .get("type")
            .ok_or_else(|| (name.to_string(), format!("field '{field_name}' has no type")))?;
        match decode_one(field_type, &data[cursor..], ctx) {
            Ok((_l, display, consumed)) => {
                if i < 4 {
                    displays.push(format!("{field_name}: {display}"));
                }
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| (name.to_string(), "offset overflow".into()))?;
            }
            Err((_l, reason)) => {
                return Err((
                    name.to_string(),
                    format!("field '{field_name}': {reason}"),
                ));
            }
        }
    }
    let field_count = fields.len();
    let display = if field_count == 0 {
        "{}".to_string()
    } else if field_count <= 4 {
        format!("{{ {} }}", displays.join(", "))
    } else {
        format!("{{ <{field_count} fields> }}")
    };
    Ok((name.to_string(), display, cursor))
}

fn decode_enum(
    name: &str,
    body: &serde_json::Map<String, Value>,
    data: &[u8],
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    if data.is_empty() {
        return Err((name.to_string(), "need 1-byte variant tag, 0 available".into()));
    }
    let tag = data[0] as usize;
    let variants = body
        .get("variants")
        .and_then(|v| v.as_array())
        .ok_or_else(|| (name.to_string(), "enum has no variants".into()))?;
    let variant = variants
        .get(tag)
        .ok_or_else(|| (name.to_string(), format!("variant index {tag} out of range")))?;
    let variant_obj = variant
        .as_object()
        .ok_or_else(|| (name.to_string(), format!("variant {tag} is not an object")))?;
    let variant_name = variant_obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("_")
        .to_string();

    // Anchor form: variant has "fields": [...]
    // Codama form: variant kind = enumStructVariantTypeNode with "struct": {...}
    //              or enumEmptyVariantTypeNode (just a name)
    let variant_kind = variant_obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let fields_source: Option<&Vec<Value>> = if variant_kind == "enumStructVariantTypeNode" {
        variant_obj
            .get("struct")
            .and_then(|s| s.as_object())
            .and_then(|s| s.get("fields"))
            .and_then(|f| f.as_array())
    } else {
        variant_obj.get("fields").and_then(|f| f.as_array())
    };

    if let Some(fields) = fields_source {
        let synthetic_struct_body: serde_json::Map<String, Value> = {
            let mut m = serde_json::Map::new();
            m.insert("fields".into(), Value::Array(fields.clone()));
            m
        };
        let (_label, struct_display, consumed) = decode_struct(
            &variant_name,
            &synthetic_struct_body,
            &data[1..],
            ctx,
        )?;
        let display = if struct_display == "{}" {
            variant_name.clone()
        } else {
            format!("{variant_name} {struct_display}")
        };
        return Ok((name.to_string(), display, 1 + consumed));
    }

    Ok((name.to_string(), variant_name, 1))
}

enum ScalarKind {
    U8, U16, U32, U64, U128,
    I8, I16, I32, I64,
    Bool,
    Pubkey,
    StringType,
    Bytes,
}

fn codama_number_size(prefix_val: Option<&Value>) -> Option<usize> {
    let obj = prefix_val?.as_object()?;
    if obj.get("kind").and_then(|k| k.as_str()) != Some("numberTypeNode") {
        return None;
    }
    let format = obj.get("format").and_then(|f| f.as_str())?;
    match format {
        "u8" => Some(1),
        "u16" => Some(2),
        "u32" => Some(4),
        "u64" => Some(8),
        _ => None,
    }
}

fn type_label_of(type_val: &Value) -> String {
    if let Some(s) = type_val.as_str() {
        return s.to_string();
    }
    let Some(obj) = type_val.as_object() else {
        return "?".to_string();
    };
    if let Some(defined) = obj.get("defined") {
        if let Some(s) = defined.as_str() {
            return s.to_string();
        }
        if let Some(name) = defined.as_object().and_then(|o| o.get("name")).and_then(|n| n.as_str())
        {
            return name.to_string();
        }
    }
    if let Some(inner) = obj.get("vec") {
        return format!("vec<{}>", type_label_of(inner));
    }
    if let Some(inner) = obj.get("option") {
        return format!("option<{}>", type_label_of(inner));
    }
    if let Some(arr) = obj.get("array").and_then(|v| v.as_array()) {
        if let (Some(t), Some(n)) = (arr.first(), arr.get(1).and_then(|v| v.as_u64())) {
            return format!("[{}; {n}]", type_label_of(t));
        }
    }
    let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
    match kind {
        "numberTypeNode" => obj
            .get("format")
            .and_then(|f| f.as_str())
            .unwrap_or("?")
            .to_string(),
        "publicKeyTypeNode" => "pubkey".to_string(),
        "booleanTypeNode" => "bool".to_string(),
        "stringTypeNode" => "string".to_string(),
        "bytesTypeNode" => "bytes".to_string(),
        "definedTypeLinkNode" => obj
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        "optionTypeNode" => {
            let inner = obj
                .get("item")
                .map(type_label_of)
                .unwrap_or_else(|| "?".to_string());
            format!("option<{inner}>")
        }
        "arrayTypeNode" => {
            let item = obj
                .get("item")
                .map(type_label_of)
                .unwrap_or_else(|| "?".to_string());
            let count = obj.get("count").and_then(|c| c.as_object());
            match count.and_then(|c| c.get("kind").and_then(|k| k.as_str())) {
                Some("fixedCountNode") => {
                    let n = count
                        .and_then(|c| c.get("value"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    format!("[{item}; {n}]")
                }
                Some("prefixedCountNode") => format!("vec<{item}>"),
                _ => format!("array<{item}>"),
            }
        }
        other => format!("kind={other}"),
    }
}

fn decode_vec(
    inner_type: &Value,
    data: &[u8],
    prefix_bytes: usize,
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    let inner_label = type_label_of(inner_type);
    let type_label = format!("vec<{inner_label}>");
    if data.len() < prefix_bytes {
        return Err((
            type_label,
            format!(
                "need {prefix_bytes}-byte length prefix, only {} available",
                data.len()
            ),
        ));
    }
    let count = read_le_count(&data[..prefix_bytes]);
    let mut cursor = prefix_bytes;
    let mut items = Vec::with_capacity(count.min(16));
    let inner_ctx = ctx.deeper();
    for i in 0..count {
        match decode_one(inner_type, &data[cursor..], inner_ctx) {
            Ok((_l, display, n)) => {
                if i < 4 {
                    items.push(display);
                }
                cursor = cursor
                    .checked_add(n)
                    .ok_or_else(|| (type_label.clone(), "offset overflow".into()))?;
            }
            Err((_l, reason)) => {
                return Err((type_label, format!("item {i}: {reason}")));
            }
        }
    }
    let display = if count == 0 {
        "[]".to_string()
    } else if count <= 4 {
        format!("[{}]", items.join(", "))
    } else {
        format!("<{count} items>")
    };
    Ok((type_label, display, cursor))
}

fn decode_option(
    inner_type: &Value,
    data: &[u8],
    prefix_bytes: usize,
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    let type_label = format!("option<{}>", type_label_of(inner_type));
    if data.len() < prefix_bytes {
        return Err((
            type_label,
            format!(
                "need {prefix_bytes}-byte flag, only {} available",
                data.len()
            ),
        ));
    }
    let is_some = data[..prefix_bytes].iter().any(|&b| b != 0);
    if !is_some {
        return Ok((type_label, "None".to_string(), prefix_bytes));
    }
    match decode_one(inner_type, &data[prefix_bytes..], ctx.deeper()) {
        Ok((_l, display, n)) => Ok((
            type_label,
            format!("Some({display})"),
            prefix_bytes + n,
        )),
        Err((_l, reason)) => Err((type_label, format!("inner: {reason}"))),
    }
}

fn decode_fixed_array(
    inner_type: &Value,
    data: &[u8],
    n: usize,
    ctx: DecodeCtx,
) -> Result<(String, String, usize), (String, String)> {
    let inner_label = type_label_of(inner_type);
    let type_label = format!("[{inner_label}; {n}]");
    let mut cursor = 0usize;
    let mut items = Vec::with_capacity(n.min(16));
    let inner_ctx = ctx.deeper();
    for i in 0..n {
        match decode_one(inner_type, &data[cursor..], inner_ctx) {
            Ok((_l, display, consumed)) => {
                if i < 4 {
                    items.push(display);
                }
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| (type_label.clone(), "offset overflow".into()))?;
            }
            Err((_l, reason)) => {
                return Err((type_label, format!("item {i}: {reason}")));
            }
        }
    }
    let display = if n == 0 {
        "[]".to_string()
    } else if n <= 4 {
        format!("[{}]", items.join(", "))
    } else {
        format!("<{n} items>")
    };
    Ok((type_label, display, cursor))
}

fn read_le_count(bytes: &[u8]) -> usize {
    let mut arr = [0u8; 8];
    let n = bytes.len().min(8);
    arr[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(arr) as usize
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 8, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
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
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "string");
        assert_eq!(out[0].value_display, "\"hello\"");
    }

    #[test]
    fn anchor_vec_u64_inline_display() {
        let args = args_json(json!([{"name":"xs","type":{"vec":"u64"}}]));
        let mut data = (3u32).to_le_bytes().to_vec();
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(&2u64.to_le_bytes());
        data.extend_from_slice(&3u64.to_le_bytes());
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "vec<u64>");
        assert_eq!(out[0].value_display, "[1, 2, 3]");
        assert_eq!(out[0].byte_length, Some(4 + 24));
    }

    #[test]
    fn anchor_vec_collapses_large_counts() {
        let args = args_json(json!([{"name":"xs","type":{"vec":"u8"}}]));
        let mut data = (20u32).to_le_bytes().to_vec();
        data.extend_from_slice(&[0xAAu8; 20]);
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].value_display, "<20 items>");
        assert_eq!(out[0].byte_length, Some(24));
    }

    #[test]
    fn anchor_empty_vec_renders_as_brackets() {
        let args = args_json(json!([{"name":"xs","type":{"vec":"u64"}}]));
        let data = (0u32).to_le_bytes().to_vec();
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "vec<u64>");
        assert_eq!(out[0].value_display, "[]");
        assert_eq!(out[0].byte_length, Some(4));
    }

    #[test]
    fn anchor_option_some_and_none() {
        let args = args_json(json!([
            {"name":"maybe_a","type":{"option":"u32"}},
            {"name":"maybe_b","type":{"option":"u32"}}
        ]));
        let mut data = vec![1u8]; // Some
        data.extend_from_slice(&42u32.to_le_bytes());
        data.push(0u8); // None
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "option<u32>");
        assert_eq!(out[0].value_display, "Some(42)");
        assert_eq!(out[0].byte_length, Some(5));
        assert_eq!(out[1].value_display, "None");
        assert_eq!(out[1].byte_length, Some(1));
    }

    #[test]
    fn anchor_fixed_array_renders_bracket_list() {
        let args = args_json(json!([{"name":"buf","type":{"array":["u8",4]}}]));
        let data = vec![0x01, 0x02, 0x03, 0x04];
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "[u8; 4]");
        assert_eq!(out[0].value_display, "[1, 2, 3, 4]");
        assert_eq!(out[0].byte_length, Some(4));
    }

    #[test]
    fn codama_prefixed_array_is_treated_as_vec() {
        let args = args_json(json!([{
            "name":"xs",
            "type":{
                "kind":"arrayTypeNode",
                "item":{"kind":"numberTypeNode","format":"u16"},
                "count":{
                    "kind":"prefixedCountNode",
                    "prefix":{"kind":"numberTypeNode","format":"u32"}
                }
            }
        }]));
        let mut data = (2u32).to_le_bytes().to_vec();
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&200u16.to_le_bytes());
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "vec<u16>");
        assert_eq!(out[0].value_display, "[100, 200]");
        assert_eq!(out[0].byte_length, Some(4 + 4));
    }

    #[test]
    fn codama_fixed_array_of_bytes() {
        let args = args_json(json!([{
            "name":"sig",
            "type":{
                "kind":"arrayTypeNode",
                "item":{"kind":"numberTypeNode","format":"u8"},
                "count":{"kind":"fixedCountNode","value":3}
            }
        }]));
        let data = vec![0xFF, 0x01, 0x02];
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "[u8; 3]");
        assert_eq!(out[0].value_display, "[255, 1, 2]");
    }

    #[test]
    fn codama_option_with_u8_prefix() {
        let args = args_json(json!([{
            "name":"maybe",
            "type":{
                "kind":"optionTypeNode",
                "item":{"kind":"publicKeyTypeNode"},
                "prefix":{"kind":"numberTypeNode","format":"u8"}
            }
        }]));
        let mut data = vec![1u8];
        data.extend_from_slice(&[0u8; 32]);
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "option<pubkey>");
        assert_eq!(
            out[0].value_display,
            "Some(11111111111111111111111111111111)"
        );
    }

    #[test]
    fn nested_vec_of_option_of_u32() {
        let args = args_json(json!([{
            "name":"xs",
            "type":{"vec":{"option":"u32"}}
        }]));
        let mut data = (2u32).to_le_bytes().to_vec();
        data.push(1); // Some
        data.extend_from_slice(&10u32.to_le_bytes());
        data.push(0); // None
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].type_label, "vec<option<u32>>");
        assert_eq!(out[0].value_display, "[Some(10), None]");
    }

    #[test]
    fn vec_with_unsupported_inner_type_errors_at_item_index() {
        let args = args_json(json!([{"name":"xs","type":{"vec":"somethingExotic"}}]));
        let mut data = (2u32).to_le_bytes().to_vec();
        data.extend_from_slice(&[0xAA, 0xBB]);
        let out = decode_args(&args, &data, 0, &[]);
        // Vec decoding surfaces the inner unsupported-type error; decode halts.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].byte_length, None);
        assert!(out[0].value_display.contains("item 0"));
    }

    #[test]
    fn composite_advances_cursor_for_next_arg() {
        let args = args_json(json!([
            {"name":"xs","type":{"vec":"u32"}},
            {"name":"trailer","type":"u8"}
        ]));
        let mut data = (2u32).to_le_bytes().to_vec();
        data.extend_from_slice(&100u32.to_le_bytes());
        data.extend_from_slice(&200u32.to_le_bytes());
        data.push(0xEE);
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].value_display, "[100, 200]");
        assert_eq!(out[0].byte_length, Some(4 + 8));
        assert_eq!(out[1].byte_offset, 12);
        assert_eq!(out[1].value_display, "238");
    }

    #[test]
    fn anchor_defined_resolves_to_struct_with_scalar_fields() {
        let types = json!([{
            "name": "Point",
            "type": {
                "kind": "struct",
                "fields": [
                    {"name": "x", "type": "u32"},
                    {"name": "y", "type": "u32"}
                ]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{"name":"p","type":{"defined":"Point"}}]));
        let mut data = 10u32.to_le_bytes().to_vec();
        data.extend_from_slice(&20u32.to_le_bytes());
        let out = decode_args(&args, &data, 0, &types);
        assert_eq!(out[0].type_label, "Point");
        assert_eq!(out[0].value_display, "{ x: 10, y: 20 }");
        assert_eq!(out[0].byte_length, Some(8));
    }

    #[test]
    fn anchor_defined_resolves_to_enum_with_unit_variant() {
        let types = json!([{
            "name": "Side",
            "type": {
                "kind": "enum",
                "variants": [{"name":"Bid"}, {"name":"Ask"}]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{"name":"side","type":{"defined":"Side"}}]));
        let out = decode_args(&args, &[1u8], 0, &types);
        assert_eq!(out[0].type_label, "Side");
        assert_eq!(out[0].value_display, "Ask");
        assert_eq!(out[0].byte_length, Some(1));
    }

    #[test]
    fn anchor_defined_enum_with_tuple_variant_fields() {
        let types = json!([{
            "name": "Swap",
            "type": {
                "kind": "enum",
                "variants": [
                    {"name": "Saber"},
                    {"name": "Crema", "fields": [{"name": "a_to_b", "type": "bool"}]}
                ]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{"name":"s","type":{"defined":"Swap"}}]));
        // variant 1 (Crema) + bool true
        let data = vec![1u8, 1u8];
        let out = decode_args(&args, &data, 0, &types);
        assert_eq!(out[0].type_label, "Swap");
        assert_eq!(out[0].value_display, "Crema { a_to_b: true }");
        assert_eq!(out[0].byte_length, Some(2));
    }

    #[test]
    fn anchor_defined_name_wrapper_form_resolves() {
        // Anchor 0.30+ wraps names: {"defined":{"name":"Point"}}
        let types = json!([{
            "name": "Point",
            "type": {
                "kind": "struct",
                "fields": [{"name": "x", "type": "u8"}]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{"name":"p","type":{"defined":{"name":"Point"}}}]));
        let out = decode_args(&args, &[7u8], 0, &types);
        assert_eq!(out[0].value_display, "{ x: 7 }");
    }

    #[test]
    fn codama_defined_type_link_resolves_to_struct() {
        let types = json!([{
            "kind": "definedTypeNode",
            "name": "Point",
            "type": {
                "kind": "structTypeNode",
                "fields": [
                    {"kind":"structFieldTypeNode","name":"x","type":{"kind":"numberTypeNode","format":"u8"}},
                    {"kind":"structFieldTypeNode","name":"y","type":{"kind":"numberTypeNode","format":"u8"}}
                ]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{
            "name": "p",
            "type": {"kind":"definedTypeLinkNode","name":"Point"}
        }]));
        let out = decode_args(&args, &[3u8, 4u8], 0, &types);
        assert_eq!(out[0].type_label, "Point");
        assert_eq!(out[0].value_display, "{ x: 3, y: 4 }");
    }

    #[test]
    fn vec_of_defined_struct_decodes_all_items() {
        let types = json!([{
            "name": "Step",
            "type": {
                "kind": "struct",
                "fields": [
                    {"name": "percent", "type": "u8"},
                    {"name": "input", "type": "u8"}
                ]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{
            "name": "steps",
            "type": {"vec": {"defined":"Step"}}
        }]));
        let mut data = (2u32).to_le_bytes().to_vec();
        data.extend_from_slice(&[50u8, 0u8, 50u8, 1u8]);
        let out = decode_args(&args, &data, 0, &types);
        assert_eq!(out[0].type_label, "vec<Step>");
        assert_eq!(
            out[0].value_display,
            "[{ percent: 50, input: 0 }, { percent: 50, input: 1 }]"
        );
        assert_eq!(out[0].byte_length, Some(4 + 4));
    }

    #[test]
    fn missing_defined_name_errors_cleanly() {
        let args = args_json(json!([{"name":"x","type":{"defined":"Nowhere"}}]));
        let out = decode_args(&args, &[0u8; 8], 0, &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].type_label, "defined<Nowhere>");
        assert_eq!(out[0].byte_length, None);
        assert!(out[0].value_display.contains("not found"));
    }

    #[test]
    fn recursion_depth_guard_halts_self_referential_type() {
        let types = json!([{
            "name": "Loop",
            "type": {
                "kind": "struct",
                "fields": [{"name": "next", "type": {"defined":"Loop"}}]
            }
        }])
        .as_array()
        .unwrap()
        .clone();
        let args = args_json(json!([{"name":"l","type":{"defined":"Loop"}}]));
        let out = decode_args(&args, &[0u8; 32], 0, &types);
        // Either errors at the top or deep — but never runs forever. Check
        // it returned and recorded a failure.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].byte_length, None);
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
        let out = decode_args(&args, &data, 0, &[]);
        assert_eq!(out[0].value_display, "-1");
        assert_eq!(out[1].value_display, i32::MIN.to_string());
    }
}
