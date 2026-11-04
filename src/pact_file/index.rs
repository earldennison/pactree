use crate::error::{PactError, PactResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum IndexKey {
    BranchHead {
        branch: String,
    },
    Revision {
        branch: String,
        revision: u64,
    },
    Component {
        branch: String,
        component: String,
    },
    ParentChild {
        branch: String,
        parent: String,
        order: u64,
        child: String,
    },
    Type {
        branch: String,
        component_type: String,
        component: String,
    },
    Aux {
        branch: String,
        key: String,
        value_hash: [u8; 16],
        component: String,
    },
    Tag {
        branch: String,
        tag: String,
        component: String,
    },
    MessageOrder {
        branch: String,
        order: u64,
    },
    ContentId {
        id: String,
    },
    ContentHash {
        hash: [u8; 32],
    },
    TextAddress {
        branch: String,
        revision: u64,
        ordinal: u64,
        component: String,
        field: String,
    },
    Generation {
        generation: u64,
    },
    Metadata,
    ParentChildFacet,
    ComponentPayload {
        segment_id: u64,
        ordinal: u32,
    },
    HeadCheckpoint,
    ComponentBatch {
        segment_id: u64,
    },
    TextChangeFacet,
    SearchChange {
        branch: String,
        revision: u64,
        ordinal: u64,
        component: String,
    },
    SearchChangeFacet,
    SearchSelectorFacet,
    ExtensionOrder {
        branch: String,
        revision: u64,
        ordinal: u64,
    },
    ExtensionKind {
        branch: String,
        kind: String,
        revision: u64,
        ordinal: u64,
    },
    ExtensionKey {
        branch: String,
        kind: String,
        key_hash: [u8; 32],
        revision: u64,
        ordinal: u64,
    },
    ExtensionRecord {
        branch: String,
        record_hash: [u8; 32],
    },
}

impl IndexKey {
    pub(crate) fn encode(&self) -> PactResult<Vec<u8>> {
        let mut out = Vec::new();
        match self {
            Self::BranchHead { branch } => {
                out.push(0);
                write_string(&mut out, branch)?;
            }
            Self::Revision { branch, revision } => {
                out.push(1);
                write_string(&mut out, branch)?;
                out.extend_from_slice(&revision.to_be_bytes());
            }
            Self::Component { branch, component } => {
                out.push(2);
                write_string(&mut out, branch)?;
                out.extend_from_slice(component.as_bytes());
            }
            Self::ParentChild {
                branch,
                parent,
                order,
                child,
            } => {
                out.push(3);
                write_string(&mut out, branch)?;
                write_string(&mut out, parent)?;
                out.extend_from_slice(&order.to_be_bytes());
                write_string(&mut out, child)?;
            }
            Self::Type {
                branch,
                component_type,
                component,
            } => {
                out.push(4);
                write_string(&mut out, branch)?;
                write_string(&mut out, component_type)?;
                out.extend_from_slice(component.as_bytes());
            }
            Self::Aux {
                branch,
                key,
                value_hash,
                component,
            } => {
                out.push(5);
                write_string(&mut out, branch)?;
                write_string(&mut out, key)?;
                out.extend_from_slice(value_hash);
                out.extend_from_slice(component.as_bytes());
            }
            Self::Tag {
                branch,
                tag,
                component,
            } => {
                out.push(6);
                write_string(&mut out, branch)?;
                write_string(&mut out, tag)?;
                out.extend_from_slice(component.as_bytes());
            }
            Self::MessageOrder { branch, order } => {
                out.push(8);
                write_string(&mut out, branch)?;
                out.extend_from_slice(&order.to_be_bytes());
            }
            Self::ContentId { id } => {
                out.push(9);
                write_string(&mut out, id)?;
            }
            Self::ContentHash { hash } => {
                out.push(10);
                out.extend_from_slice(hash);
            }
            Self::TextAddress {
                branch,
                revision,
                ordinal,
                component,
                field,
            } => {
                out.push(11);
                write_string(&mut out, branch)?;
                out.extend_from_slice(&revision.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
                write_string(&mut out, component)?;
                write_string(&mut out, field)?;
            }
            Self::Generation { generation } => {
                out.push(12);
                out.extend_from_slice(&generation.to_be_bytes());
            }
            Self::Metadata => out.push(13),
            Self::ParentChildFacet => out.push(14),
            Self::ComponentPayload {
                segment_id,
                ordinal,
            } => {
                out.push(15);
                out.extend_from_slice(&segment_id.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
            }
            Self::HeadCheckpoint => out.push(16),
            Self::ComponentBatch { segment_id } => {
                out.push(17);
                out.extend_from_slice(&segment_id.to_be_bytes());
            }
            Self::TextChangeFacet => out.push(18),
            Self::SearchChange {
                branch,
                revision,
                ordinal,
                component,
            } => {
                out.push(19);
                write_string(&mut out, branch)?;
                out.extend_from_slice(&revision.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
                out.extend_from_slice(component.as_bytes());
            }
            Self::SearchChangeFacet => out.push(20),
            Self::SearchSelectorFacet => out.push(21),
            Self::ExtensionOrder {
                branch,
                revision,
                ordinal,
            } => {
                out.push(22);
                write_string(&mut out, branch)?;
                out.extend_from_slice(&revision.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
            }
            Self::ExtensionKind {
                branch,
                kind,
                revision,
                ordinal,
            } => {
                out.push(23);
                write_string(&mut out, branch)?;
                write_string(&mut out, kind)?;
                out.extend_from_slice(&revision.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
            }
            Self::ExtensionKey {
                branch,
                kind,
                key_hash,
                revision,
                ordinal,
            } => {
                out.push(24);
                write_string(&mut out, branch)?;
                write_string(&mut out, kind)?;
                out.extend_from_slice(key_hash);
                out.extend_from_slice(&revision.to_be_bytes());
                out.extend_from_slice(&ordinal.to_be_bytes());
            }
            Self::ExtensionRecord {
                branch,
                record_hash,
            } => {
                out.push(25);
                write_string(&mut out, branch)?;
                out.extend_from_slice(record_hash);
            }
        }
        Ok(out)
    }

    pub(crate) fn decode(bytes: &[u8]) -> PactResult<Self> {
        let namespace = *bytes
            .first()
            .ok_or_else(|| PactError::corrupt("empty Pact index key"))?;
        let mut cursor = 1;
        let key = match namespace {
            0 => Self::BranchHead {
                branch: read_string(bytes, &mut cursor)?,
            },
            1 => Self::Revision {
                branch: read_string(bytes, &mut cursor)?,
                revision: read_u64(bytes, &mut cursor)?,
            },
            2 => Self::Component {
                branch: read_string(bytes, &mut cursor)?,
                component: read_remaining_string(bytes, &mut cursor)?,
            },
            3 => Self::ParentChild {
                branch: read_string(bytes, &mut cursor)?,
                parent: read_string(bytes, &mut cursor)?,
                order: read_u64(bytes, &mut cursor)?,
                child: read_string(bytes, &mut cursor)?,
            },
            4 => Self::Type {
                branch: read_string(bytes, &mut cursor)?,
                component_type: read_string(bytes, &mut cursor)?,
                component: read_remaining_string(bytes, &mut cursor)?,
            },
            5 => {
                let branch = read_string(bytes, &mut cursor)?;
                let key = read_string(bytes, &mut cursor)?;
                let mut value_hash = [0_u8; 16];
                value_hash.copy_from_slice(take(bytes, &mut cursor, 16)?);
                Self::Aux {
                    branch,
                    key,
                    value_hash,
                    component: read_remaining_string(bytes, &mut cursor)?,
                }
            }
            6 => Self::Tag {
                branch: read_string(bytes, &mut cursor)?,
                tag: read_string(bytes, &mut cursor)?,
                component: read_remaining_string(bytes, &mut cursor)?,
            },
            8 => Self::MessageOrder {
                branch: read_string(bytes, &mut cursor)?,
                order: read_u64(bytes, &mut cursor)?,
            },
            9 => Self::ContentId {
                id: read_string(bytes, &mut cursor)?,
            },
            10 => {
                let mut hash = [0_u8; 32];
                hash.copy_from_slice(take(bytes, &mut cursor, 32)?);
                Self::ContentHash { hash }
            }
            11 => Self::TextAddress {
                branch: read_string(bytes, &mut cursor)?,
                revision: read_u64(bytes, &mut cursor)?,
                ordinal: read_u64(bytes, &mut cursor)?,
                component: read_string(bytes, &mut cursor)?,
                field: read_string(bytes, &mut cursor)?,
            },
            12 => Self::Generation {
                generation: read_u64(bytes, &mut cursor)?,
            },
            13 => Self::Metadata,
            14 => Self::ParentChildFacet,
            15 => Self::ComponentPayload {
                segment_id: read_u64(bytes, &mut cursor)?,
                ordinal: read_u32(bytes, &mut cursor)?,
            },
            16 => Self::HeadCheckpoint,
            17 => Self::ComponentBatch {
                segment_id: read_u64(bytes, &mut cursor)?,
            },
            18 => Self::TextChangeFacet,
            19 => Self::SearchChange {
                branch: read_string(bytes, &mut cursor)?,
                revision: read_u64(bytes, &mut cursor)?,
                ordinal: read_u64(bytes, &mut cursor)?,
                component: read_remaining_string(bytes, &mut cursor)?,
            },
            20 => Self::SearchChangeFacet,
            21 => Self::SearchSelectorFacet,
            22 => Self::ExtensionOrder {
                branch: read_string(bytes, &mut cursor)?,
                revision: read_u64(bytes, &mut cursor)?,
                ordinal: read_u64(bytes, &mut cursor)?,
            },
            23 => Self::ExtensionKind {
                branch: read_string(bytes, &mut cursor)?,
                kind: read_string(bytes, &mut cursor)?,
                revision: read_u64(bytes, &mut cursor)?,
                ordinal: read_u64(bytes, &mut cursor)?,
            },
            24 => {
                let branch = read_string(bytes, &mut cursor)?;
                let kind = read_string(bytes, &mut cursor)?;
                let mut key_hash = [0_u8; 32];
                key_hash.copy_from_slice(take(bytes, &mut cursor, 32)?);
                Self::ExtensionKey {
                    branch,
                    kind,
                    key_hash,
                    revision: read_u64(bytes, &mut cursor)?,
                    ordinal: read_u64(bytes, &mut cursor)?,
                }
            }
            25 => {
                let branch = read_string(bytes, &mut cursor)?;
                let mut record_hash = [0_u8; 32];
                record_hash.copy_from_slice(take(bytes, &mut cursor, 32)?);
                Self::ExtensionRecord {
                    branch,
                    record_hash,
                }
            }
            _ => {
                return Err(PactError::corrupt(format!(
                    "unknown Pact index namespace: {namespace}"
                )));
            }
        };
        if cursor != bytes.len() {
            return Err(PactError::corrupt("Pact index key has trailing bytes"));
        }
        Ok(key)
    }

    pub(crate) fn type_prefix(branch: &str, component_type: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![4];
        write_string(&mut out, branch)?;
        write_string(&mut out, component_type)?;
        Ok(out)
    }

    pub(crate) fn component_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![2];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn text_address_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![11];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn search_change_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![19];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn parent_child_prefix(branch: &str, parent: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![3];
        write_string(&mut out, branch)?;
        write_string(&mut out, parent)?;
        Ok(out)
    }

    pub(crate) fn component_branch_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![2];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn parent_child_branch_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![3];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn aux_prefix(branch: &str, key: &str, value_hash: [u8; 16]) -> PactResult<Vec<u8>> {
        let mut out = vec![5];
        write_string(&mut out, branch)?;
        write_string(&mut out, key)?;
        out.extend_from_slice(&value_hash);
        Ok(out)
    }

    pub(crate) fn tag_branch_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![6];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn extension_order_prefix(branch: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![22];
        write_string(&mut out, branch)?;
        Ok(out)
    }

    pub(crate) fn extension_kind_prefix(branch: &str, kind: &str) -> PactResult<Vec<u8>> {
        let mut out = vec![23];
        write_string(&mut out, branch)?;
        write_string(&mut out, kind)?;
        Ok(out)
    }

    pub(crate) fn extension_key_hash(value: &str) -> [u8; 32] {
        *blake3::hash(value.as_bytes()).as_bytes()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct HeadMetadata {
    pub(crate) root_id: String,
    #[serde(skip)]
    pub(crate) source_file_identity: String,
    pub(crate) current_branch_id: String,
    pub(crate) branch_count: usize,
    pub(crate) trace_count: usize,
    pub(crate) node_counts: BTreeMap<String, usize>,
    pub(crate) message_counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub(crate) head_revisions: BTreeMap<String, u64>,
    pub(crate) head_transaction_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) stats: BTreeMap<String, crate::store::PactStoreStats>,
    #[serde(default)]
    pub(crate) composition_branches: BTreeSet<String>,

    #[serde(default)]
    pub(crate) derived_depth_branches: BTreeSet<String>,

    #[serde(default)]
    pub(crate) lifecycle_policy_free_branches: BTreeSet<String>,
}

fn holds_lifecycle_policy(component: &crate::components::Component) -> bool {
    !component.lifecycle.policy.is_empty() || component.children.iter().any(holds_lifecycle_policy)
}

impl HeadMetadata {
    pub(crate) fn from_ledger(ledger: &crate::store::ContextTreeLedger) -> Self {
        Self {
            root_id: ledger.root_id.clone(),
            source_file_identity: String::new(),
            current_branch_id: ledger.current_branch_id.clone(),
            branch_count: ledger.branches.len(),
            trace_count: ledger.trace_count(),
            node_counts: ledger
                .branches
                .iter()
                .map(|(branch_id, branch)| {
                    (branch_id.clone(), branch.current.tree.index.by_id.len())
                })
                .collect(),
            message_counts: ledger
                .branches
                .iter()
                .map(|(branch_id, branch)| (branch_id.clone(), branch.current.message_order.len()))
                .collect(),
            head_revisions: ledger
                .branches
                .iter()
                .map(|(branch_id, branch)| (branch_id.clone(), branch.head_revision))
                .collect(),
            head_transaction_ids: ledger
                .branches
                .keys()
                .filter_map(|branch_id| {
                    ledger
                        .event_index
                        .transaction_traces
                        .values()
                        .filter(|trace| &trace.branch_id == branch_id)
                        .filter_map(|trace| {
                            trace
                                .post_request_alteration_ref
                                .revision
                                .map(|revision| (revision, trace.transaction_id.as_str()))
                        })
                        .max_by_key(|(revision, _)| *revision)
                        .map(|(_, transaction_id)| (branch_id.clone(), transaction_id.to_string()))
                })
                .collect(),
            stats: ledger
                .branches
                .iter()
                .map(|(branch_id, branch)| (branch_id.clone(), branch.current.tree.stats.clone()))
                .collect(),
            composition_branches: ledger
                .branches
                .iter()
                .filter(|(_, branch)| ledger.base.is_some() || branch.fork_ref.is_some())
                .map(|(branch_id, _)| branch_id.clone())
                .collect(),
            derived_depth_branches: ledger
                .branches
                .iter()
                .filter(|(_, branch)| branch.derived_depth())
                .map(|(branch_id, _)| branch_id.clone())
                .collect(),
            lifecycle_policy_free_branches: ledger
                .branches
                .iter()
                .filter(|(_, branch)| !holds_lifecycle_policy(&branch.current.tree.root))
                .map(|(branch_id, _)| branch_id.clone())
                .collect(),
        }
    }

    pub(crate) fn from_snapshot(snapshot: &serde_json::Value) -> Option<Self> {
        #[cfg(test)]
        crate::tests::instrumentation::HEAD_METADATA_SNAPSHOT_DECODE_COUNT
            .with(|count| count.set(count.get() + 1));
        serde_json::from_value::<crate::store::ContextTreeLedger>(snapshot.clone())
            .ok()
            .map(|ledger| Self::from_ledger(&ledger))
    }

    pub(crate) fn encode(&self) -> PactResult<Vec<u8>> {
        serde_json::to_vec(self)
            .map_err(|error| PactError::codec(format!("encode Pact head metadata: {error}")))
    }

    pub(crate) fn decode(bytes: &[u8]) -> PactResult<Self> {
        serde_json::from_slice(bytes)
            .map_err(|error| PactError::corrupt(format!("invalid Pact head metadata: {error}")))
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) -> PactResult<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| PactError::invalid("Pact index key string exceeds u32 length"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> PactResult<String> {
    let len = read_u32(bytes, cursor)? as usize;
    let value = take(bytes, cursor, len)?;
    std::str::from_utf8(value)
        .map(str::to_string)
        .map_err(|_| PactError::corrupt("Pact index key string is not UTF-8"))
}

fn read_remaining_string(bytes: &[u8], cursor: &mut usize) -> PactResult<String> {
    let value = bytes
        .get(*cursor..)
        .ok_or_else(|| PactError::corrupt("Pact index key string offset is invalid"))?;
    *cursor = bytes.len();
    std::str::from_utf8(value)
        .map(str::to_string)
        .map_err(|_| PactError::corrupt("Pact index key string is not UTF-8"))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> PactResult<u32> {
    let raw = take(bytes, cursor, 4)?;
    Ok(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> PactResult<u64> {
    let raw = take(bytes, cursor, 8)?;
    Ok(u64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> PactResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| PactError::corrupt("Pact index key length overrun"))?;
    let value = &bytes[*cursor..end];
    *cursor = end;
    Ok(value)
}

pub(crate) struct IndexBatch {
    pub(crate) bytes: Vec<u8>,
    pub(crate) root_address: u64,
}

pub(crate) struct ContentIndexInputs<'a> {
    pub(crate) body_addresses: &'a BTreeMap<[u8; 32], u64>,
    pub(crate) manifest_addresses: &'a BTreeMap<String, u64>,
    pub(crate) legacy_content_id_values: bool,
    pub(crate) component_batch_address: Option<u64>,
    pub(crate) baseline_branches_override: Option<&'a [BaselineBranchOps]>,
    pub(crate) text_changes_override: Option<&'a [super::change_feed::IndexedTextChange]>,
    pub(crate) search_changes_override:
        Option<&'a [super::search_change_feed::IndexedSearchChange]>,
}

#[derive(Clone, Debug)]
pub(crate) struct BaselineBranchOps {
    pub(crate) branch_id: String,
    pub(crate) revision: u64,
    pub(crate) ops: Vec<crate::transaction_trace::TraceOp>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct IndexedComponent {
    pub(crate) revision: u64,
    pub(crate) order: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) child_count: usize,
    pub(crate) value: serde_json::Value,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl IndexedComponent {
    pub(crate) fn decode(bytes: &[u8]) -> PactResult<Self> {
        let mut decompressor = zstd::bulk::Decompressor::new().map_err(|error| {
            PactError::internal(format!("create Pact component decompressor: {error}"))
        })?;
        Self::decode_with(bytes, &mut decompressor)
    }

    pub(crate) fn decode_with(
        bytes: &[u8],
        decompressor: &mut zstd::bulk::Decompressor<'_>,
    ) -> PactResult<Self> {
        if let Some(payload) = bytes.strip_prefix(b"PIC3") {
            return Self::decode_compact(payload);
        }
        if let Some(payload) = bytes.strip_prefix(b"PIC4") {
            let raw_len = payload
                .get(..4)
                .ok_or_else(|| PactError::corrupt("compact indexed component is truncated"))?;
            let raw_len =
                usize::try_from(u32::from_le_bytes(raw_len.try_into().map_err(|_| {
                    PactError::corrupt("compact indexed component length is invalid")
                })?))
                .map_err(|_| PactError::corrupt("indexed component exceeds platform limits"))?;
            if raw_len > 16 * 1024 * 1024 {
                return Err(PactError::corrupt(
                    "decoded indexed component exceeds 16 MiB",
                ));
            }
            let raw = decompressor
                .decompress(&payload[4..], raw_len)
                .map_err(|error| {
                    PactError::corrupt(format!("invalid compact indexed component: {error}"))
                })?;
            return Self::decode_compact(&raw);
        }
        let decoded = if let Some(payload) = bytes.strip_prefix(b"PIC2") {
            let raw_len = payload
                .get(..4)
                .ok_or_else(|| PactError::corrupt("compressed indexed component is truncated"))?;
            let raw_len =
                usize::try_from(u32::from_le_bytes(raw_len.try_into().map_err(|_| {
                    PactError::corrupt("compressed indexed component length is invalid")
                })?))
                .map_err(|_| PactError::corrupt("indexed component exceeds platform limits"))?;
            if raw_len > 16 * 1024 * 1024 {
                return Err(PactError::corrupt(
                    "decoded indexed component exceeds 16 MiB",
                ));
            }
            decompressor
                .decompress(&payload[4..], raw_len)
                .map_err(|error| {
                    PactError::corrupt(format!("invalid compressed indexed component: {error}"))
                })?
        } else {
            bytes.to_vec()
        };
        serde_json::from_slice(&decoded)
            .map_err(|error| PactError::corrupt(format!("invalid indexed Pact component: {error}")))
    }

    fn encode(&self) -> PactResult<Vec<u8>> {
        let raw = self.encode_compact_payload()?;
        let compressed = zstd::bulk::compress(&raw, 3).map_err(|error| {
            PactError::codec(format!("compress indexed Pact component: {error}"))
        })?;
        if compressed.len().saturating_add(8) >= raw.len() {
            let mut encoded = Vec::with_capacity(4 + raw.len());
            encoded.extend_from_slice(b"PIC3");
            encoded.extend_from_slice(&raw);
            return Ok(encoded);
        }
        let mut encoded = Vec::with_capacity(8 + compressed.len());
        encoded.extend_from_slice(b"PIC4");
        encoded.extend_from_slice(
            &u32::try_from(raw.len())
                .map_err(|_| PactError::invalid("indexed component exceeds u32 length"))?
                .to_le_bytes(),
        );
        encoded.extend_from_slice(&compressed);
        Ok(encoded)
    }

    pub(crate) fn encode_for_batch(&self) -> PactResult<Vec<u8>> {
        let raw = self.encode_compact_payload()?;
        let mut encoded = Vec::with_capacity(4 + raw.len());
        encoded.extend_from_slice(b"PIC3");
        encoded.extend_from_slice(&raw);
        Ok(encoded)
    }

    fn encode_compact_payload(&self) -> PactResult<Vec<u8>> {
        let compact = serde_json::json!([
            self.revision,
            self.order,
            self.child_count,
            crate::file_format::compact_indexed_node(self.value.clone()),
        ]);
        serde_json::to_vec(&compact)
            .map_err(|error| PactError::codec(format!("encode indexed Pact component: {error}")))
    }

    fn decode_compact(bytes: &[u8]) -> PactResult<Self> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
            PactError::corrupt(format!("invalid compact indexed Pact component: {error}"))
        })?;
        let fields = value
            .as_array()
            .filter(|fields| fields.len() == 4)
            .ok_or_else(|| PactError::corrupt("compact indexed component has invalid shape"))?;
        Ok(Self {
            revision: fields[0]
                .as_u64()
                .ok_or_else(|| PactError::corrupt("compact component revision is invalid"))?,
            order: serde_json::from_value(fields[1].clone()).map_err(|error| {
                PactError::corrupt(format!("compact component order is invalid: {error}"))
            })?,
            child_count: fields[2]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| PactError::corrupt("compact component child count is invalid"))?,
            value: crate::file_format::expand_indexed_node(fields[3].clone())?,
        })
    }
}

pub(crate) const MESSAGE_ORDER_RANK_BITS: u32 = 16;

pub(crate) fn message_order_key(revision: u64, rank: u64) -> PactResult<u64> {
    if rank >= 1_u64 << MESSAGE_ORDER_RANK_BITS {
        return Err(PactError::unsupported(format!(
            "Pact message order rank {rank} exceeds {MESSAGE_ORDER_RANK_BITS} bits in one transaction"
        )));
    }
    revision
        .checked_mul(1_u64 << MESSAGE_ORDER_RANK_BITS)
        .and_then(|base| base.checked_add(rank))
        .ok_or_else(|| PactError::unsupported("Pact message order revision overflow"))
}

fn extension_index_updates(
    records: &[crate::extension::IndexedExtensionRecord],
    updates: &mut Vec<(IndexKey, Vec<u8>)>,
) -> PactResult<()> {
    for extension in records {
        let branch = extension.branch_id.clone();
        let kind = extension.record.kind.clone();
        let revision = extension.revision;
        let ordinal = extension.ordinal;
        let record_hash = IndexKey::extension_key_hash(&extension.record.record_id);
        let value = serde_json::to_vec(&crate::extension::IndexedExtensionValue {
            branch_id: branch.clone(),
            revision,
            ordinal,
            transaction_id: extension.transaction_id.clone(),
            record: extension.record.clone(),
        })
        .map_err(|error| {
            PactError::codec(format!(
                "failed to encode Pact extension index value: {error}"
            ))
        })?;
        updates.push((
            IndexKey::ExtensionOrder {
                branch: branch.clone(),
                revision,
                ordinal,
            },
            value.clone(),
        ));
        updates.push((
            IndexKey::ExtensionKind {
                branch: branch.clone(),
                kind: kind.clone(),
                revision,
                ordinal,
            },
            value.clone(),
        ));
        let locator = serde_json::to_vec(&crate::extension::IndexedExtensionLocator {
            record_id: extension.record.record_id.clone(),
        })
        .map_err(|error| {
            PactError::codec(format!(
                "failed to encode Pact extension index locator: {error}"
            ))
        })?;
        for key in &extension.record.index_keys {
            updates.push((
                IndexKey::ExtensionKey {
                    branch: branch.clone(),
                    kind: kind.clone(),
                    key_hash: IndexKey::extension_key_hash(key),
                    revision,
                    ordinal,
                },
                locator.clone(),
            ));
        }
        updates.push((
            IndexKey::ExtensionRecord {
                branch,
                record_hash,
            },
            value,
        ));
    }
    Ok(())
}

fn content_index_updates(
    new_content: &[crate::store::ContentRecord],
    body_addresses: &BTreeMap<[u8; 32], u64>,
    manifest_addresses: &BTreeMap<String, u64>,
    legacy_content_id_values: bool,
    updates: &mut Vec<(IndexKey, Vec<u8>)>,
) -> PactResult<()> {
    for content in new_content {
        let payload = if let Some(bytes) = &content.storage.inline_bytes {
            bytes.as_slice()
        } else if let Some(text) = &content.storage.inline {
            text.as_bytes()
        } else {
            continue;
        };
        let full_hash = *blake3::hash(payload).as_bytes();
        let locator = if legacy_content_id_values {
            full_hash.to_vec()
        } else {
            [
                full_hash.as_slice(),
                &manifest_addresses
                    .get(&content.id)
                    .copied()
                    .ok_or_else(|| {
                        PactError::internal("Pact content manifest address was not captured")
                    })?
                    .to_le_bytes(),
            ]
            .concat()
        };
        updates.push((
            IndexKey::ContentId {
                id: content.id.clone(),
            },
            locator,
        ));
    }
    for (hash, body_address) in body_addresses {
        updates.push((
            IndexKey::ContentHash { hash: *hash },
            body_address.to_le_bytes().to_vec(),
        ));
    }

    Ok(())
}

pub(crate) fn build_transaction_index(
    transaction: &super::transaction::PactTransaction,
    generation: u64,
    transaction_address: u64,
    existing_pages: Option<&dyn super::btree::PageReader>,
    existing_root: Option<u64>,
    start_address: u64,
    content_index: ContentIndexInputs<'_>,
) -> PactResult<IndexBatch> {
    let mut pages = StagedPages::new(existing_pages, start_address, generation);
    let mut updates = vec![
        (
            IndexKey::Revision {
                branch: transaction.branch_id.clone(),
                revision: transaction.trace.to_rev,
            },
            transaction_address.to_le_bytes().to_vec(),
        ),
        (
            IndexKey::Generation { generation },
            transaction_address.to_le_bytes().to_vec(),
        ),
        (IndexKey::TextChangeFacet, vec![1]),
        (IndexKey::SearchChangeFacet, vec![1]),
    ];
    if let Some(metadata) = transaction.head_metadata.as_ref() {
        updates.push((IndexKey::Metadata, metadata.encode()?));
    } else {
        updates.push((
            IndexKey::BranchHead {
                branch: transaction.branch_id.clone(),
            },
            transaction.trace.to_rev.to_le_bytes().to_vec(),
        ));
    }
    extension_index_updates(
        &crate::extension::indexed_records_from_transaction(transaction)?,
        &mut updates,
    )?;
    content_index_updates(
        &transaction.new_content,
        content_index.body_addresses,
        content_index.manifest_addresses,
        content_index.legacy_content_id_values,
        &mut updates,
    )?;

    let derived_text_changes = if content_index.text_changes_override.is_none() {
        Some(super::change_feed::indexed_text_changes_from_transaction(
            transaction,
        )?)
    } else {
        None
    };
    for change in content_index
        .text_changes_override
        .or(derived_text_changes.as_deref())
        .into_iter()
        .flatten()
    {
        updates.push((
            IndexKey::TextAddress {
                branch: change.change.branch_id.clone(),
                revision: change.revision,
                ordinal: change.ordinal,
                component: change.change.node_id.clone(),
                field: change.change.field.clone(),
            },
            super::change_feed::encode_index_value(change)?,
        ));
    }

    let derived_search_changes = if content_index.search_changes_override.is_none() {
        Some(super::search_change_feed::indexed_search_changes_from_transaction(transaction)?)
    } else {
        None
    };
    for change in content_index
        .search_changes_override
        .or(derived_search_changes.as_deref())
        .into_iter()
        .flatten()
    {
        updates.push((
            IndexKey::SearchChange {
                branch: transaction.branch_id.clone(),
                revision: change.revision,
                ordinal: change.ordinal,
                component: change.change.component_id.clone(),
            },
            super::search_change_feed::encode_index_value(change)?,
        ));
    }

    let derived_baseline_branches = if content_index.baseline_branches_override.is_none() {
        transaction
            .head_checkpoint
            .as_ref()
            .map(baseline_component_ops)
            .transpose()?
            .map(|ops| {
                vec![BaselineBranchOps {
                    branch_id: transaction.branch_id.clone(),
                    revision: transaction.trace.to_rev,
                    ops,
                }]
            })
    } else {
        None
    };
    let baseline_branches = content_index
        .baseline_branches_override
        .or(derived_baseline_branches.as_deref());
    let tree = if let Some(baseline_branches) = baseline_branches {
        let mut entries = updates
            .into_iter()
            .map(|(key, value)| Ok((key.encode()?, value)))
            .collect::<PactResult<Vec<_>>>()?;
        let mut ordinal_offset = 0_u32;
        for baseline in baseline_branches {
            entries.extend(baseline_index_entries(
                &baseline.branch_id,
                baseline.revision,
                generation,
                &baseline.ops,
                ordinal_offset,
                content_index.component_batch_address.is_some(),
            )?);
            ordinal_offset = ordinal_offset
                .checked_add(u32::try_from(baseline.ops.len()).map_err(|_| {
                    PactError::invalid("baseline component operation count exceeds u32")
                })?)
                .ok_or_else(|| PactError::invalid("baseline component ordinal overflow"))?;
        }
        if let Some(address) = content_index.component_batch_address {
            entries.push((
                IndexKey::ComponentBatch {
                    segment_id: generation,
                }
                .encode()?,
                address.to_le_bytes().to_vec(),
            ));
        }
        if let Some(snapshot) = transaction.head_checkpoint.as_ref()
            && super::head_checkpoint::is_compact_ledger(snapshot)
        {
            let ledger_value = if super::head_checkpoint::is_compact_ledger(snapshot) {
                snapshot.get("ledger").unwrap_or(snapshot)
            } else {
                snapshot
            };
            let ledger: crate::store::ContextTreeLedger =
                serde_json::from_value(ledger_value.clone()).map_err(|error| {
                    PactError::invalid(format!("invalid Pact checkpoint ledger: {error}"))
                })?;
            let mut components = BTreeMap::<String, Vec<_>>::new();
            for (key, value) in &entries {
                if let IndexKey::Component { branch, .. } = IndexKey::decode(key)? {
                    components
                        .entry(branch)
                        .or_default()
                        .push(crate::pact_file::segment::RecordLocator::decode(value)?);
                }
            }
            let checkpoint = super::head_checkpoint::HeadCheckpoint::from_ledger_and_components(
                &ledger, components,
            )?;
            entries.push((
                IndexKey::HeadCheckpoint.encode()?,
                super::head_checkpoint::encode(&checkpoint)?,
            ));
        }
        entries.push((IndexKey::ParentChildFacet.encode()?, vec![1]));
        entries.push((IndexKey::SearchSelectorFacet.encode()?, vec![1]));
        super::btree::CowBTree::bulk_load(&mut pages, entries)?
    } else {
        let mut tree = super::btree::CowBTree::from_root(existing_root);
        let parent_child_facet = tree
            .get(&pages, &IndexKey::ParentChildFacet.encode()?)?
            .is_some();
        for (key, value) in updates {
            (tree, _) = tree.insert(&mut pages, key.encode()?, value)?;
        }
        let mut message_rank = 0_u64;
        for op in &transaction.trace.ops {
            let derived_field = match &op.kind {
                crate::transaction_trace::TraceOpKind::SetLifecycle { .. } => true,
                crate::transaction_trace::TraceOpKind::SetField { key, .. } => key == "pact_depth",
                _ => false,
            };
            if derived_field {
                continue;
            }
            tree = apply_component_update(
                tree,
                &mut pages,
                &transaction.branch_id,
                transaction.trace.to_rev,
                &mut message_rank,
                op,
                parent_child_facet,
            )?;
        }
        tree
    };
    let staged_root = tree
        .root_address()
        .ok_or_else(|| PactError::internal("Pact transaction index did not produce a root"))?;
    let (bytes, root_address) = pages.finish(staged_root)?;
    Ok(IndexBatch {
        bytes,
        root_address,
    })
}

pub(crate) struct AppendIndexInputs<'a> {
    pub(crate) branch_id: &'a str,
    pub(crate) revision: u64,
    pub(crate) generation: u64,
    pub(crate) transaction_address: u64,
    pub(crate) index_start_address: u64,
    pub(crate) existing_root: u64,
    pub(crate) metadata: &'a HeadMetadata,

    pub(crate) ops: &'a [crate::transaction_trace::TraceOp],
    pub(crate) extensions: &'a [crate::extension::IndexedExtensionRecord],
    pub(crate) text_changes: &'a [super::change_feed::IndexedTextChange],
    pub(crate) search_changes: &'a [super::search_change_feed::IndexedSearchChange],
    pub(crate) new_content: &'a [crate::store::ContentRecord],
    pub(crate) body_addresses: &'a BTreeMap<[u8; 32], u64>,
    pub(crate) manifest_addresses: &'a BTreeMap<String, u64>,
    pub(crate) in_flight_bytes: usize,
    pub(crate) io_chunk_bytes: usize,

    pub(crate) dangling_relations: &'a [(String, String)],

    pub(crate) order_repair: Option<&'a MessageOrderRepairPlan>,
}

const REPAIR_PAGE_ENTRIES: usize = 256;

pub(crate) struct MessageOrderRepairPlan {
    pages: super::scratch::ScratchPages,
    rows: super::btree::CowBTree,

    pub(crate) rows_scanned: u64,

    pub(crate) rows_to_reindex: u64,

    pub(crate) stale_keys: u64,
}

impl MessageOrderRepairPlan {
    pub(crate) fn new(options: &crate::memory::PactMemoryOptions) -> PactResult<Self> {
        Ok(Self {
            pages: super::scratch::ScratchPages::create(
                None,
                options.io_chunk_bytes,
                options.scratch_cache_bytes,
            )?,
            rows: super::btree::CowBTree::from_root(None),
            rows_scanned: 0,
            rows_to_reindex: 0,
            stale_keys: 0,
        })
    }

    pub(crate) fn insert(&mut self, component_id: &str, order: u64) -> PactResult<()> {
        let rows = std::mem::replace(&mut self.rows, super::btree::CowBTree::from_root(None));
        let (rows, _) = rows.insert(
            &mut self.pages,
            component_id.as_bytes().to_vec(),
            order.to_le_bytes().to_vec(),
        )?;
        self.rows = rows;
        self.rows_scanned = self.rows_scanned.saturating_add(1);
        Ok(())
    }

    pub(crate) fn get(&self, component_id: &[u8]) -> PactResult<Option<u64>> {
        self.rows
            .get(&self.pages, component_id)?
            .map(|value| super::codec::decode_u64_value(&value, "Pact message order plan value"))
            .transpose()
    }

    pub(crate) fn page(&self, start: &[u8]) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        self.rows
            .range_limit(&self.pages, start, &[0xff], REPAIR_PAGE_ENTRIES, false)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows_to_reindex == 0 && self.stale_keys == 0
    }
}

fn apply_message_order_repair(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    plan: &MessageOrderRepairPlan,
) -> PactResult<super::btree::CowBTree> {
    let message_order = |order: u64| -> PactResult<Vec<u8>> {
        IndexKey::MessageOrder {
            branch: branch.to_string(),
            order,
        }
        .encode()
    };
    let end = message_order(u64::MAX)?;
    let mut start = message_order(0)?;
    loop {
        let page = tree.range_limit(&*pages, &start, &end, REPAIR_PAGE_ENTRIES, false)?;
        let Some((last, _)) = page.last() else {
            break;
        };
        let mut next = last.clone();
        next.push(0);
        for (key, value) in &page {
            let IndexKey::MessageOrder { order, .. } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact message order range returned another namespace",
                ));
            };
            if plan.get(value)?.is_some_and(|wanted| wanted != order) {
                (tree, _, _) = tree.delete(pages, key)?;
            }
        }
        if page.len() < REPAIR_PAGE_ENTRIES {
            break;
        }
        start = next;
    }
    let mut start = Vec::new();
    loop {
        let page = plan.page(&start)?;
        let Some((last, _)) = page.last() else {
            break;
        };
        let mut next = last.clone();
        next.push(0);
        for (id, order) in &page {
            let order = super::codec::decode_u64_value(order, "Pact message order plan value")?;
            let component_id = std::str::from_utf8(id).map_err(|_| {
                PactError::corrupt("Pact message order plan holds a non-UTF-8 component id")
            })?;
            let key = message_order(order)?;
            match tree.get(&*pages, &key)? {
                Some(existing) if existing == *id => continue,
                Some(existing) => {
                    return Err(PactError::corrupt(format!(
                        "Pact message order key {order} is held by {} and wanted by {component_id}",
                        String::from_utf8_lossy(&existing)
                    )));
                }
                None => {}
            }
            let component_key = IndexKey::Component {
                branch: branch.to_string(),
                component: component_id.to_string(),
            };
            let Some(facet) = tree.get(&*pages, &component_key.encode()?)? else {
                continue;
            };
            let (mut indexed, _) = indexed_component_from_facet(&tree, &*pages, &facet)?;
            if let Some(previous) = indexed.order
                && previous != order
            {
                let previous_key = message_order(previous)?;
                if tree.get(&*pages, &previous_key)?.as_deref() == Some(id.as_slice()) {
                    (tree, _, _) = tree.delete(pages, &previous_key)?;
                }
            }
            indexed.order = Some(order);
            tree = write_indexed_component(tree, pages, branch, &component_key, &indexed)?;
            (tree, _) = tree.insert(pages, key, id.clone())?;
        }
        if page.len() < REPAIR_PAGE_ENTRIES {
            break;
        }
        start = next;
    }
    Ok(tree)
}

pub(crate) struct StagedAppendIndex<'a> {
    pages: StagedPages<'a>,
    root: u64,
}

impl StagedAppendIndex<'_> {
    pub(crate) fn root(&self) -> u64 {
        self.root
    }

    pub(crate) fn reader(&self) -> &(impl super::btree::PageReader + '_) {
        &self.pages
    }

    pub(crate) fn staged_page_count(&self) -> usize {
        self.pages.staged_page_count()
    }

    pub(crate) fn spilled(&self) -> bool {
        self.pages.spilled()
    }

    pub(crate) fn finish(self) -> PactResult<IndexBatch> {
        let (bytes, root_address) = self.pages.finish(self.root)?;
        Ok(IndexBatch {
            bytes,
            root_address,
        })
    }

    pub(crate) fn finish_into(self, out: &mut dyn std::io::Write) -> PactResult<(u64, u64)> {
        let mut counting = CountingWriter {
            inner: out,
            written: 0,
        };
        let root = self.pages.finish_into(
            self.root,
            super::codec::INDEX_BATCH_TARGET_PAGES,
            &mut counting,
        )?;
        Ok((counting.written, root))
    }
}

pub(crate) struct CountingWriter<'a> {
    inner: &'a mut dyn std::io::Write,
    written: u64,
}

impl std::io::Write for CountingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) fn stage_rebuilt_index<'a>(
    entries: impl IntoIterator<Item = PactResult<(Vec<u8>, Vec<u8>)>>,
    index_start_address: u64,
    generation: u64,
    in_flight_bytes: usize,
    io_chunk_bytes: usize,
) -> PactResult<StagedAppendIndex<'a>> {
    let mut pages = StagedPages::new_bounded(
        None,
        index_start_address,
        generation,
        in_flight_bytes,
        io_chunk_bytes,
    );
    let tree = super::btree::CowBTree::bulk_load_sorted(&mut pages, entries)?;
    let root = tree
        .root_address()
        .ok_or_else(|| PactError::invalid("Pact index rebuild produced no entries"))?;
    Ok(StagedAppendIndex { pages, root })
}

pub(crate) fn stage_append_index<'a>(
    existing: &'a dyn super::btree::PageReader,
    inputs: AppendIndexInputs<'_>,
) -> PactResult<StagedAppendIndex<'a>> {
    let mut pages = StagedPages::new_bounded(
        Some(existing),
        inputs.index_start_address,
        inputs.generation,
        inputs.in_flight_bytes,
        inputs.io_chunk_bytes,
    );
    let branch = inputs.branch_id;
    let mut updates = vec![
        (
            IndexKey::Revision {
                branch: branch.to_string(),
                revision: inputs.revision,
            },
            inputs.transaction_address.to_le_bytes().to_vec(),
        ),
        (
            IndexKey::Generation {
                generation: inputs.generation,
            },
            inputs.transaction_address.to_le_bytes().to_vec(),
        ),
        (IndexKey::TextChangeFacet, vec![1]),
        (IndexKey::SearchChangeFacet, vec![1]),
        (IndexKey::Metadata, inputs.metadata.encode()?),
    ];
    extension_index_updates(inputs.extensions, &mut updates)?;
    content_index_updates(
        inputs.new_content,
        inputs.body_addresses,
        inputs.manifest_addresses,
        false,
        &mut updates,
    )?;
    for change in inputs.text_changes {
        updates.push((
            IndexKey::TextAddress {
                branch: change.change.branch_id.clone(),
                revision: change.revision,
                ordinal: change.ordinal,
                component: change.change.node_id.clone(),
                field: change.change.field.clone(),
            },
            super::change_feed::encode_index_value(change)?,
        ));
    }
    for change in inputs.search_changes {
        updates.push((
            IndexKey::SearchChange {
                branch: branch.to_string(),
                revision: change.revision,
                ordinal: change.ordinal,
                component: change.change.component_id.clone(),
            },
            super::search_change_feed::encode_index_value(change)?,
        ));
    }

    let mut tree = super::btree::CowBTree::from_root(Some(inputs.existing_root));
    let parent_child_facet = tree
        .get(&pages, &IndexKey::ParentChildFacet.encode()?)?
        .is_some();
    for (key, value) in updates {
        (tree, _) = tree.insert(&mut pages, key.encode()?, value)?;
    }
    if parent_child_facet {
        for (parent, child) in inputs.dangling_relations {
            tree = remove_child_relation(tree, &mut pages, branch, parent, child)?;
        }
    }
    let mut message_rank = 0_u64;
    for op in inputs.ops {
        tree = match &op.kind {
            crate::transaction_trace::TraceOpKind::SetLifecycle { .. } => {
                return Err(PactError::internal(
                    "Pact append index received a derived lifecycle op",
                ));
            }
            crate::transaction_trace::TraceOpKind::SetField { key, .. } if key == "pact_depth" => {
                return Err(PactError::internal(
                    "Pact append index received a derived depth op",
                ));
            }
            crate::transaction_trace::TraceOpKind::InsertComponent {
                parent_id,
                index,
                component,
            } => apply_append_insert(
                tree,
                &mut pages,
                branch,
                inputs.revision,
                &mut message_rank,
                &op.component_id,
                parent_id,
                *index,
                component,
                parent_child_facet,
            )?,
            crate::transaction_trace::TraceOpKind::SetField { key, value }
                if key == "child_ids" =>
            {
                if parent_child_facet {
                    let exact = op.component_id == inputs.metadata.root_id;
                    verify_child_ids_order(&tree, &pages, branch, &op.component_id, value, exact)?;
                }
                apply_component_update(
                    tree,
                    &mut pages,
                    branch,
                    inputs.revision,
                    &mut message_rank,
                    op,
                    parent_child_facet,
                )?
            }
            _ => apply_component_update(
                tree,
                &mut pages,
                branch,
                inputs.revision,
                &mut message_rank,
                op,
                parent_child_facet,
            )?,
        };
    }
    if let Some(plan) = inputs.order_repair {
        tree = apply_message_order_repair(tree, &mut pages, branch, plan)?;
    }
    let root = tree
        .root_address()
        .ok_or_else(|| PactError::internal("Pact append index did not produce a root"))?;
    Ok(StagedAppendIndex { pages, root })
}

const CHILD_ORDER_PAGE_ENTRIES: usize = 256;

fn verify_child_ids_order(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    branch: &str,
    parent: &str,
    value: &serde_json::Value,
    exact: bool,
) -> PactResult<()> {
    let expected = value
        .as_array()
        .ok_or_else(|| PactError::corrupt(format!("Pact child_ids of {parent} is not an array")))?;
    let prefix = IndexKey::parent_child_prefix(branch, parent)?;
    let end = prefix_successor(&prefix)?;
    let mut position = 0_usize;
    let mut start = prefix.clone();
    loop {
        let page = tree.range_limit(pages, &start, &end, CHILD_ORDER_PAGE_ENTRIES, false)?;
        if page.is_empty() {
            break;
        }
        for (key, _) in &page {
            let IndexKey::ParentChild { child, order, .. } = IndexKey::decode(key)? else {
                return Err(PactError::corrupt(
                    "Pact parent-child range returned another namespace",
                ));
            };
            let expected_id = expected
                .get(position)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    PactError::corrupt(format!(
                        "Pact child_ids of {parent} is shorter than its relations"
                    ))
                })?;
            if expected_id != child || order != position as u64 {
                return Err(PactError::corrupt(format!(
                    "Pact child_ids of {parent} disagrees with its relations at ordinal {position}"
                )));
            }
            position += 1;
        }
        let last = page
            .last()
            .map(|(key, _)| key.clone())
            .ok_or_else(|| PactError::internal("non-empty page without a last key"))?;
        start = prefix_successor(&last)?;
        if page.len() < CHILD_ORDER_PAGE_ENTRIES {
            break;
        }
    }
    if exact && position != expected.len() {
        return Err(PactError::corrupt(format!(
            "Pact child_ids of {parent} names {} children but {position} relations exist",
            expected.len()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_append_insert(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    revision: u64,
    message_rank: &mut u64,
    component_id: &str,
    parent_id: &str,
    index: usize,
    component: &serde_json::Value,
    parent_child_facet: bool,
) -> PactResult<super::btree::CowBTree> {
    let component_key = IndexKey::Component {
        branch: branch.to_string(),
        component: component_id.to_string(),
    };
    if tree.get(pages, &component_key.encode()?)?.is_some() {
        return Err(PactError::invalid(format!(
            "Pact component {component_id} is already indexed"
        )));
    }
    let mut value = component.clone();
    let object = value.as_object_mut().ok_or_else(|| {
        PactError::invalid("inserted Pact component index value must be an object")
    })?;
    object
        .entry("id".to_string())
        .or_insert_with(|| serde_json::Value::String(component_id.to_string()));
    let order = if component_type(&value) == Some("message_turn") {
        let order = message_order_key(revision, *message_rank)?;
        *message_rank += 1;
        let key = IndexKey::MessageOrder {
            branch: branch.to_string(),
            order,
        }
        .encode()?;
        if tree.get(pages, &key)?.is_some() {
            return Err(PactError::corrupt(format!(
                "Pact message order key {order} is already present"
            )));
        }
        Some(order)
    } else {
        None
    };
    let indexed = IndexedComponent {
        revision,
        order,
        child_count: 0,
        value,
    };
    tree = write_indexed_component(tree, pages, branch, &component_key, &indexed)?;
    if let Some(order) = order {
        (tree, _) = tree.insert(
            pages,
            IndexKey::MessageOrder {
                branch: branch.to_string(),
                order,
            }
            .encode()?,
            component_id.as_bytes().to_vec(),
        )?;
    }
    if parent_child_facet {
        tree = insert_child_relation_tail(tree, pages, branch, parent_id, index, component_id)?;
    }
    Ok(tree)
}

fn insert_child_relation_tail(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    parent: &str,
    index: usize,
    child: &str,
) -> PactResult<super::btree::CowBTree> {
    let prefix = IndexKey::parent_child_prefix(branch, parent)?;
    let end = prefix_successor(&prefix)?;
    let order_bound = |order: u64| {
        let mut key = prefix.clone();
        key.extend_from_slice(&order.to_be_bytes());
        key
    };
    let index_order = u64::try_from(index)
        .map_err(|_| PactError::invalid("Pact child index exceeds platform limits"))?;
    if index > 0 {
        let previous = tree.range_limit(
            pages,
            &order_bound(index_order - 1),
            &order_bound(index_order),
            1,
            false,
        )?;
        if previous.is_empty() {
            return Err(PactError::corrupt(format!(
                "Pact parent {parent} has no child at ordinal {}",
                index - 1
            )));
        }
    }
    let beyond = tree.range_limit(pages, &order_bound(index_order), &end, 1, false)?;
    if !beyond.is_empty() {
        return Err(PactError::corrupt(format!(
            "Pact parent {parent} already has a child at or beyond ordinal {index}"
        )));
    }
    (tree, _) = tree.insert(
        pages,
        IndexKey::ParentChild {
            branch: branch.to_string(),
            parent: parent.to_string(),
            order: index_order,
            child: child.to_string(),
        }
        .encode()?,
        Vec::new(),
    )?;
    set_indexed_child_count(tree, pages, branch, parent, index + 1)
}

fn baseline_index_entries(
    branch: &str,
    revision: u64,
    generation: u64,
    ops: &[crate::transaction_trace::TraceOp],
    ordinal_offset: u32,
    component_batch_backed: bool,
) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut entries = Vec::new();
    let mut children_by_parent = BTreeMap::<String, Vec<String>>::new();
    for op in ops {
        if let crate::transaction_trace::TraceOpKind::InsertComponent {
            parent_id, index, ..
        } = &op.kind
        {
            let children = children_by_parent.entry(parent_id.clone()).or_default();
            if *index > children.len() {
                return Err(PactError::invalid(
                    "baseline Pact child index exceeds its parent child count",
                ));
            }
            children.insert(*index, op.component_id.clone());
        }
    }
    let mut message_rank = 0_u64;
    for (ordinal, op) in ops.iter().enumerate() {
        let crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } = &op.kind
        else {
            continue;
        };
        let mut value = component.clone();
        let object = value.as_object_mut().ok_or_else(|| {
            PactError::invalid("inserted Pact component index value must be an object")
        })?;
        object
            .entry("id".to_string())
            .or_insert_with(|| serde_json::Value::String(op.component_id.clone()));
        let order = if component_type(&value) == Some("message_turn") {
            let order = message_order_key(revision, message_rank)?;
            message_rank += 1;
            Some(order)
        } else {
            None
        };
        let indexed = IndexedComponent {
            revision,
            order,
            child_count: children_by_parent
                .get(&op.component_id)
                .map(Vec::len)
                .unwrap_or_default(),
            value,
        };
        let locator =
            crate::pact_file::segment::RecordLocator {
                segment_id: generation,
                ordinal: ordinal_offset
                    .checked_add(u32::try_from(ordinal).map_err(|_| {
                        PactError::invalid("baseline component ordinal exceeds u32")
                    })?)
                    .ok_or_else(|| PactError::invalid("baseline component ordinal overflow"))?,
            };
        if !component_batch_backed {
            entries.push((
                IndexKey::ComponentPayload {
                    segment_id: locator.segment_id,
                    ordinal: locator.ordinal,
                }
                .encode()?,
                indexed.encode()?,
            ));
        }
        entries.push((
            IndexKey::Component {
                branch: branch.to_string(),
                component: op.component_id.clone(),
            }
            .encode()?,
            locator.encode().to_vec(),
        ));
        entries.extend(
            component_secondary_entries(branch, &indexed, &locator.encode())?
                .into_iter()
                .map(|(key, value)| Ok((key.encode()?, value)))
                .collect::<PactResult<Vec<_>>>()?,
        );
        if let Some(order) = order {
            entries.push((
                IndexKey::MessageOrder {
                    branch: branch.to_string(),
                    order,
                }
                .encode()?,
                op.component_id.as_bytes().to_vec(),
            ));
        }
    }
    for (parent, children) in children_by_parent {
        for (order, child) in children.into_iter().enumerate() {
            entries.push((
                IndexKey::ParentChild {
                    branch: branch.to_string(),
                    parent: parent.clone(),
                    order: order as u64,
                    child,
                }
                .encode()?,
                Vec::new(),
            ));
        }
    }
    Ok(entries)
}

pub(crate) fn baseline_component_payloads(
    transaction: &super::transaction::PactTransaction,
    generation: u64,
) -> PactResult<Vec<(u32, Vec<u8>)>> {
    let snapshot = transaction
        .head_checkpoint
        .as_ref()
        .ok_or_else(|| PactError::invalid("Pact component batch requires a baseline snapshot"))?;
    let ops = baseline_component_ops(snapshot)?;
    baseline_component_payloads_from_ops(
        &transaction.branch_id,
        transaction.trace.to_rev,
        generation,
        &ops,
    )
}

pub(crate) fn baseline_component_payloads_from_ops(
    branch: &str,
    revision: u64,
    generation: u64,
    ops: &[crate::transaction_trace::TraceOp],
) -> PactResult<Vec<(u32, Vec<u8>)>> {
    baseline_component_payloads_from_branches(
        &[BaselineBranchOps {
            branch_id: branch.to_string(),
            revision,
            ops: ops.to_vec(),
        }],
        generation,
    )
}

pub(crate) fn baseline_component_payloads_from_branches(
    branches: &[BaselineBranchOps],
    generation: u64,
) -> PactResult<Vec<(u32, Vec<u8>)>> {
    let mut entries = Vec::new();
    let mut ordinal_offset = 0_u32;
    for branch in branches {
        entries.extend(baseline_index_entries(
            &branch.branch_id,
            branch.revision,
            generation,
            &branch.ops,
            ordinal_offset,
            false,
        )?);
        ordinal_offset = ordinal_offset
            .checked_add(u32::try_from(branch.ops.len()).map_err(|_| {
                PactError::invalid("baseline component operation count exceeds u32")
            })?)
            .ok_or_else(|| PactError::invalid("baseline component ordinal overflow"))?;
    }
    entries
        .into_iter()
        .filter_map(|(key, value)| match IndexKey::decode(&key) {
            Ok(IndexKey::ComponentPayload {
                segment_id,
                ordinal,
            }) if segment_id == generation => Some(
                IndexedComponent::decode(&value)
                    .and_then(|component| component.encode_for_batch())
                    .map(|value| (ordinal, value)),
            ),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn component_secondary_entries(
    branch: &str,
    component: &IndexedComponent,
    facet_value: &[u8],
) -> PactResult<Vec<(IndexKey, Vec<u8>)>> {
    let component_id = component
        .value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| PactError::corrupt("indexed Pact component has no id"))?;
    let mut entries = Vec::new();
    if let Some(component_type) = component_type(&component.value) {
        entries.push((
            IndexKey::Type {
                branch: branch.to_string(),
                component_type: component_type.to_string(),
                component: component_id.to_string(),
            },
            facet_value.to_vec(),
        ));
    }
    for (key, value) in [
        ("key", component.value.get("key")),
        ("role", component.value.get("role")),
        ("tool_call_id", component.value.get("tool_call_id")),
        ("tool_name", component.value.get("tool_name")),
        ("success", component.value.get("success")),
    ] {
        if let Some(value) = value {
            entries.push((
                IndexKey::Aux {
                    branch: branch.to_string(),
                    key: key.to_string(),
                    value_hash: index_value_hash(value)?,
                    component: component_id.to_string(),
                },
                facet_value.to_vec(),
            ));
        }
    }
    if let Some(aux) = component
        .value
        .get("aux")
        .and_then(serde_json::Value::as_object)
    {
        for (key, value) in aux {
            entries.push((
                IndexKey::Aux {
                    branch: branch.to_string(),
                    key: key.clone(),
                    value_hash: index_value_hash(value)?,
                    component: component_id.to_string(),
                },
                facet_value.to_vec(),
            ));
        }
    }
    if let Some(tags) = component
        .value
        .get("tags")
        .and_then(serde_json::Value::as_array)
    {
        for tag in tags.iter().filter_map(serde_json::Value::as_str) {
            entries.push((
                IndexKey::Tag {
                    branch: branch.to_string(),
                    tag: tag.to_string(),
                    component: component_id.to_string(),
                },
                facet_value.to_vec(),
            ));
        }
    }
    Ok(entries)
}

pub(crate) fn index_value_hash(value: &serde_json::Value) -> PactResult<[u8; 16]> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| PactError::codec(format!("encode Pact index value: {error}")))?;
    let mut hash = [0_u8; 16];
    hash.copy_from_slice(&blake3::hash(&bytes).as_bytes()[..16]);
    Ok(hash)
}

fn delete_component_secondary_entries(
    mut tree: super::btree::CowBTree,
    pages: &mut impl super::btree::PageWriter,
    branch: &str,
    component: &IndexedComponent,
) -> PactResult<super::btree::CowBTree> {
    for (key, _) in component_secondary_entries(branch, component, &[])? {
        (tree, _, _) = tree.delete(pages, &key.encode()?)?;
    }
    Ok(tree)
}

fn insert_component_secondary_entries(
    mut tree: super::btree::CowBTree,
    pages: &mut impl super::btree::PageWriter,
    branch: &str,
    component: &IndexedComponent,
    locator: crate::pact_file::segment::RecordLocator,
) -> PactResult<super::btree::CowBTree> {
    for (key, value) in component_secondary_entries(branch, component, &locator.encode())? {
        (tree, _) = tree.insert(pages, key.encode()?, value)?;
    }
    Ok(tree)
}

pub(crate) fn baseline_component_ops(
    snapshot: &serde_json::Value,
) -> PactResult<Vec<crate::transaction_trace::TraceOp>> {
    let Some(current_branch_id) = snapshot
        .get("current_branch_id")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    let Some(branch) = snapshot
        .get("branches")
        .and_then(|branches| branches.get(current_branch_id))
    else {
        return Ok(Vec::new());
    };
    let Some(root_value) = branch.pointer("/current/tree/root") else {
        return Ok(Vec::new());
    };
    let root = root_value.clone();
    let component_type = root
        .get("componentType")
        .or_else(|| root.get("node_type"))
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("root".to_string()));
    let empty = serde_json::json!({
        "id": snapshot.get("root_id").cloned().unwrap_or_default(),
        "componentType": component_type,
        "children": [],
    });
    Ok(crate::transaction_trace::TransactionTrace::diff(
        0,
        branch
            .get("head_revision")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        &empty,
        &root,
    )
    .ops)
}

pub(crate) fn baseline_component_ops_from_ledger(
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<Vec<crate::transaction_trace::TraceOp>> {
    let branch = ledger
        .branches
        .get(&ledger.current_branch_id)
        .ok_or_else(|| PactError::corrupt("Pact baseline current branch is missing"))?;
    baseline_component_ops_for_branch(branch)
}

fn baseline_component_ops_for_branch(
    branch: &crate::store::PactBranch,
) -> PactResult<Vec<crate::transaction_trace::TraceOp>> {
    fn collect(
        component: &crate::Component,
        parent_id: &str,
        depth: usize,
        index: usize,
        out: &mut Vec<(usize, usize, String, crate::transaction_trace::TraceOp)>,
    ) {
        let mut value = component.clone_without_children().project_component_value();
        value["child_ids"] = serde_json::Value::Array(
            component
                .children
                .iter()
                .map(|child| serde_json::Value::String(child.id.clone()))
                .collect(),
        );
        let component_id = component.id.clone();
        out.push((
            depth,
            index,
            component_id.clone(),
            crate::transaction_trace::TraceOp::new(
                component_id,
                crate::transaction_trace::TraceOpKind::InsertComponent {
                    parent_id: parent_id.to_string(),
                    index,
                    component: value,
                },
            ),
        ));
        for (child_index, child) in component.children.iter().enumerate() {
            collect(child, &component.id, depth + 1, child_index, out);
        }
    }

    let mut ordered = Vec::new();
    for (index, child) in branch.current.tree.root.children.iter().enumerate() {
        collect(child, &branch.current.tree.root.id, 1, index, &mut ordered);
    }
    ordered.sort_by(|left, right| {
        (left.0, left.1, left.2.as_str()).cmp(&(right.0, right.1, right.2.as_str()))
    });
    Ok(ordered.into_iter().map(|(_, _, _, op)| op).collect())
}

pub(crate) fn baseline_component_ops_by_branch_from_ledger(
    ledger: &crate::store::ContextTreeLedger,
) -> PactResult<Vec<BaselineBranchOps>> {
    ledger
        .branches
        .iter()
        .map(|(branch_id, branch)| {
            Ok(BaselineBranchOps {
                branch_id: branch_id.clone(),
                revision: branch.head_revision,
                ops: baseline_component_ops_for_branch(branch)?,
            })
        })
        .collect()
}

pub(crate) fn indexed_component_from_facet(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    facet: &[u8],
) -> PactResult<(
    IndexedComponent,
    Option<crate::pact_file::segment::RecordLocator>,
)> {
    if facet.len() != crate::pact_file::segment::RecordLocator::ENCODED_BYTES {
        return Ok((IndexedComponent::decode(facet)?, None));
    }
    let locator = crate::pact_file::segment::RecordLocator::decode(facet)?;
    let payload = component_payload_from_tree(tree, pages, locator)?;
    Ok((IndexedComponent::decode(&payload)?, Some(locator)))
}

fn component_payload_from_tree(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    locator: crate::pact_file::segment::RecordLocator,
) -> PactResult<Vec<u8>> {
    if let Some(payload) = tree.get(
        pages,
        &IndexKey::ComponentPayload {
            segment_id: locator.segment_id,
            ordinal: locator.ordinal,
        }
        .encode()?,
    )? {
        return Ok(payload);
    }
    let address = tree
        .get(
            pages,
            &IndexKey::ComponentBatch {
                segment_id: locator.segment_id,
            }
            .encode()?,
        )?
        .ok_or_else(|| PactError::corrupt("Pact component locator has no payload batch"))?;
    let address = u64::from_le_bytes(
        address
            .as_slice()
            .try_into()
            .map_err(|_| PactError::corrupt("Pact component batch address has invalid width"))?,
    );
    pages.read_component_payload(address, locator.ordinal)
}

fn write_indexed_component(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    key: &IndexKey,
    indexed: &IndexedComponent,
) -> PactResult<super::btree::CowBTree> {
    if let Some(existing) = tree.get(pages, &key.encode()?)? {
        let (previous, locator) = indexed_component_from_facet(&tree, pages, &existing)?;
        tree = delete_component_secondary_entries(tree, pages, branch, &previous)?;
        if let Some(locator) = locator {
            (tree, _, _) = tree.delete(
                pages,
                &IndexKey::ComponentPayload {
                    segment_id: locator.segment_id,
                    ordinal: locator.ordinal,
                }
                .encode()?,
            )?;
        }
    }
    let locator = crate::pact_file::segment::RecordLocator {
        segment_id: pages.generation,
        ordinal: pages.next_component_ordinal,
    };
    pages.next_component_ordinal = pages
        .next_component_ordinal
        .checked_add(1)
        .ok_or_else(|| PactError::invalid("Pact component ordinal overflow"))?;
    (tree, _) = tree.insert(
        pages,
        IndexKey::ComponentPayload {
            segment_id: locator.segment_id,
            ordinal: locator.ordinal,
        }
        .encode()?,
        indexed.encode()?,
    )?;
    (tree, _) = tree.insert(pages, key.encode()?, locator.encode().to_vec())?;
    insert_component_secondary_entries(tree, pages, branch, indexed, locator)
}

fn apply_component_update(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    revision: u64,
    message_rank: &mut u64,
    op: &crate::transaction_trace::TraceOp,
    parent_child_facet: bool,
) -> PactResult<super::btree::CowBTree> {
    use crate::transaction_trace::TraceOpKind;

    let component_key = IndexKey::Component {
        branch: branch.to_string(),
        component: op.component_id.clone(),
    };
    match &op.kind {
        TraceOpKind::InsertComponent {
            parent_id,
            index,
            component,
        } => {
            let mut value = component.clone();
            let object = value.as_object_mut().ok_or_else(|| {
                PactError::invalid("inserted Pact component index value must be an object")
            })?;
            object
                .entry("id".to_string())
                .or_insert_with(|| serde_json::Value::String(op.component_id.clone()));
            let order = if component_type(&value) == Some("message_turn") {
                let order = message_order_key(revision, *message_rank)?;
                *message_rank += 1;
                Some(order)
            } else {
                None
            };
            let indexed = IndexedComponent {
                revision,
                order,
                child_count: 0,
                value,
            };
            tree = write_indexed_component(tree, pages, branch, &component_key, &indexed)?;
            if let Some(order) = order {
                (tree, _) = tree.insert(
                    pages,
                    IndexKey::MessageOrder {
                        branch: branch.to_string(),
                        order,
                    }
                    .encode()?,
                    op.component_id.as_bytes().to_vec(),
                )?;
            }
            if parent_child_facet {
                tree = insert_child_relation(
                    tree,
                    pages,
                    branch,
                    parent_id,
                    *index,
                    &op.component_id,
                )?;
            }
        }
        TraceOpKind::DeleteComponent => {
            let existing = tree.get(pages, &component_key.encode()?)?;
            if let Some(existing) = existing {
                let (indexed, _) = indexed_component_from_facet(&tree, pages, &existing)?;
                if parent_child_facet {
                    let parent = match indexed.value.get("parent_id").and_then(|v| v.as_str()) {
                        Some(parent) => Some(parent.to_string()),

                        None => relation_parent_of(&tree, pages, branch, &op.component_id)?,
                    };
                    if let Some(parent) = parent {
                        tree =
                            remove_child_relation(tree, pages, branch, &parent, &op.component_id)?;
                    }
                    let mut subtree = Vec::new();
                    collect_subtree_ids(&tree, pages, branch, &op.component_id, &mut subtree)?;
                    for component_id in subtree.into_iter().rev() {
                        tree = delete_indexed_component(tree, pages, branch, &component_id)?;
                    }
                } else {
                    tree = delete_indexed_component(tree, pages, branch, &op.component_id)?;
                }
            }
        }
        kind => {
            let Some(existing) = tree.get(pages, &component_key.encode()?)? else {
                return Ok(tree);
            };
            let (mut indexed, _) = indexed_component_from_facet(&tree, pages, &existing)?;
            if parent_child_facet && let TraceOpKind::MoveComponent { parent_id, index } = kind {
                let old_parent = indexed
                    .value
                    .get("parent_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        PactError::corrupt("moved indexed Pact component has no parent_id")
                    })?
                    .to_string();
                tree = remove_child_relation(tree, pages, branch, &old_parent, &op.component_id)?;
                tree = insert_child_relation(
                    tree,
                    pages,
                    branch,
                    parent_id,
                    *index,
                    &op.component_id,
                )?;
            }
            apply_component_change(&mut indexed.value, kind)?;
            indexed.revision = revision;
            tree = write_indexed_component(tree, pages, branch, &component_key, &indexed)?;
        }
    }
    Ok(tree)
}

fn relation_parent_of(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    branch: &str,
    child: &str,
) -> PactResult<Option<String>> {
    let prefix = IndexKey::parent_child_branch_prefix(branch)?;
    let end = prefix_successor(&prefix)?;
    let mut start = prefix;
    loop {
        let page = tree.range_limit(pages, &start, &end, 256, false)?;
        if page.is_empty() {
            return Ok(None);
        }
        for (key, _) in &page {
            if let IndexKey::ParentChild {
                parent,
                child: candidate,
                ..
            } = IndexKey::decode(key)?
                && candidate == child
            {
                return Ok(Some(parent));
            }
        }
        let last = page
            .last()
            .map(|(key, _)| key.clone())
            .ok_or_else(|| PactError::internal("non-empty page without a last key"))?;
        start = prefix_successor(&last)?;
        if page.len() < 256 {
            return Ok(None);
        }
    }
}

fn parent_children(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    branch: &str,
    parent: &str,
) -> PactResult<Vec<String>> {
    let prefix = IndexKey::parent_child_prefix(branch, parent)?;
    let end = prefix_successor(&prefix)?;
    tree.range(pages, &prefix, &end)?
        .into_iter()
        .map(|(key, _)| match IndexKey::decode(&key)? {
            IndexKey::ParentChild { child, .. } => Ok(child),
            _ => Err(PactError::corrupt(
                "Pact parent-child range returned another namespace",
            )),
        })
        .collect()
}

fn replace_parent_children(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    parent: &str,
    children: &[String],
) -> PactResult<super::btree::CowBTree> {
    let prefix = IndexKey::parent_child_prefix(branch, parent)?;
    let end = prefix_successor(&prefix)?;
    let existing = tree.range(pages, &prefix, &end)?;
    for (key, _) in existing {
        (tree, _, _) = tree.delete(pages, &key)?;
    }
    for (order, child) in children.iter().enumerate() {
        (tree, _) = tree.insert(
            pages,
            IndexKey::ParentChild {
                branch: branch.to_string(),
                parent: parent.to_string(),
                order: order as u64,
                child: child.clone(),
            }
            .encode()?,
            Vec::new(),
        )?;
    }
    set_indexed_child_count(tree, pages, branch, parent, children.len())
}

fn insert_child_relation(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    parent: &str,
    index: usize,
    child: &str,
) -> PactResult<super::btree::CowBTree> {
    let mut children = parent_children(&tree, pages, branch, parent)?;
    if index > children.len() {
        return Err(PactError::corrupt(
            "Pact child insertion index exceeds its parent child count",
        ));
    }
    if index == children.len() {
        (tree, _) = tree.insert(
            pages,
            IndexKey::ParentChild {
                branch: branch.to_string(),
                parent: parent.to_string(),
                order: index as u64,
                child: child.to_string(),
            }
            .encode()?,
            Vec::new(),
        )?;
        return set_indexed_child_count(tree, pages, branch, parent, index + 1);
    }
    children.insert(index, child.to_string());
    replace_parent_children(tree, pages, branch, parent, &children)
}

fn remove_child_relation(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    parent: &str,
    child: &str,
) -> PactResult<super::btree::CowBTree> {
    let mut children = parent_children(&tree, pages, branch, parent)?;
    let Some(position) = children.iter().position(|candidate| candidate == child) else {
        return Err(PactError::corrupt(
            "Pact parent-child facet is missing a component relation",
        ));
    };
    if position + 1 == children.len() {
        (tree, _, _) = tree.delete(
            pages,
            &IndexKey::ParentChild {
                branch: branch.to_string(),
                parent: parent.to_string(),
                order: position as u64,
                child: child.to_string(),
            }
            .encode()?,
        )?;
        return set_indexed_child_count(tree, pages, branch, parent, position);
    }
    children.remove(position);
    replace_parent_children(tree, pages, branch, parent, &children)
}

fn set_indexed_child_count(
    tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    component_id: &str,
    child_count: usize,
) -> PactResult<super::btree::CowBTree> {
    let key = IndexKey::Component {
        branch: branch.to_string(),
        component: component_id.to_string(),
    };
    let Some(existing) = tree.get(pages, &key.encode()?)? else {
        return Ok(tree);
    };
    let (mut indexed, _) = indexed_component_from_facet(&tree, pages, &existing)?;
    if indexed.child_count == child_count {
        return Ok(tree);
    }
    indexed.child_count = child_count;
    write_indexed_component(tree, pages, branch, &key, &indexed)
}

fn collect_subtree_ids(
    tree: &super::btree::CowBTree,
    pages: &(impl super::btree::PageReader + ?Sized),
    branch: &str,
    component: &str,
    out: &mut Vec<String>,
) -> PactResult<()> {
    if out.iter().any(|visited| visited == component) {
        return Err(PactError::corrupt(
            "Pact parent-child facet contains a cycle",
        ));
    }
    out.push(component.to_string());
    for child in parent_children(tree, pages, branch, component)? {
        collect_subtree_ids(tree, pages, branch, &child, out)?;
    }
    Ok(())
}

fn delete_indexed_component(
    mut tree: super::btree::CowBTree,
    pages: &mut StagedPages<'_>,
    branch: &str,
    component_id: &str,
) -> PactResult<super::btree::CowBTree> {
    let key = IndexKey::Component {
        branch: branch.to_string(),
        component: component_id.to_string(),
    };
    let Some(existing) = tree.get(pages, &key.encode()?)? else {
        return Ok(tree);
    };
    let (indexed, locator) = indexed_component_from_facet(&tree, pages, &existing)?;
    tree = delete_component_secondary_entries(tree, pages, branch, &indexed)?;
    (tree, _, _) = tree.delete(pages, &key.encode()?)?;
    if let Some(locator) = locator {
        (tree, _, _) = tree.delete(
            pages,
            &IndexKey::ComponentPayload {
                segment_id: locator.segment_id,
                ordinal: locator.ordinal,
            }
            .encode()?,
        )?;
    }
    if let Some(order) = indexed.order {
        (tree, _, _) = tree.delete(
            pages,
            &IndexKey::MessageOrder {
                branch: branch.to_string(),
                order,
            }
            .encode()?,
        )?;
    }
    tree = replace_parent_children(tree, pages, branch, component_id, &[])?;
    Ok(tree)
}

fn prefix_successor(prefix: &[u8]) -> PactResult<Vec<u8>> {
    let mut end = prefix.to_vec();
    for byte in end.iter_mut().rev() {
        if *byte != u8::MAX {
            *byte += 1;
            return Ok(end);
        }
        *byte = 0;
    }
    Err(PactError::invalid(
        "Pact index prefix has no finite successor",
    ))
}

fn apply_component_change(
    value: &mut serde_json::Value,
    kind: &crate::transaction_trace::TraceOpKind,
) -> PactResult<()> {
    use crate::transaction_trace::TraceOpKind;
    let object = value
        .as_object_mut()
        .ok_or_else(|| PactError::corrupt("indexed Pact component is not an object"))?;
    match kind {
        TraceOpKind::MoveComponent { parent_id, .. } => {
            object.insert(
                "parent_id".to_string(),
                serde_json::Value::String(parent_id.clone()),
            );
        }
        TraceOpKind::SetField { key, value } => {
            object.insert(key.clone(), value.clone());
        }
        TraceOpKind::UnsetField { key } => {
            object.remove(key);
        }
        TraceOpKind::SetAttr { key, value } => {
            let aux = object
                .entry("aux".to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            aux.as_object_mut()
                .ok_or_else(|| PactError::corrupt("indexed Pact component aux is not an object"))?
                .insert(key.clone(), value.clone());
        }
        TraceOpKind::UnsetAttr { key } => {
            if let Some(aux) = object
                .get_mut("aux")
                .and_then(serde_json::Value::as_object_mut)
            {
                aux.remove(key);
            }
        }
        TraceOpKind::SetContent {
            content,
            content_ref,
            content_preview,
        } => {
            set_optional(
                object,
                "content",
                content.as_ref().map(|value| serde_json::json!(value)),
            );
            set_optional(object, "content_ref", content_ref.clone());
            set_optional(
                object,
                "content_preview",
                content_preview
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
            );
        }
        TraceOpKind::ReplaceContentRange {
            start,
            end,
            replacement,
        } => {
            let content = object
                .get_mut("content")
                .and_then(|value| value.as_str())
                .ok_or_else(|| PactError::corrupt("indexed Pact component has no text content"))?;
            let mut updated = content.to_string();
            if *start > *end
                || *end > updated.len()
                || !updated.is_char_boundary(*start)
                || !updated.is_char_boundary(*end)
            {
                return Err(PactError::corrupt("indexed Pact content range is invalid"));
            }
            updated.replace_range(*start..*end, replacement);
            object.insert("content".to_string(), serde_json::Value::String(updated));
        }
        TraceOpKind::SetLifecycle { value } => {
            object.insert("lifecycle".to_string(), value.clone());
        }
        TraceOpKind::InsertComponent { .. } | TraceOpKind::DeleteComponent => {}
    }
    Ok(())
}

fn set_optional(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<serde_json::Value>,
) {
    if let Some(value) = value {
        object.insert(key.to_string(), value);
    } else {
        object.remove(key);
    }
}

pub(crate) fn component_type(value: &serde_json::Value) -> Option<&str> {
    value
        .get("node_type")
        .or_else(|| value.get("componentType"))
        .and_then(serde_json::Value::as_str)
}

pub(crate) fn read_index_value(
    file: &[u8],
    root_address: u64,
    key: &IndexKey,
) -> PactResult<Option<Vec<u8>>> {
    let pages = ExistingPages { file };
    super::btree::CowBTree::from_root(Some(root_address)).get(&pages, &key.encode()?)
}

pub(crate) fn parent_children_from_file(
    file: &[u8],
    root_address: u64,
    branch: &str,
    parent: &str,
) -> PactResult<Vec<String>> {
    let pages = ExistingPages { file };
    parent_children_from_pages(&pages, root_address, branch, parent)
}

pub(crate) fn parent_children_from_pages(
    pages: &dyn super::btree::PageReader,
    root_address: u64,
    branch: &str,
    parent: &str,
) -> PactResult<Vec<String>> {
    let tree = super::btree::CowBTree::from_root(Some(root_address));
    let prefix = IndexKey::parent_child_prefix(branch, parent)?;
    let end = prefix_successor(&prefix)?;
    tree.range(pages, &prefix, &end)?
        .into_iter()
        .map(|(key, _)| match IndexKey::decode(&key)? {
            IndexKey::ParentChild { child, .. } => Ok(child),
            _ => Err(PactError::corrupt(
                "Pact parent-child range returned another namespace",
            )),
        })
        .collect()
}

pub(crate) fn parent_children_by_parent_from_pages(
    pages: &dyn super::btree::PageReader,
    root_address: u64,
    branch: &str,
) -> PactResult<BTreeMap<String, Vec<String>>> {
    let tree = super::btree::CowBTree::from_root(Some(root_address));
    let prefix = IndexKey::parent_child_branch_prefix(branch)?;
    let end = prefix_successor(&prefix)?;
    let mut children = BTreeMap::<String, Vec<(u64, String)>>::new();
    for (key, _) in tree.range(pages, &prefix, &end)? {
        let IndexKey::ParentChild {
            parent,
            order,
            child,
            ..
        } = IndexKey::decode(&key)?
        else {
            return Err(PactError::corrupt(
                "Pact parent-child branch range returned another namespace",
            ));
        };
        children.entry(parent).or_default().push((order, child));
    }
    children
        .into_iter()
        .map(|(parent, mut ordered)| {
            ordered.sort_by_key(|(order, _)| *order);
            if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(PactError::corrupt(
                    "Pact parent-child facet repeats a child order",
                ));
            }
            Ok((
                parent,
                ordered.into_iter().map(|(_, child)| child).collect(),
            ))
        })
        .collect()
}

pub(crate) fn read_index_value_from_pages(
    pages: &dyn super::btree::PageReader,
    root_address: u64,
    key: &IndexKey,
) -> PactResult<Option<Vec<u8>>> {
    super::btree::CowBTree::from_root(Some(root_address)).get(pages, &key.encode()?)
}

pub(crate) fn read_component_payload(
    file: &[u8],
    root_address: u64,
    locator: crate::pact_file::segment::RecordLocator,
) -> PactResult<Vec<u8>> {
    let pages = ExistingPages { file };
    read_component_payload_from_pages(&pages, root_address, locator)
}

pub(crate) fn read_component_payload_from_pages(
    pages: &dyn super::btree::PageReader,
    root_address: u64,
    locator: crate::pact_file::segment::RecordLocator,
) -> PactResult<Vec<u8>> {
    component_payload_from_tree(
        &super::btree::CowBTree::from_root(Some(root_address)),
        pages,
        locator,
    )
}

pub(crate) fn generation_addresses(
    file: &[u8],
    root_address: u64,
    generation: u64,
) -> PactResult<Vec<u64>> {
    let pages = ExistingPages { file };
    let entries = super::btree::CowBTree::from_root(Some(root_address)).range_limit(
        &pages,
        &IndexKey::Generation { generation: 0 }.encode()?,
        &IndexKey::Generation {
            generation: u64::MAX,
        }
        .encode()?,
        usize::try_from(generation)
            .map_err(|_| PactError::corrupt("Pact generation exceeds platform limits"))?,
        false,
    )?;
    entries
        .into_iter()
        .map(|(_, address)| {
            let raw: [u8; 8] = address
                .as_slice()
                .try_into()
                .map_err(|_| PactError::corrupt("Pact generation address has invalid width"))?;
            Ok(u64::from_le_bytes(raw))
        })
        .collect()
}

struct ExistingPages<'a> {
    file: &'a [u8],
}

impl super::btree::PageReader for ExistingPages<'_> {
    fn read_page(&self, address: u64) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
        super::codec::decode_index_page_record(self.file, address)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        super::codec::decode_component_payload_batch_record(self.file, address, ordinal)
    }
}

enum PageStore {
    Memory(BTreeMap<u64, [u8; super::btree::PAGE_BYTES]>),
    Scratch {
        file: super::scratch::ScratchFile,
        spans: BTreeMap<u64, super::scratch::SpoolSpan>,
    },
}

impl PageStore {
    fn len(&self) -> usize {
        match self {
            Self::Memory(pages) => pages.len(),
            Self::Scratch { spans, .. } => spans.len(),
        }
    }

    fn contains_key(&self, address: u64) -> bool {
        match self {
            Self::Memory(pages) => pages.contains_key(&address),
            Self::Scratch { spans, .. } => spans.contains_key(&address),
        }
    }

    fn get(&self, address: u64) -> PactResult<Option<[u8; super::btree::PAGE_BYTES]>> {
        match self {
            Self::Memory(pages) => Ok(pages.get(&address).copied()),
            Self::Scratch { file, spans } => {
                let Some(span) = spans.get(&address) else {
                    return Ok(None);
                };
                let bytes = file.read_span(*span)?;
                let mut page = [0_u8; super::btree::PAGE_BYTES];
                if bytes.len() != page.len() {
                    return Err(PactError::corrupt(
                        "spilled Pact index page has invalid width",
                    ));
                }
                page.copy_from_slice(&bytes);
                Ok(Some(page))
            }
        }
    }

    fn insert(&mut self, address: u64, page: [u8; super::btree::PAGE_BYTES]) -> PactResult<()> {
        match self {
            Self::Memory(pages) => {
                pages.insert(address, page);
            }
            Self::Scratch { file, spans } => {
                let span = file.append(&page)?;
                spans.insert(address, span);
            }
        }
        Ok(())
    }

    fn remove(&mut self, address: u64) {
        match self {
            Self::Memory(pages) => {
                pages.remove(&address);
            }
            Self::Scratch { spans, .. } => {
                spans.remove(&address);
            }
        }
    }

    fn spill(&mut self, io_chunk_bytes: usize) -> PactResult<()> {
        if let Self::Memory(pages) = self {
            let mut file = super::scratch::ScratchFile::create(None, io_chunk_bytes)?;
            let mut spans = BTreeMap::new();
            for (address, page) in pages.iter() {
                spans.insert(*address, file.append(page)?);
            }
            *self = Self::Scratch { file, spans };
        }
        Ok(())
    }
}

struct StagedPages<'a> {
    existing_pages: Option<&'a dyn super::btree::PageReader>,

    staged: PageStore,
    batch_address: u64,
    next_address: u64,
    generation: u64,
    next_component_ordinal: u32,

    budget_pages: Option<usize>,
    io_chunk_bytes: usize,
}

impl<'a> StagedPages<'a> {
    fn new(
        existing_pages: Option<&'a dyn super::btree::PageReader>,
        start_address: u64,
        generation: u64,
    ) -> Self {
        Self {
            existing_pages,
            staged: PageStore::Memory(BTreeMap::new()),
            batch_address: start_address,
            next_address: 1,
            generation,
            next_component_ordinal: 0,
            budget_pages: None,
            io_chunk_bytes: 0,
        }
    }

    fn new_bounded(
        existing_pages: Option<&'a dyn super::btree::PageReader>,
        start_address: u64,
        generation: u64,
        in_flight_bytes: usize,
        io_chunk_bytes: usize,
    ) -> Self {
        let mut pages = Self::new(existing_pages, start_address, generation);
        pages.budget_pages = Some((in_flight_bytes / super::btree::PAGE_BYTES).max(1));
        pages.io_chunk_bytes = io_chunk_bytes.max(4096);
        pages
    }

    fn staged_page_count(&self) -> usize {
        self.staged.len()
    }

    fn spilled(&self) -> bool {
        matches!(self.staged, PageStore::Scratch { .. })
    }

    fn finish(self, root: u64) -> PactResult<(Vec<u8>, u64)> {
        self.finish_with_batch_limit(root, super::codec::INDEX_BATCH_TARGET_PAGES)
    }

    fn finish_with_batch_limit(self, root: u64, max_pages: usize) -> PactResult<(Vec<u8>, u64)> {
        let mut bytes = Vec::new();
        let root = self.finish_into(root, max_pages, &mut bytes)?;
        Ok((bytes, root))
    }

    fn finish_into(
        self,
        root: u64,
        max_pages: usize,
        out: &mut dyn std::io::Write,
    ) -> PactResult<u64> {
        if max_pages == 0 || max_pages > super::codec::MAX_INDEX_BATCH_PAGES {
            return Err(PactError::internal(
                "Pact index batch page limit is out of range",
            ));
        }
        let mut packer = BatchPacker {
            staged: self.staged,
            generation: self.generation,
            max_pages,
            batch_address: self.batch_address,
            relocated: BTreeMap::new(),
            pending: Vec::new(),
            out,
            written: 0,
        };
        let root = packer.pack_reachable(root)?;
        packer.flush()?;
        if packer.written == 0 {
            return Err(PactError::invalid(
                "Pact transaction index produced no staged pages",
            ));
        }
        Ok(root)
    }
}

struct BatchPacker<'a> {
    staged: PageStore,
    generation: u64,
    max_pages: usize,
    batch_address: u64,
    relocated: BTreeMap<u64, u64>,
    pending: Vec<[u8; super::btree::PAGE_BYTES]>,
    out: &'a mut dyn std::io::Write,
    written: u64,
}

impl BatchPacker<'_> {
    fn pack_reachable(&mut self, root: u64) -> PactResult<u64> {
        let mut stack = vec![(root, false)];
        while let Some((address, references_done)) = stack.pop() {
            if self.relocated.contains_key(&address) {
                continue;
            }
            let Some(page) = self.staged.get(address)? else {
                continue;
            };
            if !references_done {
                stack.push((address, true));
                for reference in super::btree::page_references(&page)? {
                    if self.staged.contains_key(reference)
                        && !self.relocated.contains_key(&reference)
                    {
                        stack.push((reference, false));
                    }
                }
                continue;
            }
            let relocated = &self.relocated;
            let packed_page = super::btree::map_page_references(&page, |reference| {
                Ok(relocated.get(&reference).copied().unwrap_or(reference))
            })?;
            if self.pending.len() >= self.max_pages {
                self.flush()?;
            }
            let packed =
                super::codec::pack_index_page_address(self.batch_address, self.pending.len())?;
            self.pending.push(packed_page);
            self.relocated.insert(address, packed);

            self.staged.remove(address);
        }
        Ok(self.relocated.get(&root).copied().unwrap_or(root))
    }

    fn flush(&mut self) -> PactResult<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let record = super::codec::encode_index_page_batch_record(self.generation, &self.pending)?;
        self.batch_address = self
            .batch_address
            .checked_add(record.len() as u64)
            .ok_or_else(|| PactError::invalid("Pact index batch address overflow"))?;
        self.out.write_all(&record)?;
        self.written += record.len() as u64;
        self.pending.clear();
        Ok(())
    }
}

impl super::btree::PageReader for StagedPages<'_> {
    fn read_page(&self, address: u64) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
        if let Some(page) = self.staged.get(address)? {
            return Ok(page);
        }
        let pages = self
            .existing_pages
            .ok_or_else(|| PactError::corrupt("Pact index root has no backing file"))?;
        pages.read_page(address)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        self.existing_pages
            .ok_or_else(|| PactError::corrupt("Pact component batch has no backing file"))?
            .read_component_payload(address, ordinal)
    }
}

impl super::btree::PageWriter for StagedPages<'_> {
    fn write_page(&mut self, page: [u8; super::btree::PAGE_BYTES]) -> PactResult<u64> {
        let address = self.next_address;
        self.next_address = self
            .next_address
            .checked_add(1)
            .ok_or_else(|| PactError::invalid("Pact index page address overflow"))?;
        self.staged.insert(address, page)?;
        if let Some(budget) = self.budget_pages
            && !self.spilled()
            && self.staged.len() > budget
        {
            self.staged.spill(self.io_chunk_bytes)?;
        }
        Ok(address)
    }
}
