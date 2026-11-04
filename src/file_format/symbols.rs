use super::ScannedRecord;
use super::layout::BUILTIN_SYMBOLS;
use serde_json::Value;
use std::collections::BTreeMap;
use tiktoken_rs::Rank;

#[derive(Clone, Debug)]
pub(super) struct SymbolTable {
    values: Vec<String>,
    index: BTreeMap<String, u32>,
    file_local_id_kinds: BTreeMap<u32, String>,
    token_span_spans: Vec<Vec<Rank>>,
    dense_token_ids: Vec<Rank>,
    dense_token_index: BTreeMap<Rank, u64>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        let values = BUILTIN_SYMBOLS
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        let index = values
            .iter()
            .enumerate()
            .map(|(index, value)| (value.clone(), index as u32))
            .collect();
        Self {
            values,
            index,
            file_local_id_kinds: BTreeMap::new(),
            token_span_spans: Vec::new(),
            dense_token_ids: Vec::new(),
            dense_token_index: BTreeMap::new(),
        }
    }
}

impl SymbolTable {
    pub(super) fn from_symbol_records(records: &[ScannedRecord]) -> Self {
        let mut table = Self::default();
        for record in records.iter().filter(|record| record.kind == "symbol") {
            if let Some(values) = record.payload.get("values").and_then(Value::as_array) {
                for value in values.iter().filter_map(Value::as_str) {
                    if table.index.contains_key(value) {
                        continue;
                    }
                    let id = table.values.len() as u32;
                    table.index.insert(value.to_string(), id);
                    table.values.push(value.to_string());
                }
                let local_ids = record
                    .payload
                    .get("file_local_ids")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let local_kinds = record
                    .payload
                    .get("file_local_id_kinds")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for (index, value) in local_ids.iter().filter_map(Value::as_u64).enumerate() {
                    let Ok(id) = u32::try_from(value) else {
                        continue;
                    };
                    if table.value(id).is_some() {
                        table.file_local_id_kinds.insert(
                            id,
                            local_kinds
                                .get(index)
                                .and_then(Value::as_str)
                                .unwrap_or("id")
                                .to_string(),
                        );
                    }
                }
                for span in record
                    .payload
                    .get("token_span_tokens")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let tokens = span
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_u64)
                        .filter_map(|token| Rank::try_from(token).ok())
                        .collect::<Vec<_>>();
                    if !tokens.is_empty() {
                        table.token_span_spans.push(tokens);
                    }
                }
                for token in record
                    .payload
                    .get("token_span_dense_tokens")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .filter_map(|token| Rank::try_from(token).ok())
                {
                    let dense_id = table.dense_token_ids.len() as u64;
                    table.dense_token_index.insert(token, dense_id);
                    table.dense_token_ids.push(token);
                }
            }
        }
        table
    }

    pub(super) fn symbol_id(&self, value: &str) -> Option<u32> {
        self.index.get(value).copied()
    }

    pub(super) fn file_local_id(&self, value: &str) -> Option<u32> {
        let id = self.symbol_id(value)?;
        self.file_local_id_kinds.contains_key(&id).then_some(id)
    }

    pub(super) fn value(&self, id: u32) -> Option<&str> {
        self.values.get(id as usize).map(String::as_str)
    }

    pub(super) fn token_span_id_at(&self, tokens: &[Rank], index: usize) -> Option<(u64, usize)> {
        let first = *tokens.get(index)?;
        self.token_span_spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.first().copied() == Some(first))
            .filter(|(_, span)| {
                let end = index + span.len();
                end <= tokens.len() && tokens[index..end] == *span.as_slice()
            })
            .max_by_key(|(_, span)| span.len())
            .map(|(span_id, span)| (span_id as u64, span.len()))
    }

    pub(super) fn token_span_tokens(&self, id: u64) -> Option<&[Rank]> {
        self.token_span_spans.get(id as usize).map(Vec::as_slice)
    }

    pub(super) fn dense_token_id(&self, token: Rank) -> Option<u64> {
        self.dense_token_index.get(&token).copied()
    }

    pub(super) fn dense_token(&self, id: u64) -> Option<Rank> {
        self.dense_token_ids.get(id as usize).copied()
    }
}

pub(super) fn string_counts_from_records(records: &[ScannedRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for record in records.iter().filter(|record| record.kind != "symbol") {
        collect_string_counts(&record.payload, &mut counts);
    }
    counts
}

fn collect_string_counts(value: &Value, counts: &mut BTreeMap<String, usize>) {
    match value {
        Value::String(text) => *counts.entry(text.clone()).or_default() += 1,
        Value::Array(items) => {
            for item in items {
                collect_string_counts(item, counts);
            }
        }
        Value::Object(items) => {
            for item in items.values() {
                collect_string_counts(item, counts);
            }
        }
        _ => {}
    }
}

pub(super) fn estimate_symbol_saved_bytes(value: &str, count: usize) -> usize {
    if count < 2 {
        return 0;
    }
    let raw_bytes = count.saturating_mul(5 + value.len());
    let symbol_ref_bytes = count.saturating_mul(5);
    let table_bytes = 5 + value.len();
    raw_bytes.saturating_sub(symbol_ref_bytes + table_bytes)
}
