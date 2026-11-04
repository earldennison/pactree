use super::SymbolTable;
use super::english_lexpack::ENGLISH_LEXPACK_TEXTS;
use super::layout::{
    BINARY_BYTES_SENTINEL, RECORD_PAYLOAD_MAGIC, VALUE_TAG_ARRAY, VALUE_TAG_BYTES, VALUE_TAG_F64,
    VALUE_TAG_FALSE, VALUE_TAG_FILE_LOCAL_ID_REF, VALUE_TAG_I64, VALUE_TAG_NULL, VALUE_TAG_OBJECT,
    VALUE_TAG_OBJECT_SYMBOL_KEYS, VALUE_TAG_STRING, VALUE_TAG_SYMBOL_REF,
    VALUE_TAG_TOKEN_SPAN_STRING, VALUE_TAG_TRUE, VALUE_TAG_U64, VALUE_TAG_VAR_U64,
    VALUE_TAG_ZIGZAG_I64,
};
use super::layout::{
    TOKEN_SPAN_PART_DENSE_TOKEN_ID, TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN, TOKEN_SPAN_PART_LEX_REF,
    TOKEN_SPAN_PART_LEX_REF_RUN, TOKEN_SPAN_PART_SPAN_REF, TOKEN_SPAN_PART_SPAN_REF_RUN,
    TOKEN_SPAN_PART_TOKEN_ID, TOKEN_SPAN_PART_TOKEN_ID_RUN, TOKEN_SPAN_PART_TOKEN_MIXED_RUN,
    TOKEN_SPAN_PART_TOKEN_TABLE_RUN, TOKEN_SPAN_PART_UTF8_ESCAPE,
    TOKEN_SPAN_TOKENIZER_O200K_BASE_ID,
};
use super::shell_lexpack::SHELL_LEXPACK_TEXTS;
use super::util::{base64_encode, write_u32};
use crate::error::{PactError, PactResult};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use tiktoken_rs::{Rank, o200k_base_singleton};

#[derive(Clone, Copy, Debug)]
pub(super) struct PactValueCodecOptions {
    pub(super) token_span_strings: bool,
}

impl Default for PactValueCodecOptions {
    fn default() -> Self {
        Self {
            token_span_strings: true,
        }
    }
}

pub(super) fn encode_pact_value_payload(value: &Value) -> PactResult<Vec<u8>> {
    encode_pact_value_payload_with_options(value, PactValueCodecOptions::default())
}

pub(super) fn encode_pact_value_payload_with_options(
    value: &Value,
    options: PactValueCodecOptions,
) -> PactResult<Vec<u8>> {
    encode_pact_value_payload_with_symbols_and_options(value, &SymbolTable::default(), options)
}

pub(super) fn encode_pact_value_payload_with_symbols_and_options(
    value: &Value,
    symbols: &SymbolTable,
    options: PactValueCodecOptions,
) -> PactResult<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(RECORD_PAYLOAD_MAGIC);
    encode_pact_value(value, &mut out, symbols, options)?;
    Ok(out)
}

#[derive(Clone, Debug)]
struct InstalledLexEntry {
    lexical_id: u64,
    pack: LexPackKind,
    text: &'static str,
    token_ids: Vec<Rank>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LexPackKind {
    English,
    Shell,
}

#[derive(Clone, Copy, Debug)]
struct ActiveLexPacks {
    shell: bool,
}

impl ActiveLexPacks {
    fn includes(self, pack: LexPackKind) -> bool {
        match pack {
            LexPackKind::English => true,
            LexPackKind::Shell => self.shell,
        }
    }
}

#[derive(Clone, Debug)]
enum TokenSpanPart {
    LexRef(u64),
    SpanRef(u64),
    DenseTokenId(u64),
    TokenId(Rank),
}

pub(super) fn decode_pact_value_payload_with_symbols(
    bytes: &[u8],
    symbols: &SymbolTable,
) -> PactResult<Value> {
    decode_pact_value_payload_with_symbols_mode(bytes, symbols, false)
}

pub(super) fn decode_trace_value_payload_with_symbols_filtered(
    bytes: &[u8],
    symbols: &SymbolTable,
) -> PactResult<Value> {
    decode_pact_value_payload_with_symbols_mode(bytes, symbols, true)
}

fn decode_pact_value_payload_with_symbols_mode(
    bytes: &[u8],
    symbols: &SymbolTable,
    filter_trace_ops: bool,
) -> PactResult<Value> {
    if bytes.len() < RECORD_PAYLOAD_MAGIC.len()
        || &bytes[..RECORD_PAYLOAD_MAGIC.len()] != RECORD_PAYLOAD_MAGIC
    {
        return Err(PactError::corrupt(
            "record payload missing Pact value magic",
        ));
    }
    let mut offset = RECORD_PAYLOAD_MAGIC.len();
    let value = decode_pact_value(bytes, &mut offset, symbols, filter_trace_ops)?;
    if offset != bytes.len() {
        return Err(PactError::codec(format!(
            "record payload has {} trailing bytes",
            bytes.len() - offset
        )));
    }
    Ok(value)
}

fn encode_pact_value(
    value: &Value,
    out: &mut Vec<u8>,
    symbols: &SymbolTable,
    options: PactValueCodecOptions,
) -> PactResult<()> {
    match value {
        Value::Null => out.push(VALUE_TAG_NULL),
        Value::Bool(false) => out.push(VALUE_TAG_FALSE),
        Value::Bool(true) => out.push(VALUE_TAG_TRUE),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                out.push(VALUE_TAG_ZIGZAG_I64);
                write_varint(out, zigzag_i64(value));
            } else if let Some(value) = number.as_u64() {
                out.push(VALUE_TAG_VAR_U64);
                write_varint(out, value);
            } else if let Some(value) = number.as_f64() {
                out.push(VALUE_TAG_F64);
                out.extend_from_slice(&value.to_le_bytes());
            } else {
                return Err(PactError::codec("unsupported JSON number"));
            }
        }
        Value::String(value) => {
            if let Some(id) = symbols.file_local_id(value) {
                out.push(VALUE_TAG_FILE_LOCAL_ID_REF);
                write_varint(out, u64::from(id));
            } else if let Some(id) = symbols.symbol_id(value) {
                out.push(VALUE_TAG_SYMBOL_REF);
                write_u32(out, id);
            } else if options.token_span_strings {
                encode_token_span_string(value, out, symbols)?;
            } else {
                out.push(VALUE_TAG_STRING);
                write_len_prefixed_bytes(out, value.as_bytes())?;
            }
        }
        Value::Array(values) => {
            out.push(VALUE_TAG_ARRAY);
            write_len(out, values.len())?;
            for value in values {
                encode_pact_value(value, out, symbols, options)?;
            }
        }
        Value::Object(object) => {
            if let Some(bytes) = binary_bytes_from_sentinel(object)? {
                out.push(VALUE_TAG_BYTES);
                write_len_prefixed_bytes(out, &bytes)?;
                return Ok(());
            }
            let symbolized_keys = object
                .keys()
                .map(|key| symbols.symbol_id(key))
                .collect::<Option<Vec<_>>>();
            if let Some(key_ids) = symbolized_keys {
                out.push(VALUE_TAG_OBJECT_SYMBOL_KEYS);
                write_len(out, object.len())?;
                for ((_, value), key_id) in object.iter().zip(key_ids) {
                    write_varint(out, u64::from(key_id));
                    encode_pact_value(value, out, symbols, options)?;
                }
            } else {
                out.push(VALUE_TAG_OBJECT);
                write_len(out, object.len())?;
                for (key, value) in object {
                    write_len_prefixed_bytes(out, key.as_bytes())?;
                    encode_pact_value(value, out, symbols, options)?;
                }
            }
        }
    }
    Ok(())
}

fn decode_pact_value(
    bytes: &[u8],
    offset: &mut usize,
    symbols: &SymbolTable,
    filter_trace_ops: bool,
) -> PactResult<Value> {
    let tag = read_byte(bytes, offset)?;
    match tag {
        VALUE_TAG_NULL => Ok(Value::Null),
        VALUE_TAG_FALSE => Ok(Value::Bool(false)),
        VALUE_TAG_TRUE => Ok(Value::Bool(true)),
        VALUE_TAG_I64 => {
            let value = i64::from_le_bytes(read_array::<8>(bytes, offset)?);
            Ok(Value::Number(value.into()))
        }
        VALUE_TAG_U64 => {
            let value = u64::from_le_bytes(read_array::<8>(bytes, offset)?);
            Ok(Value::Number(value.into()))
        }
        VALUE_TAG_VAR_U64 => {
            let value = read_varint(bytes, offset)?;
            Ok(Value::Number(value.into()))
        }
        VALUE_TAG_ZIGZAG_I64 => {
            let value = unzigzag_i64(read_varint(bytes, offset)?);
            Ok(Value::Number(value.into()))
        }
        VALUE_TAG_F64 => {
            let value = f64::from_le_bytes(read_array::<8>(bytes, offset)?);
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| PactError::codec("invalid f64 JSON number"))
        }
        VALUE_TAG_STRING => {
            let bytes = read_len_prefixed_bytes(bytes, offset)?;
            String::from_utf8(bytes.to_vec())
                .map(Value::String)
                .map_err(|err| PactError::codec(format!("invalid UTF-8 string: {err}")))
        }
        VALUE_TAG_TOKEN_SPAN_STRING => {
            decode_token_span_string(bytes, offset, symbols).map(Value::String)
        }
        VALUE_TAG_SYMBOL_REF => {
            let id = u32::from_le_bytes(read_array::<4>(bytes, offset)?);
            symbols
                .value(id)
                .map(|value| Value::String(value.to_string()))
                .ok_or_else(|| {
                    PactError::codec(format!("record payload references missing symbol {id}"))
                })
        }
        VALUE_TAG_FILE_LOCAL_ID_REF => {
            let id = read_varint(bytes, offset)?;
            let id = u32::try_from(id)
                .map_err(|_| PactError::codec(format!("file-local id {id} exceeds u32")))?;
            symbols
                .value(id)
                .map(|value| Value::String(value.to_string()))
                .ok_or_else(|| {
                    PactError::codec(format!(
                        "record payload references missing file-local id {id}"
                    ))
                })
        }
        VALUE_TAG_BYTES => {
            let bytes = read_len_prefixed_bytes(bytes, offset)?;
            Ok(Value::String(base64_encode(bytes)))
        }
        VALUE_TAG_ARRAY => {
            let len = read_len(bytes, offset)?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(decode_pact_value(bytes, offset, symbols, filter_trace_ops)?);
            }
            Ok(Value::Array(values))
        }
        VALUE_TAG_OBJECT => {
            let len = read_len(bytes, offset)?;
            let mut object = serde_json::Map::new();
            for _ in 0..len {
                let key_bytes = read_len_prefixed_bytes(bytes, offset)?;
                let key = String::from_utf8(key_bytes.to_vec())
                    .map_err(|err| PactError::codec(format!("invalid UTF-8 object key: {err}")))?;
                let value = if filter_trace_ops && key == "ops" {
                    decode_filtered_trace_ops(bytes, offset, symbols)?
                } else {
                    decode_pact_value(bytes, offset, symbols, filter_trace_ops)?
                };
                object.insert(key, value);
            }
            Ok(Value::Object(object))
        }
        VALUE_TAG_OBJECT_SYMBOL_KEYS => {
            let len = read_len(bytes, offset)?;
            let mut object = serde_json::Map::new();
            for _ in 0..len {
                let key_id = read_varint(bytes, offset)?;
                let key = symbols
                    .value(u32::try_from(key_id).map_err(|_| {
                        PactError::codec(format!("object key symbol id {key_id} exceeds u32"))
                    })?)
                    .ok_or_else(|| {
                        PactError::codec(format!("object references missing key symbol {key_id}"))
                    })?
                    .to_string();
                let value = if filter_trace_ops && key == "ops" {
                    decode_filtered_trace_ops(bytes, offset, symbols)?
                } else {
                    decode_pact_value(bytes, offset, symbols, filter_trace_ops)?
                };
                object.insert(key, value);
            }
            Ok(Value::Object(object))
        }
        _ => Err(PactError::codec(format!(
            "unknown Pact value tag 0x{tag:02x}"
        ))),
    }
}

fn decode_filtered_trace_ops(
    bytes: &[u8],
    offset: &mut usize,
    symbols: &SymbolTable,
) -> PactResult<Value> {
    let tag = read_byte(bytes, offset)?;
    if tag != VALUE_TAG_ARRAY {
        return Err(PactError::codec("trace ops payload is not an array"));
    }
    let len = read_len(bytes, offset)?;
    let mut ops = Vec::new();
    for _ in 0..len {
        let op = decode_pact_value(bytes, offset, symbols, false)?;
        if !compact_trace_op_is_derived(&op) {
            ops.push(op);
        }
    }
    Ok(Value::Array(ops))
}

fn compact_trace_op_is_derived(op: &Value) -> bool {
    const DERIVED_FIELDS: &[&str] = &[
        "depth",
        "pact_depth",
        "child_ids",
        "visible_in_head",
        "first_seen",
        "last_seen",
        "deleted_at",
        "pact_lifecycle",
    ];
    const DERIVED_ATTRS: &[&str] = &[
        "visible_in_head",
        "first_seen",
        "last_seen",
        "deleted_at",
        "pact_lifecycle",
    ];
    let Some(object) = op.as_object() else {
        return false;
    };
    let code = object.get("op").and_then(Value::as_u64);
    let key = object.get("key").and_then(Value::as_str);
    matches!(code, Some(4 | 5)) && key.is_some_and(|key| DERIVED_FIELDS.contains(&key))
        || matches!(code, Some(6 | 7)) && key.is_some_and(|key| DERIVED_ATTRS.contains(&key))
}

fn encode_token_span_string(
    value: &str,
    out: &mut Vec<u8>,
    symbols: &SymbolTable,
) -> PactResult<()> {
    let tokens = o200k_base_singleton().encode_ordinary(value);
    let decoded = o200k_base_singleton().decode(&tokens).map_err(|err| {
        PactError::codec(format!(
            "failed to validate token-span string decode: {err}"
        ))
    })?;
    if decoded != value {
        out.push(VALUE_TAG_TOKEN_SPAN_STRING);
        write_varint(out, TOKEN_SPAN_TOKENIZER_O200K_BASE_ID);
        write_len(out, 1)?;
        out.push(TOKEN_SPAN_PART_UTF8_ESCAPE);
        write_len_prefixed_bytes(out, value.as_bytes())?;
        return Ok(());
    }
    let parts = best_token_span_parts(value, &tokens, symbols);
    out.push(VALUE_TAG_TOKEN_SPAN_STRING);
    write_varint(out, TOKEN_SPAN_TOKENIZER_O200K_BASE_ID);
    write_token_span_parts(out, &parts)?;
    Ok(())
}

fn write_token_span_parts(out: &mut Vec<u8>, parts: &[TokenSpanPart]) -> PactResult<()> {
    let encoded = encode_token_span_parts_payload(parts)?;
    out.extend_from_slice(&encoded);
    Ok(())
}

fn encode_token_span_parts_payload(parts: &[TokenSpanPart]) -> PactResult<Vec<u8>> {
    let mut encoded = Vec::new();
    let mut part_count = 0usize;
    let mut index = 0usize;
    while index < parts.len() {
        match parts[index] {
            TokenSpanPart::LexRef(_) => {
                let start = index;
                while index < parts.len() && matches!(parts[index], TokenSpanPart::LexRef(_)) {
                    index += 1;
                }
                let run = &parts[start..index];
                if run.len() == 1 {
                    if let TokenSpanPart::LexRef(id) = run[0] {
                        encoded.push(TOKEN_SPAN_PART_LEX_REF);
                        write_varint(&mut encoded, id);
                    }
                } else {
                    encoded.push(TOKEN_SPAN_PART_LEX_REF_RUN);
                    write_len(&mut encoded, run.len())?;
                    for part in run {
                        if let TokenSpanPart::LexRef(id) = *part {
                            write_varint(&mut encoded, id);
                        }
                    }
                }
            }
            TokenSpanPart::SpanRef(_) => {
                let start = index;
                while index < parts.len() && matches!(parts[index], TokenSpanPart::SpanRef(_)) {
                    index += 1;
                }
                let run = &parts[start..index];
                if run.len() == 1 {
                    if let TokenSpanPart::SpanRef(id) = run[0] {
                        encoded.push(TOKEN_SPAN_PART_SPAN_REF);
                        write_varint(&mut encoded, id);
                    }
                } else {
                    encoded.push(TOKEN_SPAN_PART_SPAN_REF_RUN);
                    write_len(&mut encoded, run.len())?;
                    for part in run {
                        if let TokenSpanPart::SpanRef(id) = *part {
                            write_varint(&mut encoded, id);
                        }
                    }
                }
            }
            TokenSpanPart::DenseTokenId(_) | TokenSpanPart::TokenId(_) => {
                let start = index;
                while index < parts.len()
                    && matches!(
                        parts[index],
                        TokenSpanPart::DenseTokenId(_) | TokenSpanPart::TokenId(_)
                    )
                {
                    index += 1;
                }
                let run = &parts[start..index];
                let (run_count, run_encoded) = encode_token_run_parts_payload(run)?;
                part_count += run_count;
                encoded.extend_from_slice(&run_encoded);
                continue;
            }
        }
        part_count += 1;
    }
    let mut out = Vec::new();
    write_len(&mut out, part_count)?;
    out.extend_from_slice(&encoded);
    Ok(out)
}

fn encode_token_run_parts_payload(parts: &[TokenSpanPart]) -> PactResult<(usize, Vec<u8>)> {
    let mut grouped = Vec::new();
    let mut group_count = 0usize;
    let mut index = 0usize;
    while index < parts.len() {
        match parts[index] {
            TokenSpanPart::DenseTokenId(_) => {
                let start = index;
                while index < parts.len() && matches!(parts[index], TokenSpanPart::DenseTokenId(_))
                {
                    index += 1;
                }
                let run = &parts[start..index];
                group_count += 1;
                if run.len() == 1 {
                    if let TokenSpanPart::DenseTokenId(id) = run[0] {
                        grouped.push(TOKEN_SPAN_PART_DENSE_TOKEN_ID);
                        write_varint(&mut grouped, id);
                    }
                } else {
                    grouped.push(TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN);
                    write_len(&mut grouped, run.len())?;
                    for part in run {
                        if let TokenSpanPart::DenseTokenId(id) = *part {
                            write_varint(&mut grouped, id);
                        }
                    }
                }
            }
            TokenSpanPart::TokenId(_) => {
                let start = index;
                while index < parts.len() && matches!(parts[index], TokenSpanPart::TokenId(_)) {
                    index += 1;
                }
                let run = &parts[start..index];
                group_count += 1;
                if run.len() == 1 {
                    if let TokenSpanPart::TokenId(token) = run[0] {
                        grouped.push(TOKEN_SPAN_PART_TOKEN_ID);
                        write_varint(&mut grouped, u64::from(token));
                    }
                } else {
                    grouped.push(TOKEN_SPAN_PART_TOKEN_ID_RUN);
                    write_len(&mut grouped, run.len())?;
                    for part in run {
                        if let TokenSpanPart::TokenId(token) = *part {
                            write_varint(&mut grouped, u64::from(token));
                        }
                    }
                }
            }
            _ => {
                return Err(PactError::codec(
                    "token run encoder received non-token part",
                ));
            }
        }
    }

    let mut best_count = group_count;
    let mut best = grouped;
    let mixed = encode_mixed_token_run_payload(parts)?;
    if mixed.len() < best.len() {
        best_count = 1;
        best = mixed;
    }
    if let Some(table) = encode_token_table_run_payload(parts)?
        && table.len().saturating_mul(2) < best.len()
    {
        best_count = 1;
        best = table;
    }
    Ok((best_count, best))
}

fn encode_mixed_token_run_payload(parts: &[TokenSpanPart]) -> PactResult<Vec<u8>> {
    let bitset_len = parts.len().div_ceil(8);
    let mut bitset = vec![0_u8; bitset_len];
    let mut values = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match *part {
            TokenSpanPart::DenseTokenId(id) => {
                bitset[index / 8] |= 1 << (index % 8);
                write_varint(&mut values, id);
            }
            TokenSpanPart::TokenId(token) => write_varint(&mut values, u64::from(token)),
            _ => {
                return Err(PactError::codec(
                    "mixed token run encoder received non-token part",
                ));
            }
        }
    }
    let mut encoded = Vec::new();
    encoded.push(TOKEN_SPAN_PART_TOKEN_MIXED_RUN);
    write_len(&mut encoded, parts.len())?;
    encoded.extend_from_slice(&bitset);
    encoded.extend_from_slice(&values);
    Ok(encoded)
}

fn encode_token_table_run_payload(parts: &[TokenSpanPart]) -> PactResult<Option<Vec<u8>>> {
    if parts.len() < 8
        || !parts
            .iter()
            .all(|part| matches!(part, TokenSpanPart::TokenId(_)))
    {
        return Ok(None);
    }
    let mut vocab = Vec::<Rank>::new();
    let mut vocab_index = BTreeMap::<Rank, usize>::new();
    for part in parts {
        let TokenSpanPart::TokenId(token) = *part else {
            return Ok(None);
        };
        if !vocab_index.contains_key(&token) {
            let index = vocab_index.len();
            vocab_index.insert(token, index);
            vocab.push(token);
        }
    }
    let width = if vocab.len() <= u8::MAX as usize + 1 {
        1_u8
    } else if vocab.len() <= u16::MAX as usize + 1 {
        2_u8
    } else {
        return Ok(None);
    };

    let mut encoded = Vec::new();
    encoded.push(TOKEN_SPAN_PART_TOKEN_TABLE_RUN);
    write_len(&mut encoded, parts.len())?;
    write_len(&mut encoded, vocab.len())?;
    encoded.push(width);
    for token in vocab {
        write_varint(&mut encoded, u64::from(token));
    }
    for part in parts {
        let TokenSpanPart::TokenId(token) = *part else {
            return Ok(None);
        };
        let Some(index) = vocab_index.get(&token).copied() else {
            return Ok(None);
        };
        match width {
            1 => encoded.push(index as u8),
            2 => encoded.extend_from_slice(&(index as u16).to_le_bytes()),
            _ => return Ok(None),
        }
    }
    Ok(Some(encoded))
}

fn best_token_span_parts(
    value: &str,
    tokens: &[Rank],
    symbols: &SymbolTable,
) -> Vec<TokenSpanPart> {
    let english_only = ActiveLexPacks { shell: false };
    let active_packs = active_lex_packs(value);
    let mut candidates = vec![
        encode_token_span_parts(tokens, symbols, english_only, false),
        encode_token_span_parts(tokens, symbols, english_only, true),
    ];
    if active_packs.shell {
        candidates.push(encode_token_span_parts(
            tokens,
            symbols,
            active_packs,
            false,
        ));
        candidates.push(encode_token_span_parts(tokens, symbols, active_packs, true));
    }
    candidates
        .into_iter()
        .min_by_key(|parts| {
            encode_token_span_parts_payload(parts)
                .map(|payload| payload.len())
                .unwrap_or(usize::MAX)
        })
        .unwrap_or_default()
}

fn encode_token_span_parts(
    tokens: &[Rank],
    symbols: &SymbolTable,
    active_packs: ActiveLexPacks,
    use_dense_tokens: bool,
) -> Vec<TokenSpanPart> {
    let mut parts = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some((span_id, token_count)) = symbols.token_span_id_at(tokens, index) {
            parts.push(TokenSpanPart::SpanRef(span_id));
            index += token_count;
        } else if let Some(entry) = match_installed_lex_entry(tokens, index, active_packs) {
            parts.push(TokenSpanPart::LexRef(entry.lexical_id));
            index += entry.token_ids.len();
        } else if use_dense_tokens && let Some(dense_id) = symbols.dense_token_id(tokens[index]) {
            if ref_cost(dense_id) < token_id_cost(tokens[index]) {
                parts.push(TokenSpanPart::DenseTokenId(dense_id));
            } else {
                parts.push(TokenSpanPart::TokenId(tokens[index]));
            }
            index += 1;
        } else {
            parts.push(TokenSpanPart::TokenId(tokens[index]));
            index += 1;
        }
    }
    parts
}

fn decode_token_span_string(
    bytes: &[u8],
    offset: &mut usize,
    symbols: &SymbolTable,
) -> PactResult<String> {
    let tokenizer_id = read_varint(bytes, offset)?;
    if tokenizer_id != TOKEN_SPAN_TOKENIZER_O200K_BASE_ID {
        return Err(PactError::codec(format!(
            "unsupported token-span tokenizer id {tokenizer_id}"
        )));
    }
    let part_count = read_len(bytes, offset)?;
    let mut tokens = Vec::<Rank>::new();
    let mut output = Vec::<u8>::new();
    for _ in 0..part_count {
        let part_tag = read_byte(bytes, offset)?;
        match part_tag {
            TOKEN_SPAN_PART_TOKEN_ID => {
                let token = read_varint(bytes, offset)?;
                let token = Rank::try_from(token).map_err(|_| {
                    PactError::codec(format!("token-span token id {token} exceeds u32"))
                })?;
                tokens.push(token);
            }
            TOKEN_SPAN_PART_TOKEN_ID_RUN => {
                let len = read_len(bytes, offset)?;
                for _ in 0..len {
                    let token = read_varint(bytes, offset)?;
                    let token = Rank::try_from(token).map_err(|_| {
                        PactError::codec(format!("token-span token id {token} exceeds u32"))
                    })?;
                    tokens.push(token);
                }
            }
            TOKEN_SPAN_PART_DENSE_TOKEN_ID => {
                let dense_id = read_varint(bytes, offset)?;
                let token = symbols.dense_token(dense_id).ok_or_else(|| {
                    PactError::codec(format!("missing dense token id {dense_id}"))
                })?;
                tokens.push(token);
            }
            TOKEN_SPAN_PART_DENSE_TOKEN_ID_RUN => {
                let len = read_len(bytes, offset)?;
                for _ in 0..len {
                    let dense_id = read_varint(bytes, offset)?;
                    let token = symbols.dense_token(dense_id).ok_or_else(|| {
                        PactError::codec(format!("missing dense token id {dense_id}"))
                    })?;
                    tokens.push(token);
                }
            }
            TOKEN_SPAN_PART_TOKEN_MIXED_RUN => {
                let len = read_len(bytes, offset)?;
                let bitset_len = len.div_ceil(8);
                let bitset = bytes
                    .get(*offset..offset.saturating_add(bitset_len))
                    .ok_or_else(|| PactError::codec("truncated token-span mixed token bitset"))?;
                *offset += bitset_len;
                for index in 0..len {
                    let value = read_varint(bytes, offset)?;
                    if bitset[index / 8] & (1 << (index % 8)) != 0 {
                        let token = symbols.dense_token(value).ok_or_else(|| {
                            PactError::codec(format!("missing dense token id {value}"))
                        })?;
                        tokens.push(token);
                    } else {
                        let token = Rank::try_from(value).map_err(|_| {
                            PactError::codec(format!("token-span token id {value} exceeds u32"))
                        })?;
                        tokens.push(token);
                    }
                }
            }
            TOKEN_SPAN_PART_TOKEN_TABLE_RUN => {
                let len = read_len(bytes, offset)?;
                let vocab_len = read_len(bytes, offset)?;
                let width = read_byte(bytes, offset)?;
                if !matches!(width, 1 | 2) {
                    return Err(PactError::codec(format!(
                        "unsupported token table index width {width}"
                    )));
                }
                let mut vocab = Vec::with_capacity(vocab_len);
                for _ in 0..vocab_len {
                    let token = read_varint(bytes, offset)?;
                    let token = Rank::try_from(token).map_err(|_| {
                        PactError::codec(format!("token-span token id {token} exceeds u32"))
                    })?;
                    vocab.push(token);
                }
                for _ in 0..len {
                    let index = match width {
                        1 => usize::from(read_byte(bytes, offset)?),
                        2 => {
                            let bytes = read_array::<2>(bytes, offset)?;
                            usize::from(u16::from_le_bytes(bytes))
                        }
                        _ => unreachable!(),
                    };
                    let token = vocab.get(index).copied().ok_or_else(|| {
                        PactError::codec(format!("token table index {index} out of bounds"))
                    })?;
                    tokens.push(token);
                }
            }
            TOKEN_SPAN_PART_UTF8_ESCAPE => {
                if !tokens.is_empty() {
                    let decoded = o200k_base_singleton()
                        .decode_bytes(&tokens)
                        .map_err(|err| {
                            PactError::codec(format!(
                                "failed to decode token-span token ids: {err}"
                            ))
                        })?;
                    output.extend(decoded);
                    tokens.clear();
                }
                output.extend_from_slice(read_len_prefixed_bytes(bytes, offset)?);
            }
            TOKEN_SPAN_PART_LEX_REF => {
                if !tokens.is_empty() {
                    let decoded = o200k_base_singleton()
                        .decode_bytes(&tokens)
                        .map_err(|err| {
                            PactError::codec(format!(
                                "failed to decode token-span token ids: {err}"
                            ))
                        })?;
                    output.extend(decoded);
                    tokens.clear();
                }
                let lexical_id = read_varint(bytes, offset)?;
                let entry = installed_lex_entry(lexical_id).ok_or_else(|| {
                    PactError::codec(format!("missing installed lexpack entry {lexical_id}"))
                })?;
                output.extend_from_slice(entry.text.as_bytes());
            }
            TOKEN_SPAN_PART_LEX_REF_RUN => {
                if !tokens.is_empty() {
                    let decoded = o200k_base_singleton()
                        .decode_bytes(&tokens)
                        .map_err(|err| {
                            PactError::codec(format!(
                                "failed to decode token-span token ids: {err}"
                            ))
                        })?;
                    output.extend(decoded);
                    tokens.clear();
                }
                let len = read_len(bytes, offset)?;
                for _ in 0..len {
                    let lexical_id = read_varint(bytes, offset)?;
                    let entry = installed_lex_entry(lexical_id).ok_or_else(|| {
                        PactError::codec(format!("missing installed lexpack entry {lexical_id}"))
                    })?;
                    output.extend_from_slice(entry.text.as_bytes());
                }
            }
            TOKEN_SPAN_PART_SPAN_REF => {
                if !tokens.is_empty() {
                    let decoded = o200k_base_singleton()
                        .decode_bytes(&tokens)
                        .map_err(|err| {
                            PactError::codec(format!(
                                "failed to decode token-span token ids: {err}"
                            ))
                        })?;
                    output.extend(decoded);
                    tokens.clear();
                }
                let span_id = read_varint(bytes, offset)?;
                let span = symbols.token_span_tokens(span_id).ok_or_else(|| {
                    PactError::codec(format!("missing token-span dictionary entry {span_id}"))
                })?;
                let decoded = o200k_base_singleton().decode_bytes(span).map_err(|err| {
                    PactError::codec(format!("failed to decode token-span span {span_id}: {err}"))
                })?;
                output.extend(decoded);
            }
            TOKEN_SPAN_PART_SPAN_REF_RUN => {
                if !tokens.is_empty() {
                    let decoded = o200k_base_singleton()
                        .decode_bytes(&tokens)
                        .map_err(|err| {
                            PactError::codec(format!(
                                "failed to decode token-span token ids: {err}"
                            ))
                        })?;
                    output.extend(decoded);
                    tokens.clear();
                }
                let len = read_len(bytes, offset)?;
                for _ in 0..len {
                    let span_id = read_varint(bytes, offset)?;
                    let span = symbols.token_span_tokens(span_id).ok_or_else(|| {
                        PactError::codec(format!("missing token-span dictionary entry {span_id}"))
                    })?;
                    let decoded = o200k_base_singleton().decode_bytes(span).map_err(|err| {
                        PactError::codec(format!(
                            "failed to decode token-span span {span_id}: {err}"
                        ))
                    })?;
                    output.extend(decoded);
                }
            }
            _ => {
                return Err(PactError::codec(format!(
                    "unknown token-span string part tag 0x{part_tag:02x}"
                )));
            }
        }
    }
    if !tokens.is_empty() {
        let decoded = o200k_base_singleton()
            .decode_bytes(&tokens)
            .map_err(|err| {
                PactError::codec(format!("failed to decode token-span token ids: {err}"))
            })?;
        output.extend(decoded);
    }
    String::from_utf8(output)
        .map_err(|err| PactError::codec(format!("invalid token-span UTF-8 string: {err}")))
}

fn match_installed_lex_entry(
    tokens: &[Rank],
    index: usize,
    active_packs: ActiveLexPacks,
) -> Option<&'static InstalledLexEntry> {
    let first = *tokens.get(index)?;
    installed_lex_index()
        .get(&first)?
        .iter()
        .filter_map(|entry_index| installed_lex_entries().get(*entry_index))
        .filter(|entry| active_packs.includes(entry.pack))
        .filter(|entry| {
            let end = index + entry.token_ids.len();
            end <= tokens.len() && tokens[index..end] == *entry.token_ids.as_slice()
        })
        .filter(|entry| ref_cost(entry.lexical_id) < token_sequence_cost(&entry.token_ids))
        .max_by_key(|entry| (entry.token_ids.len(), entry.text.len()))
}

fn installed_lex_entry(lexical_id: u64) -> Option<&'static InstalledLexEntry> {
    let index = usize::try_from(lexical_id).ok()?;
    installed_lex_entries().get(index)
}

fn installed_lex_entries() -> &'static [InstalledLexEntry] {
    static ENTRIES: OnceLock<Vec<InstalledLexEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = Vec::new();
        for (pack, texts) in [
            (LexPackKind::English, ENGLISH_LEXPACK_TEXTS),
            (LexPackKind::Shell, SHELL_LEXPACK_TEXTS),
        ] {
            for text in texts {
                let token_ids = o200k_base_singleton().encode_ordinary(text);
                if token_ids.is_empty() {
                    continue;
                }
                let Ok(decoded) = o200k_base_singleton().decode(&token_ids) else {
                    continue;
                };
                if decoded != *text {
                    continue;
                }
                let lexical_id = entries.len() as u64;
                entries.push(InstalledLexEntry {
                    lexical_id,
                    pack,
                    text,
                    token_ids,
                });
            }
        }
        entries
    })
}

fn installed_lex_index() -> &'static HashMap<Rank, Vec<usize>> {
    static INDEX: OnceLock<HashMap<Rank, Vec<usize>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = HashMap::<Rank, Vec<usize>>::new();
        for (entry_index, entry) in installed_lex_entries().iter().enumerate() {
            if ref_cost(entry.lexical_id) >= token_sequence_cost(&entry.token_ids) {
                continue;
            }
            if let Some(first) = entry.token_ids.first().copied() {
                index.entry(first).or_default().push(entry_index);
            }
        }
        for entries in index.values_mut() {
            entries.sort_by(|left, right| {
                let left = &installed_lex_entries()[*left];
                let right = &installed_lex_entries()[*right];
                right
                    .token_ids
                    .len()
                    .cmp(&left.token_ids.len())
                    .then_with(|| right.text.len().cmp(&left.text.len()))
                    .then_with(|| left.lexical_id.cmp(&right.lexical_id))
            });
        }
        index
    })
}

fn active_lex_packs(value: &str) -> ActiveLexPacks {
    ActiveLexPacks {
        shell: looks_shell_like(value),
    }
}

fn looks_shell_like(value: &str) -> bool {
    let trimmed = value.trim_start();
    value.contains(" && ")
        || value.contains(" || ")
        || value.contains(" | ")
        || value.contains(" 2>&1")
        || value.contains("/dev/null")
        || value.contains(" --")
        || value.contains("$(")
        || value.contains("${")
        || value.contains("/tmp/")
        || value.contains("./")
        || starts_with_any(
            trimmed,
            &[
                "git ", "cargo ", "python ", "python3 ", "pip ", "poetry ", "npm ", "pnpm ",
                "yarn ", "node ", "docker ", "kubectl ", "rg ", "grep ", "find ", "sed ", "awk ",
                "cat ", "ls ", "cd ", "cp ", "mv ", "rm ", "mkdir ", "curl ", "wget ", "ssh ",
                "sudo ",
            ],
        )
}

fn starts_with_any(value: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| value.starts_with(prefix))
}

fn token_id_cost(token_id: Rank) -> usize {
    1 + varint_len(u64::from(token_id))
}

fn token_sequence_cost(tokens: &[Rank]) -> usize {
    tokens.iter().map(|token| token_id_cost(*token)).sum()
}

fn ref_cost(ref_id: u64) -> usize {
    1 + varint_len(ref_id)
}

fn varint_len(mut value: u64) -> usize {
    let mut size = 1usize;
    value >>= 7;
    while value != 0 {
        size += 1;
        value >>= 7;
    }
    size
}

fn binary_bytes_from_sentinel(
    object: &serde_json::Map<String, Value>,
) -> PactResult<Option<Vec<u8>>> {
    if object.len() != 1 {
        return Ok(None);
    }
    let Some(value) = object.get(BINARY_BYTES_SENTINEL) else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(PactError::invalid(
            "binary bytes sentinel must contain a byte array",
        ));
    };
    let mut bytes = Vec::with_capacity(items.len());
    for item in items {
        let byte = item
            .as_u64()
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| PactError::invalid("binary bytes sentinel contains non-byte value"))?;
        bytes.push(byte);
    }
    Ok(Some(bytes))
}

fn write_len_prefixed_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> PactResult<()> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

pub(super) fn write_len(out: &mut Vec<u8>, len: usize) -> PactResult<()> {
    let len = u64::try_from(len)
        .map_err(|_| PactError::codec(format!("value length {len} exceeds u64")))?;
    write_varint(out, len);
    Ok(())
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn zigzag_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag_i64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn read_byte(bytes: &[u8], offset: &mut usize) -> PactResult<u8> {
    let start = *offset;
    let byte = *bytes
        .get(start)
        .ok_or_else(|| PactError::codec(format!("unexpected end of payload at offset {start}")))?;
    *offset += 1;
    Ok(byte)
}

pub(super) fn read_len(bytes: &[u8], offset: &mut usize) -> PactResult<usize> {
    let len = read_varint(bytes, offset)?;
    usize::try_from(len).map_err(|_| PactError::codec(format!("value length {len} exceeds usize")))
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> PactResult<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        if shift >= 64 {
            return Err(PactError::codec("varint length exceeds u64"));
        }
        let byte = read_byte(bytes, offset)?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

fn read_len_prefixed_bytes<'a>(bytes: &'a [u8], offset: &mut usize) -> PactResult<&'a [u8]> {
    let len = read_len(bytes, offset)?;
    let start = *offset;
    let end = start
        .checked_add(len)
        .ok_or_else(|| PactError::codec("payload length overflow"))?;
    let value = bytes.get(start..end).ok_or_else(|| {
        PactError::codec(format!(
            "unexpected end of payload at offset {start} for {len} bytes"
        ))
    })?;
    *offset = end;
    Ok(value)
}

fn read_array<const N: usize>(bytes: &[u8], offset: &mut usize) -> PactResult<[u8; N]> {
    let start = *offset;
    let end = start
        .checked_add(N)
        .ok_or_else(|| PactError::codec("payload offset overflow"))?;
    let value = bytes.get(start..end).ok_or_else(|| {
        PactError::codec(format!(
            "unexpected end of payload at offset {start} for {N} bytes"
        ))
    })?;
    *offset = end;
    value
        .try_into()
        .map_err(|_| PactError::codec("failed to read fixed-width payload value"))
}
