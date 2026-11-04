use super::*;
use crate::error::{PactError, PactResult};

pub struct PactFileBuilder {
    pub(super) records: Vec<()>,
}

impl PactFileBuilder {
    #[cfg(test)]
    pub(crate) fn from_ledger(root_store: &ContextTreeLedger) -> PactResult<Vec<u8>> {
        crate::pact_file::encode_file_baseline(root_store)
    }

    pub(super) fn from_ledger_with_views(
        root_store: &ContextTreeLedger,
        views: &PactViewRecords,
    ) -> PactResult<Vec<u8>> {
        let mut ledger = root_store.clone();
        super::set_v2_view_records(&mut ledger, views)?;
        crate::pact_file::encode_file_baseline(&ledger)
    }

    pub(super) fn push<T: Serialize>(&mut self, _kind: RecordKind, payload: &T) -> PactResult<()> {
        serde_json::to_value(payload)
            .map_err(|error| PactError::codec(format!("encode record facade: {error}")))?;
        self.records.push(());
        Ok(())
    }

    pub(super) fn encode(self) -> PactResult<Vec<u8>> {
        self.encode_with_options(PactValueCodecOptions::default())
    }

    pub(super) fn encode_with_options(
        self,
        _value_options: PactValueCodecOptions,
    ) -> PactResult<Vec<u8>> {
        let _ = self;
        Err(PactError::unsupported(
            "record-page Pact encoding was removed before v0.1",
        ))
    }
}

pub(super) const HOT_QUERY_INDEX_NAME: &str = "pact.hot_query_index.v1";

pub(super) fn encode_index_record(
    name: &str,
    entries: BTreeMap<String, Value>,
    seq: u64,
    symbols: &SymbolTable,
) -> Option<EncodedRecord> {
    let payload = IndexPayload {
        name: name.to_string(),
        entries,
    };
    let mut payload = serde_json::to_value(payload).ok()?;
    prepare_binary_payload_slots(RecordKind::Index, &mut payload).ok()?;
    let payload = compact_canonical_record_payload(RecordKind::Index, payload);
    let payload = encode_pact_value_payload_with_symbols_and_options(
        &payload,
        symbols,
        PactValueCodecOptions {
            token_span_strings: false,
        },
    )
    .ok()?;
    Some(EncodedRecord {
        kind: RecordKind::Index,
        seq,
        payload,
    })
}

pub(super) fn hot_query_shard_key(value: &str) -> String {
    value.chars().take(4).collect()
}

pub(super) fn hot_node_kind(node: &Value) -> Option<&str> {
    node.get("type")
        .and_then(Value::as_str)
        .or_else(|| node.get("componentType").and_then(Value::as_str))
}

pub(super) fn hot_node_depth_keys(node: &Value) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(depth) = node.get("pact_depth").and_then(Value::as_i64) {
        keys.push(format!("d{depth}"));
        keys.push(format!("pact_depth={depth}"));
        keys.push(format!("depth={depth}"));
    }
    if let Some(depth) = node
        .get("aux")
        .or_else(|| node.get("attrs"))
        .and_then(|aux| aux.get("pact_depth").or_else(|| aux.get("depth")))
        .and_then(Value::as_i64)
    {
        keys.push(format!("d{depth}"));
        keys.push(format!("pact_depth={depth}"));
        keys.push(format!("depth={depth}"));
    }
    keys.sort();
    keys.dedup();
    keys
}
