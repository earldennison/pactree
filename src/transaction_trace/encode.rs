use super::TransactionTrace;
use super::trace_op::{TraceOp, TraceOpKind};
use crate::error::{PactError, PactResult};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub(crate) const MAGIC: &[u8; 4] = b"PTTR";

pub(crate) const FORMAT_VERSION: u8 = 2;

pub(crate) mod opcode {

    pub(crate) const DELETE_COMPONENT: u8 = 1;
    pub const MOVE_COMPONENT: u8 = 2;
    pub const SET_FIELD: u8 = 3;
    pub const UNSET_FIELD: u8 = 4;
    pub const SET_ATTR: u8 = 5;
    pub const UNSET_ATTR: u8 = 6;
    pub const SET_CONTENT: u8 = 7;
    pub const REPLACE_CONTENT_RANGE: u8 = 8;
    pub const SET_LIFECYCLE: u8 = 9;

    pub const INSERT_COMPONENT: u8 = 10;
}

pub(crate) mod content_flags {
    pub const CONTENT: u8 = 0b001;
    pub const CONTENT_REF: u8 = 0b010;
    pub const PREVIEW: u8 = 0b100;
}

pub(crate) mod insert_flags {
    pub const HAS_CHILDREN: u8 = 0b001;
}

pub(crate) const WELL_KNOWN: &[&str] = &[
    "id",
    "componentType",
    "type",
    "content",
    "content_ref",
    "content_preview",
    "children",
    "attrs",
    "offset",
    "parent_id",
    "pact_depth",
    "depth",
    "role",
    "lifecycle",
    "tool_name",
    "tool_call_id",
    "success",
    "status",
    "result",
    "error_message",
    "ttl",
    "cad",
    "key",
    "tags",
    "root",
];

struct SymbolTable {
    added: Vec<String>,
    index: HashMap<String, u32>,
}

impl SymbolTable {
    fn new() -> Self {
        let mut index = HashMap::with_capacity(WELL_KNOWN.len());
        for (i, symbol) in WELL_KNOWN.iter().enumerate() {
            index.insert((*symbol).to_string(), i as u32);
        }
        Self {
            added: Vec::new(),
            index,
        }
    }

    fn intern(&mut self, value: &str) -> u32 {
        if let Some(idx) = self.index.get(value) {
            return *idx;
        }
        let idx = WELL_KNOWN.len() as u32 + self.added.len() as u32;
        self.added.push(value.to_string());
        self.index.insert(value.to_string(), idx);
        idx
    }

    fn intern_value(&mut self, value: &Value) -> u32 {
        self.intern(&serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()))
    }
}

pub(crate) fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint(data: &[u8], pos: &mut usize) -> PactResult<u64> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data
            .get(*pos)
            .ok_or_else(|| PactError::codec("unexpected end of trace bytes"))?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return Err(PactError::codec("varint overflow"));
        }
    }
    Ok(value)
}

pub fn encode_transaction_trace(trace: &TransactionTrace) -> Vec<u8> {
    let mut symbols = SymbolTable::new();
    let mut body = Vec::new();
    for op in &trace.ops {
        encode_op(op, &mut symbols, &mut body);
    }

    let mut out = Vec::with_capacity(body.len() + symbols.added.len() * 8 + 16);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    write_varint(&mut out, trace.from_rev);
    write_varint(&mut out, trace.to_rev);

    write_varint(&mut out, symbols.added.len() as u64);
    for symbol in &symbols.added {
        write_varint(&mut out, symbol.len() as u64);
        out.extend_from_slice(symbol.as_bytes());
    }
    write_varint(&mut out, trace.ops.len() as u64);
    out.extend_from_slice(&body);
    out
}

fn encode_op(op: &TraceOp, symbols: &mut SymbolTable, body: &mut Vec<u8>) {
    let node = symbols.intern(&op.component_id);
    match &op.kind {
        TraceOpKind::InsertComponent {
            parent_id,
            index,
            component,
        } => {
            let parent = symbols.intern(parent_id);
            body.push(opcode::INSERT_COMPONENT);
            write_varint(body, node as u64);
            write_varint(body, parent as u64);
            write_varint(body, *index as u64);
            encode_insert_component(component, symbols, body);
        }
        TraceOpKind::DeleteComponent => {
            body.push(opcode::DELETE_COMPONENT);
            write_varint(body, node as u64);
        }
        TraceOpKind::MoveComponent { parent_id, index } => {
            let parent = symbols.intern(parent_id);
            body.push(opcode::MOVE_COMPONENT);
            write_varint(body, node as u64);
            write_varint(body, parent as u64);
            write_varint(body, *index as u64);
        }
        TraceOpKind::SetField { key, value } => {
            let key = symbols.intern(key);
            let value = symbols.intern_value(value);
            body.push(opcode::SET_FIELD);
            write_varint(body, node as u64);
            write_varint(body, key as u64);
            write_varint(body, value as u64);
        }
        TraceOpKind::UnsetField { key } => {
            let key = symbols.intern(key);
            body.push(opcode::UNSET_FIELD);
            write_varint(body, node as u64);
            write_varint(body, key as u64);
        }
        TraceOpKind::SetAttr { key, value } => {
            let key = symbols.intern(key);
            let value = symbols.intern_value(value);
            body.push(opcode::SET_ATTR);
            write_varint(body, node as u64);
            write_varint(body, key as u64);
            write_varint(body, value as u64);
        }
        TraceOpKind::UnsetAttr { key } => {
            let key = symbols.intern(key);
            body.push(opcode::UNSET_ATTR);
            write_varint(body, node as u64);
            write_varint(body, key as u64);
        }
        TraceOpKind::SetContent {
            content,
            content_ref,
            content_preview,
        } => {
            let mut flags = 0u8;
            if content.is_some() {
                flags |= content_flags::CONTENT;
            }
            if content_ref.is_some() {
                flags |= content_flags::CONTENT_REF;
            }
            if content_preview.is_some() {
                flags |= content_flags::PREVIEW;
            }
            body.push(opcode::SET_CONTENT);
            write_varint(body, node as u64);
            body.push(flags);
            if let Some(text) = content {
                let sym = symbols.intern(text);
                write_varint(body, sym as u64);
            }
            if let Some(value) = content_ref {
                let sym = symbols.intern_value(value);
                write_varint(body, sym as u64);
            }
            if let Some(text) = content_preview {
                let sym = symbols.intern(text);
                write_varint(body, sym as u64);
            }
        }
        TraceOpKind::ReplaceContentRange {
            start,
            end,
            replacement,
        } => {
            let sym = symbols.intern(replacement);
            body.push(opcode::REPLACE_CONTENT_RANGE);
            write_varint(body, node as u64);
            write_varint(body, *start as u64);
            write_varint(body, *end as u64);
            write_varint(body, sym as u64);
        }
        TraceOpKind::SetLifecycle { value } => {
            let sym = symbols.intern_value(value);
            body.push(opcode::SET_LIFECYCLE);
            write_varint(body, node as u64);
            write_varint(body, sym as u64);
        }
    }
}

fn encode_insert_component(component_value: &Value, symbols: &mut SymbolTable, body: &mut Vec<u8>) {
    let mut fields = Vec::new();
    let mut attrs = Vec::new();
    let mut flags = 0u8;

    if let Some(object) = component_value.as_object() {
        if object.contains_key("children") {
            flags |= insert_flags::HAS_CHILDREN;
        }
        for (key, value) in object {
            match key.as_str() {
                "id" | "children" => {}
                "attrs" => {
                    if let Some(attr_object) = value.as_object() {
                        for (attr_key, attr_value) in attr_object {
                            attrs
                                .push((symbols.intern(attr_key), symbols.intern_value(attr_value)));
                        }
                    } else {
                        fields.push((symbols.intern(key), symbols.intern_value(value)));
                    }
                }
                _ => fields.push((symbols.intern(key), symbols.intern_value(value))),
            }
        }
    }

    body.push(flags);
    write_varint(body, fields.len() as u64);
    for (key, value) in fields {
        write_varint(body, key as u64);
        write_varint(body, value as u64);
    }
    write_varint(body, attrs.len() as u64);
    for (key, value) in attrs {
        write_varint(body, key as u64);
        write_varint(body, value as u64);
    }
}

pub fn decode_transaction_trace(data: &[u8]) -> PactResult<TransactionTrace> {
    decode_transaction_trace_with(data, false)
}

pub fn decode_transaction_trace_with(
    data: &[u8],
    skip_derived_depth: bool,
) -> PactResult<TransactionTrace> {
    if data.len() < 5 || &data[..4] != MAGIC {
        return Err(PactError::codec("not a binary transaction trace"));
    }
    if data[4] != FORMAT_VERSION {
        return Err(PactError::codec(format!(
            "unsupported trace version: {}",
            data[4]
        )));
    }
    let mut pos = 5;
    let from_rev = read_varint(data, &mut pos)?;
    let to_rev = read_varint(data, &mut pos)?;

    let mut symbols: Vec<String> = WELL_KNOWN.iter().map(|s| (*s).to_string()).collect();

    let symbol_count = read_varint(data, &mut pos)?;
    for _ in 0..symbol_count {
        let len = read_varint(data, &mut pos)? as usize;
        let end = pos
            .checked_add(len)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| PactError::codec("symbol length overruns trace bytes"))?;
        let text = std::str::from_utf8(&data[pos..end])
            .map_err(|_| PactError::codec("symbol is not valid utf-8"))?
            .to_string();
        symbols.push(text);
        pos = end;
    }

    let sym = |idx: u64| -> PactResult<String> {
        symbols
            .get(idx as usize)
            .cloned()
            .ok_or_else(|| PactError::codec(format!("symbol index {idx} out of range")))
    };
    let sym_value = |idx: u64| -> PactResult<Value> {
        let text = symbols
            .get(idx as usize)
            .ok_or_else(|| PactError::codec(format!("symbol index {idx} out of range")))?;
        serde_json::from_str(text)
            .map_err(|err| PactError::codec(format!("invalid encoded value: {err}")))
    };

    let op_count = read_varint(data, &mut pos)?;
    let mut ops = Vec::new();
    for _ in 0..op_count {
        let op = *data
            .get(pos)
            .ok_or_else(|| PactError::codec("unexpected end of trace body"))?;
        pos += 1;
        let component_id = sym(read_varint(data, &mut pos)?)?;
        let kind = match op {
            opcode::INSERT_COMPONENT => {
                let parent_id = sym(read_varint(data, &mut pos)?)?;
                let index = read_varint(data, &mut pos)? as usize;
                let flags = *data
                    .get(pos)
                    .ok_or_else(|| PactError::codec("unexpected end of insert_component flags"))?;
                pos += 1;
                let field_count = read_varint(data, &mut pos)?;
                let mut object = Map::new();
                object.insert("id".to_string(), Value::String(component_id.clone()));
                for _ in 0..field_count {
                    let key = sym(read_varint(data, &mut pos)?)?;
                    let value = sym_value(read_varint(data, &mut pos)?)?;
                    object.insert(key, value);
                }
                let attr_count = read_varint(data, &mut pos)?;
                if attr_count > 0 {
                    let mut attr_object = Map::new();
                    for _ in 0..attr_count {
                        let key = sym(read_varint(data, &mut pos)?)?;
                        let value = sym_value(read_varint(data, &mut pos)?)?;
                        attr_object.insert(key, value);
                    }
                    object.insert("attrs".to_string(), Value::Object(attr_object));
                }
                if flags & insert_flags::HAS_CHILDREN != 0 {
                    object.insert("children".to_string(), Value::Array(Vec::new()));
                }
                TraceOpKind::InsertComponent {
                    parent_id,
                    index,
                    component: Value::Object(object),
                }
            }
            opcode::DELETE_COMPONENT => TraceOpKind::DeleteComponent,
            opcode::MOVE_COMPONENT => {
                let parent_id = sym(read_varint(data, &mut pos)?)?;
                let index = read_varint(data, &mut pos)? as usize;
                TraceOpKind::MoveComponent { parent_id, index }
            }
            opcode::SET_FIELD => {
                let key_index = read_varint(data, &mut pos)?;
                let value_index = read_varint(data, &mut pos)?;
                if skip_derived_depth
                    && symbols.get(key_index as usize).map(String::as_str) == Some("pact_depth")
                {
                    continue;
                }
                let key = sym(key_index)?;
                let value = sym_value(value_index)?;
                TraceOpKind::SetField { key, value }
            }
            opcode::UNSET_FIELD => {
                let key = sym(read_varint(data, &mut pos)?)?;
                TraceOpKind::UnsetField { key }
            }
            opcode::SET_ATTR => {
                let key = sym(read_varint(data, &mut pos)?)?;
                let value = sym_value(read_varint(data, &mut pos)?)?;
                TraceOpKind::SetAttr { key, value }
            }
            opcode::UNSET_ATTR => {
                let key = sym(read_varint(data, &mut pos)?)?;
                TraceOpKind::UnsetAttr { key }
            }
            opcode::SET_CONTENT => {
                let flags = *data
                    .get(pos)
                    .ok_or_else(|| PactError::codec("unexpected end of set_content flags"))?;
                pos += 1;
                let content = if flags & content_flags::CONTENT != 0 {
                    Some(sym(read_varint(data, &mut pos)?)?)
                } else {
                    None
                };
                let content_ref = if flags & content_flags::CONTENT_REF != 0 {
                    Some(sym_value(read_varint(data, &mut pos)?)?)
                } else {
                    None
                };
                let content_preview = if flags & content_flags::PREVIEW != 0 {
                    Some(sym(read_varint(data, &mut pos)?)?)
                } else {
                    None
                };
                TraceOpKind::SetContent {
                    content,
                    content_ref,
                    content_preview,
                }
            }
            opcode::REPLACE_CONTENT_RANGE => {
                let start = read_varint(data, &mut pos)? as usize;
                let end = read_varint(data, &mut pos)? as usize;
                let replacement = sym(read_varint(data, &mut pos)?)?;
                TraceOpKind::ReplaceContentRange {
                    start,
                    end,
                    replacement,
                }
            }
            opcode::SET_LIFECYCLE => {
                let value = sym_value(read_varint(data, &mut pos)?)?;
                TraceOpKind::SetLifecycle { value }
            }
            other => return Err(PactError::codec(format!("unknown trace opcode: {other}"))),
        };
        ops.push(TraceOp { component_id, kind });
    }

    let trace = TransactionTrace {
        from_rev,
        to_rev,
        ops,
    };
    trace.validate()?;
    Ok(trace)
}
