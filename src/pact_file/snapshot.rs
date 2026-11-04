use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use memmap2::{Mmap, MmapOptions};

use super::backend::{IoCounterSet, IoCounters};
use super::codec::{
    ContentBodySource, RECORD_HEADER_BYTES, decode_commit_marker_region,
    decode_content_body_record, decode_index_page_batch_record, decode_transaction_region,
};
use super::index::{IndexKey, read_index_value_from_pages};
use super::layout::HEADER_BYTES;
use super::root_slot::root_slot_candidates;
use crate::error::{PactError, PactResult};

#[cfg(test)]
mod tests;

const COMMIT_MARKER_BYTES: u64 = super::codec::COMMIT_RECORD_BYTES;

pub(crate) struct Snapshot {
    path: PathBuf,
    shared: Arc<SharedSnapshotData>,
    caches: Arc<SnapshotCaches>,
    generation: u64,
    committed_len: u64,
    transaction_root: u64,
    index_root: u64,
    source_file_identity: [u8; super::layout::FILE_IDENTITY_BYTES],
    counters: Arc<IoCounterSet>,
}

impl Snapshot {
    pub(crate) fn open(path: &Path, counters: Arc<IoCounterSet>) -> PactResult<Self> {
        Self::open_with_memory_options(path, counters, &crate::PactMemoryOptions::default())
    }

    pub(crate) fn open_with_memory_options(
        path: &Path,
        counters: Arc<IoCounterSet>,
        options: &crate::PactMemoryOptions,
    ) -> PactResult<Self> {
        options.validate()?;
        Self::open_with_caches(path, counters, options, None)
    }

    fn open_with_caches(
        path: &Path,
        counters: Arc<IoCounterSet>,
        options: &crate::PactMemoryOptions,
        reuse: Option<&Arc<SnapshotCaches>>,
    ) -> PactResult<Self> {
        let canonical = path.canonicalize()?;
        let mut file = File::open(&canonical)?;
        let metadata = file.metadata()?;
        let header =
            read_range(&mut file, 0, HEADER_BYTES as u64).map_err(|error| match error {
                PactError::Io(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    PactError::corrupt("truncated Pact file envelope")
                }
                other => other,
            })?;
        super::envelope::classify(&header)?;
        let source_file_identity = super::envelope::file_identity(&header)?;
        let mut bytes_read = header.len() as u64;
        let mut selected = None;
        for (_, slot) in root_slot_candidates(&header)? {
            if slot.file_length_watermark > metadata.len()
                || slot.file_length_watermark
                    != slot
                        .committed_transaction_address
                        .saturating_add(COMMIT_MARKER_BYTES)
            {
                continue;
            }
            let marker_bytes = read_range(
                &mut file,
                slot.committed_transaction_address,
                COMMIT_MARKER_BYTES,
            )?;
            bytes_read = bytes_read.saturating_add(marker_bytes.len() as u64);
            let Ok(marker) = decode_commit_marker_region(
                &marker_bytes,
                slot.committed_transaction_address,
                slot.generation,
            ) else {
                continue;
            };
            if marker.index_root_address != slot.index_root_address
                || marker.file_length_watermark != slot.file_length_watermark
                || marker.segment_directory_address != slot.segment_directory_address
                || marker.segment_directory_len != slot.segment_directory_len
                || marker.allocator_address != slot.allocator_address
                || marker.allocator_len != slot.allocator_len
                || marker.segment_directory_digest != slot.segment_directory_digest
                || marker.allocator_digest != slot.allocator_digest
            {
                continue;
            }
            if selected
                .as_ref()
                .is_none_or(|(generation, _, _, _)| slot.generation > *generation)
            {
                selected = Some((
                    slot.generation,
                    slot.file_length_watermark,
                    marker.transaction_address,
                    slot.index_root_address,
                ));
            }
        }
        let (generation, committed_len, transaction_root, index_root) = selected
            .ok_or_else(|| PactError::corrupt("Pact snapshot has no valid committed root"))?;
        let platform_id = crate::file_identity::file_id(&file)?;
        let key = MappingKey::new(
            &canonical,
            &metadata,
            generation,
            committed_len,
            platform_id,
        );
        let shared = shared_mapping(key, &file, committed_len)?;

        let caches = match reuse {
            Some(caches) if caches.identity == (source_file_identity, platform_id) => {
                Arc::clone(caches)
            }
            _ => Arc::new(SnapshotCaches::new(
                options,
                source_file_identity,
                platform_id,
            )),
        };
        counters.record(&IoCounters {
            bytes_read,
            mappings_touched: 1,
            ..IoCounters::default()
        });
        Ok(Self {
            path: canonical,
            shared,
            caches,
            generation,
            committed_len,
            transaction_root,
            index_root,
            source_file_identity,
            counters,
        })
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn source_file_identity(&self) -> String {
        self.source_file_identity
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn refresh(&self) -> PactResult<Self> {
        Self::open_with_caches(
            &self.path,
            Arc::clone(&self.counters),
            &self.caches.options,
            Some(&self.caches),
        )
    }

    pub(crate) fn branch_head(&self, branch: &str) -> PactResult<Option<u64>> {
        if let Some(metadata) = self.metadata_optional()? {
            return Ok(metadata.head_revisions.get(branch).copied());
        }
        let value = read_index_value_from_pages(
            self,
            self.index_root,
            &IndexKey::BranchHead {
                branch: branch.to_string(),
            },
        )?;
        value
            .map(|bytes| {
                let raw: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| PactError::corrupt("Pact branch head has invalid width"))?;
                Ok(u64::from_le_bytes(raw))
            })
            .transpose()
    }

    pub(crate) fn index_range(
        &self,
        start: &IndexKey,
        end: &IndexKey,
        limit: usize,
        reverse: bool,
    ) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        super::btree::CowBTree::from_root(Some(self.index_root)).range_limit(
            self,
            &start.encode()?,
            &end.encode()?,
            limit,
            reverse,
        )
    }

    pub(crate) fn index_value(&self, key: &IndexKey) -> PactResult<Option<Vec<u8>>> {
        read_index_value_from_pages(self, self.index_root, key)
    }

    pub(crate) fn index_range_window(
        &self,
        start: &IndexKey,
        end: &IndexKey,
        offset: usize,
        limit: usize,
    ) -> PactResult<super::btree::RangeWindow> {
        super::btree::CowBTree::from_root(Some(self.index_root)).range_window(
            self,
            &start.encode()?,
            &end.encode()?,
            offset,
            limit,
        )
    }

    pub(crate) fn has_text_change_facet(&self) -> PactResult<bool> {
        Ok(
            read_index_value_from_pages(self, self.index_root, &IndexKey::TextChangeFacet)?
                .is_some(),
        )
    }

    pub(crate) fn has_search_change_facet(&self) -> PactResult<bool> {
        Ok(
            read_index_value_from_pages(self, self.index_root, &IndexKey::SearchChangeFacet)?
                .is_some(),
        )
    }

    pub(crate) fn has_search_selector_facet(&self) -> PactResult<bool> {
        Ok(
            read_index_value_from_pages(self, self.index_root, &IndexKey::SearchSelectorFacet)?
                .is_some(),
        )
    }

    pub(crate) fn search_changes(
        &self,
        branch: &str,
        cursor: Option<(u64, u64)>,
        limit: usize,
    ) -> PactResult<Vec<super::search_change_feed::IndexedSearchChange>> {
        let (start_revision, start_ordinal) = match cursor {
            Some((u64::MAX, u64::MAX)) => return Ok(Vec::new()),
            Some((revision, u64::MAX)) => (revision.saturating_add(1), 0),
            Some((revision, ordinal)) => (revision, ordinal + 1),
            None => (0, 0),
        };
        let start = IndexKey::SearchChange {
            branch: branch.to_string(),
            revision: start_revision,
            ordinal: start_ordinal,
            component: String::new(),
        }
        .encode()?;
        let prefix = IndexKey::search_change_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let entries = self.index_range_raw_limit(&start, &end, limit, false)?;
        let mut changes = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let IndexKey::SearchChange {
                branch: indexed_branch,
                revision,
                ordinal,
                component,
            } = IndexKey::decode(&key)?
            else {
                return Err(PactError::corrupt(
                    "Pact search-change range returned another namespace",
                ));
            };
            if indexed_branch != branch {
                return Err(PactError::corrupt(
                    "Pact search-change range crossed its branch boundary",
                ));
            }
            changes.push(super::search_change_feed::decode_index_value(
                revision, ordinal, component, &value,
            )?);
        }
        self.record_rows_read(changes.len());
        Ok(changes)
    }

    pub(crate) fn latest_search_change_position(
        &self,
        branch: &str,
    ) -> PactResult<Option<(u64, u64)>> {
        let prefix = IndexKey::search_change_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let Some((key, _)) = self
            .index_range_raw_limit(&prefix, &end, 1, true)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let IndexKey::SearchChange {
            branch: indexed_branch,
            revision,
            ordinal,
            ..
        } = IndexKey::decode(&key)?
        else {
            return Err(PactError::corrupt(
                "Pact search-change range returned another namespace",
            ));
        };
        if indexed_branch != branch {
            return Err(PactError::corrupt(
                "Pact search-change range crossed its branch boundary",
            ));
        }
        Ok(Some((revision, ordinal)))
    }

    pub(crate) fn text_changes(
        &self,
        branch: &str,
        cursor: Option<(u64, u64)>,
        limit: usize,
    ) -> PactResult<Vec<super::change_feed::IndexedTextChange>> {
        let (start_revision, start_ordinal) = match cursor {
            Some((u64::MAX, u64::MAX)) => return Ok(Vec::new()),
            Some((revision, u64::MAX)) => (revision + 1, 0),
            Some((revision, ordinal)) => (revision, ordinal + 1),
            None => (0, 0),
        };
        let start = IndexKey::TextAddress {
            branch: branch.to_string(),
            revision: start_revision,
            ordinal: start_ordinal,
            component: String::new(),
            field: String::new(),
        }
        .encode()?;
        let prefix = IndexKey::text_address_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let entries = self.index_range_raw_limit(&start, &end, limit, false)?;
        let mut changes = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let IndexKey::TextAddress {
                branch: indexed_branch,
                revision,
                ordinal,
                component,
                field,
            } = IndexKey::decode(&key)?
            else {
                return Err(PactError::corrupt(
                    "Pact text-change range returned another namespace",
                ));
            };
            if indexed_branch != branch {
                return Err(PactError::corrupt(
                    "Pact text-change range crossed its branch boundary",
                ));
            }
            let mut change = super::change_feed::decode_index_value(
                revision,
                ordinal,
                &indexed_branch,
                &component,
                &field,
                &value,
            )?;
            if !change.change.tombstone
                && change.change.text_hash.is_empty()
                && let Some(content_id) = change.change.content_ref.as_deref()
                && let Some(hash) = self.content_hash(content_id)?
            {
                change.change.text_hash = hash;
            }
            changes.push(change);
        }
        self.resolve_inline_text_change_hashes(branch, &mut changes)?;
        self.record_rows_read(changes.len());
        Ok(changes)
    }

    fn resolve_inline_text_change_hashes(
        &self,
        branch: &str,
        changes: &mut [super::change_feed::IndexedTextChange],
    ) -> PactResult<()> {
        let requested = changes
            .iter()
            .filter(|change| {
                !change.change.tombstone
                    && change.change.content_ref.is_none()
                    && change.change.text_hash.is_empty()
            })
            .map(|change| {
                (
                    (change.revision, change.ordinal),
                    (change.change.node_id.as_str(), change.change.field.as_str()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if requested.is_empty() {
            return Ok(());
        }
        let revisions = requested
            .keys()
            .map(|(revision, _)| *revision)
            .collect::<std::collections::BTreeSet<_>>();
        let mut hashes = BTreeMap::<(u64, u64), String>::new();
        for mut trace in self
            .public_traces_unresolved()?
            .into_values()
            .filter(|trace| {
                trace.branch_id == branch
                    && trace
                        .replay_trace
                        .as_ref()
                        .is_some_and(|replay| revisions.contains(&replay.to_rev))
            })
        {
            let marker_ids = trace
                .replay_trace
                .as_ref()
                .into_iter()
                .flat_map(|replay| &replay.ops)
                .filter_map(|op| match &op.kind {
                    crate::transaction_trace::TraceOpKind::InsertComponent {
                        component, ..
                    } => component
                        .get("$pact_head_component")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            if !marker_ids.is_empty() {
                let mut components = BTreeMap::new();
                for component_id in marker_ids {
                    let component =
                        self.component_shallow(branch, &component_id)?
                            .ok_or_else(|| {
                                PactError::corrupt(
                                    "Pact text trace references a missing canonical component",
                                )
                            })?;
                    components.insert(component_id, component.value);
                }
                let transaction_id = trace.transaction_id.clone();
                let mut singleton = BTreeMap::from([(transaction_id.clone(), trace)]);
                super::head_checkpoint::resolve_head_component_refs(&mut singleton, &components)?;
                trace = singleton
                    .remove(&transaction_id)
                    .ok_or_else(|| PactError::internal("resolved Pact text trace disappeared"))?;
            }
            let replay = trace
                .replay_trace
                .as_ref()
                .ok_or_else(|| PactError::corrupt("Pact text trace lost its replay operations"))?;
            for derived in super::change_feed::indexed_text_changes_from_trace(
                branch,
                replay,
                &mut |content_id| {
                    self.content(content_id)?
                        .map(|record| super::change_feed::content_record_bytes(&record))
                        .transpose()
                },
            )? {
                let position = (derived.revision, derived.ordinal);
                if let Some((node, field)) = requested.get(&position) {
                    if derived.change.node_id != *node || derived.change.field != *field {
                        return Err(PactError::corrupt(
                            "Pact text-change facet disagrees with its canonical trace",
                        ));
                    }
                    hashes.insert(position, derived.change.text_hash);
                }
            }
        }
        for change in changes.iter_mut().filter(|change| {
            !change.change.tombstone
                && change.change.content_ref.is_none()
                && change.change.text_hash.is_empty()
        }) {
            change.change.text_hash = hashes
                .remove(&(change.revision, change.ordinal))
                .ok_or_else(|| {
                    PactError::corrupt("Pact text-change hash is missing from its canonical trace")
                })?;
        }
        Ok(())
    }

    pub(crate) fn component(
        &self,
        branch: &str,
        component: &str,
    ) -> PactResult<Option<super::index::IndexedComponent>> {
        self.component_shallow(branch, component)?
            .map(|indexed| self.attach_child_ids(branch, indexed))
            .transpose()
    }

    pub(crate) fn component_shallow(
        &self,
        branch: &str,
        component: &str,
    ) -> PactResult<Option<super::index::IndexedComponent>> {
        let Some(facet) = self.component_facet(branch, component)? else {
            return Ok(None);
        };
        Ok(Some(self.component_from_facet(&facet)?))
    }

    fn component_from_facet(&self, facet: &[u8]) -> PactResult<super::index::IndexedComponent> {
        let payload = if facet.len() == super::segment::RecordLocator::ENCODED_BYTES {
            let locator = super::segment::RecordLocator::decode(facet)?;
            super::index::read_component_payload_from_pages(self, self.index_root, locator)?
        } else {
            facet.to_vec()
        };
        super::index::IndexedComponent::decode(&payload)
    }

    fn component_facet(&self, branch: &str, component: &str) -> PactResult<Option<Vec<u8>>> {
        read_index_value_from_pages(
            self,
            self.index_root,
            &IndexKey::Component {
                branch: branch.to_string(),
                component: component.to_string(),
            },
        )
    }

    fn attach_child_ids(
        &self,
        branch: &str,
        mut indexed: super::index::IndexedComponent,
    ) -> PactResult<super::index::IndexedComponent> {
        if indexed.child_count == 0 {
            return Ok(indexed);
        }
        let component_id = indexed
            .value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| PactError::corrupt("indexed Pact component has no id"))?;
        let child_ids = self.parent_children(branch, component_id)?;
        if child_ids.len() != indexed.child_count {
            return Err(PactError::corrupt(
                "Pact component child count disagrees with parent-child facet",
            ));
        }
        indexed
            .value
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt("indexed Pact component is not an object"))?
            .insert(
                "child_ids".to_string(),
                serde_json::Value::Array(
                    child_ids
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        Ok(indexed)
    }

    #[cfg(test)]
    pub(crate) fn raw_component_facet(&self, branch: &str, component: &str) -> PactResult<Vec<u8>> {
        self.component_facet(branch, component)?
            .ok_or_else(|| PactError::not_found("Pact component facet not found"))
    }

    pub(crate) fn has_parent_child_facet(&self) -> PactResult<bool> {
        Ok(
            read_index_value_from_pages(self, self.index_root, &IndexKey::ParentChildFacet)?
                .is_some(),
        )
    }

    pub(crate) fn component_subtree(
        &self,
        branch: &str,
        component: &str,
    ) -> PactResult<Option<serde_json::Value>> {
        let Some(indexed) = self.component(branch, component)? else {
            return Ok(None);
        };
        let limit = self
            .metadata()?
            .node_counts
            .get(branch)
            .copied()
            .unwrap_or(1)
            .max(1);
        let mut visited = HashSet::new();
        Ok(Some(self.build_component_subtree(
            branch,
            indexed,
            &mut visited,
            limit,
        )?))
    }

    fn build_component_subtree(
        &self,
        branch: &str,
        indexed: super::index::IndexedComponent,
        visited: &mut HashSet<String>,
        limit: usize,
    ) -> PactResult<serde_json::Value> {
        let component_id = indexed
            .value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| PactError::corrupt("indexed Pact component has no id"))?
            .to_string();
        if visited.len() >= limit || !visited.insert(component_id.clone()) {
            return Err(PactError::corrupt(
                "Pact parent-child facet exceeds node count or contains a cycle",
            ));
        }
        let stored_child_ids = indexed.value.get("child_ids").and_then(|value| {
            value.as_array().map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        });
        let child_ids = if indexed.child_count == 0 {
            Vec::new()
        } else if let Some(child_ids) = stored_child_ids {
            if child_ids.len() != indexed.child_count {
                return Err(PactError::corrupt(
                    "Pact component child count disagrees with stored child ids",
                ));
            }
            child_ids
        } else {
            let children = self.parent_children(branch, &component_id)?;
            if children.len() != indexed.child_count {
                return Err(PactError::corrupt(
                    "Pact component child count disagrees with parent-child facet",
                ));
            }
            children
        };
        let mut children = Vec::with_capacity(child_ids.len());
        for child_id in &child_ids {
            let child = self.component(branch, child_id)?.ok_or_else(|| {
                PactError::corrupt("Pact parent-child facet references a missing component")
            })?;
            children.push(self.build_component_subtree(branch, child, visited, limit)?);
        }
        let mut value = indexed.value;
        let object = value
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt("indexed Pact component is not an object"))?;
        object.insert(
            "child_ids".to_string(),
            serde_json::Value::Array(
                child_ids
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        object.insert("children".to_string(), serde_json::Value::Array(children));
        Ok(value)
    }

    pub(crate) fn parent_children(&self, branch: &str, parent: &str) -> PactResult<Vec<String>> {
        let prefix = IndexKey::parent_child_prefix(branch, parent)?;
        let end = prefix_successor(&prefix)?;
        self.index_range_raw(&prefix, &end)?
            .into_iter()
            .map(|(key, _)| match IndexKey::decode(&key)? {
                IndexKey::ParentChild { child, .. } => Ok(child),
                _ => Err(PactError::corrupt(
                    "Pact parent-child range returned another namespace",
                )),
            })
            .collect()
    }

    fn index_range_raw(&self, start: &[u8], end: &[u8]) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        super::btree::CowBTree::from_root(Some(self.index_root)).range(self, start, end)
    }

    fn index_range_raw_limit(
        &self,
        start: &[u8],
        end: &[u8],
        limit: usize,
        reverse: bool,
    ) -> PactResult<Vec<(Vec<u8>, Vec<u8>)>> {
        super::btree::CowBTree::from_root(Some(self.index_root))
            .range_limit(self, start, end, limit, reverse)
    }

    pub(crate) fn content(
        &self,
        content_id: &str,
    ) -> PactResult<Option<crate::store::ContentRecord>> {
        let Some((expected_hash, content_locator)) = self.content_locator(content_id)? else {
            return Ok(None);
        };
        Ok(Some(super::codec::decode_content_record_at(
            &self.shared.mapping,
            content_locator,
            content_id,
            &expected_hash,
        )?))
    }

    pub(crate) fn content_records(
        &self,
        ids: &std::collections::BTreeSet<String>,
    ) -> PactResult<Vec<crate::store::ContentRecord>> {
        let mut by_address: BTreeMap<u64, Vec<(String, [u8; 32])>> = BTreeMap::new();
        for id in ids {
            if let Some((hash, address)) = self.content_locator(id)? {
                by_address
                    .entry(address)
                    .or_default()
                    .push((id.clone(), hash));
            }
        }
        let mut records = Vec::with_capacity(ids.len());
        for (address, wanted) in by_address {
            match super::codec::decode_content_batch_records_at(&self.shared.mapping, address)? {
                Some(batch) => {
                    let mut batch = batch
                        .into_iter()
                        .map(|record| (record.id.clone(), record))
                        .collect::<BTreeMap<_, _>>();
                    for (id, hash) in wanted {
                        let record = batch.remove(&id).ok_or_else(|| {
                            PactError::corrupt("Pact content batch does not contain its indexed id")
                        })?;
                        let (payload, _) = super::codec::captured_content_payload(&record)?;
                        if blake3::hash(payload).as_bytes() != &hash {
                            return Err(PactError::corrupt(
                                "Pact content id index disagrees with its batched payload",
                            ));
                        }
                        records.push(record);
                    }
                }
                None => {
                    for (id, hash) in wanted {
                        records.push(super::codec::decode_content_record_at(
                            &self.shared.mapping,
                            address,
                            &id,
                            &hash,
                        )?);
                    }
                }
            }
        }
        Ok(records)
    }

    pub(crate) fn root_children_subtrees(
        &self,
        branch: &str,
        root_id: &str,
    ) -> PactResult<Vec<serde_json::Value>> {
        let limit = self
            .metadata()?
            .node_counts
            .get(branch)
            .copied()
            .unwrap_or(1)
            .max(1);
        let mut components = self.branch_components_by_batch(branch)?;
        let mut children_of = self.branch_parent_children(branch)?;
        let mut visited = HashSet::new();
        let mut children = Vec::new();
        for child_id in children_of.remove(root_id).unwrap_or_default() {
            children.push(self.assemble_component_subtree(
                &child_id,
                &mut components,
                &mut children_of,
                &mut visited,
                limit,
            )?);
        }
        Ok(children)
    }

    fn branch_components_by_batch(
        &self,
        branch: &str,
    ) -> PactResult<HashMap<String, super::index::IndexedComponent>> {
        let prefix = IndexKey::component_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let entries = self.index_range_raw(&prefix, &end)?;
        let mut components = HashMap::with_capacity(entries.len());
        let mut by_segment: BTreeMap<u64, Vec<(u32, String)>> = BTreeMap::new();
        let mut decompressor = zstd::bulk::Decompressor::new().map_err(|error| {
            PactError::internal(format!("create Pact component decompressor: {error}"))
        })?;
        for (key, facet) in entries {
            let IndexKey::Component { component, .. } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt(
                    "Pact component range returned another namespace",
                ));
            };
            if facet.len() != super::segment::RecordLocator::ENCODED_BYTES {
                components.insert(
                    component,
                    super::index::IndexedComponent::decode_with(&facet, &mut decompressor)?,
                );
                continue;
            }
            let locator = super::segment::RecordLocator::decode(&facet)?;
            by_segment
                .entry(locator.segment_id)
                .or_default()
                .push((locator.ordinal, component));
        }
        for (segment_id, wanted) in by_segment {
            let mut prefix = vec![15];
            prefix.extend_from_slice(&segment_id.to_be_bytes());
            let end = prefix_successor(&prefix)?;

            let wanted_ordinals = wanted
                .iter()
                .map(|(ordinal, _)| *ordinal)
                .collect::<HashSet<u32>>();
            let mut stored: HashMap<u32, Vec<u8>> = HashMap::with_capacity(wanted.len());
            let mut start = prefix.clone();
            loop {
                let page = self.index_range_raw_limit(&start, &end, 256, false)?;
                let Some((last_key, _)) = page.last() else {
                    break;
                };
                let mut next = last_key.clone();
                next.push(0);
                let page_len = page.len();
                for (key, payload) in page {
                    let IndexKey::ComponentPayload { ordinal, .. } = IndexKey::decode(&key)? else {
                        return Err(PactError::corrupt(
                            "Pact component payload range returned another namespace",
                        ));
                    };
                    if wanted_ordinals.contains(&ordinal) {
                        stored.insert(ordinal, payload);
                    }
                }
                if page_len < 256 {
                    break;
                }
                start = next;
            }
            let mut batch = None;
            for (ordinal, component) in wanted {
                let payload = match stored.remove(&ordinal) {
                    Some(payload) => payload,
                    None => {
                        if batch.is_none() {
                            let address = self
                                .index_value(&IndexKey::ComponentBatch { segment_id })?
                                .ok_or_else(|| {
                                    PactError::corrupt(
                                        "Pact component locator has no payload batch",
                                    )
                                })?;
                            let address = u64::from_le_bytes(
                                address.as_slice().try_into().map_err(|_| {
                                    PactError::corrupt(
                                        "Pact component batch address has invalid width",
                                    )
                                })?,
                            );
                            batch = Some(super::codec::decode_component_payload_batch_record_all(
                                self.mapping(),
                                address,
                            )?);
                        }
                        batch
                            .as_mut()
                            .and_then(|batch| batch.remove(&ordinal))
                            .ok_or_else(|| {
                                PactError::corrupt(format!(
                                    "Pact component payload batch has no ordinal {ordinal}"
                                ))
                            })?
                    }
                };
                components.insert(
                    component,
                    super::index::IndexedComponent::decode_with(&payload, &mut decompressor)?,
                );
            }
        }
        self.record_rows_read(components.len());
        Ok(components)
    }

    fn branch_parent_children(&self, branch: &str) -> PactResult<HashMap<String, Vec<String>>> {
        let prefix = IndexKey::parent_child_branch_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
        for (key, _) in self.index_range_raw(&prefix, &end)? {
            let IndexKey::ParentChild { parent, child, .. } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt(
                    "Pact parent-child range returned another namespace",
                ));
            };
            children_of.entry(parent).or_default().push(child);
        }
        Ok(children_of)
    }

    fn assemble_component_subtree(
        &self,
        component_id: &str,
        components: &mut HashMap<String, super::index::IndexedComponent>,
        children_of: &mut HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        limit: usize,
    ) -> PactResult<serde_json::Value> {
        if visited.len() >= limit || !visited.insert(component_id.to_string()) {
            return Err(PactError::corrupt(
                "Pact parent-child facet exceeds node count or contains a cycle",
            ));
        }
        let indexed = components.remove(component_id).ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact parent-child facet references a missing component {component_id}"
            ))
        })?;
        let child_ids = if indexed.child_count == 0 {
            Vec::new()
        } else {
            let children = children_of.remove(component_id).unwrap_or_default();
            if children.len() != indexed.child_count {
                return Err(PactError::corrupt(
                    "Pact component child count disagrees with parent-child facet",
                ));
            }
            children
        };
        let mut children = Vec::with_capacity(child_ids.len());
        for child_id in &child_ids {
            children.push(self.assemble_component_subtree(
                child_id,
                components,
                children_of,
                visited,
                limit,
            )?);
        }
        let mut value = indexed.value;
        let object = value
            .as_object_mut()
            .ok_or_else(|| PactError::corrupt("indexed Pact component is not an object"))?;
        object.insert(
            "child_ids".to_string(),
            serde_json::Value::Array(
                child_ids
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        object.insert("children".to_string(), serde_json::Value::Array(children));
        Ok(value)
    }

    fn content_locator(&self, content_id: &str) -> PactResult<Option<([u8; 32], u64)>> {
        let Some(indexed) = read_index_value_from_pages(
            self,
            self.index_root,
            &IndexKey::ContentId {
                id: content_id.to_string(),
            },
        )?
        else {
            return Ok(None);
        };
        let expected_hash: [u8; 32] = indexed
            .get(..32)
            .ok_or_else(|| PactError::corrupt("Pact content id hash is truncated"))?
            .try_into()
            .map_err(|_| PactError::corrupt("Pact content id hash has invalid width"))?;
        let content_locator = match indexed.len() {
            40 => {
                let address = &indexed[32..];
                u64::from_le_bytes(address.try_into().map_err(|_| {
                    PactError::corrupt("Pact content locator address has invalid width")
                })?)
            }
            32 => {
                let body_address = read_index_value_from_pages(
                    self,
                    self.index_root,
                    &IndexKey::ContentHash {
                        hash: expected_hash,
                    },
                )?
                .ok_or_else(|| PactError::corrupt("Pact content body is missing from the index"))?;
                let body_address = decode_u64_index_value(&body_address, "content body address")?;
                let generation = super::codec::decode_any_record_generation_at(
                    &self.shared.mapping,
                    body_address,
                )?;
                let transaction_address = read_index_value_from_pages(
                    self,
                    self.index_root,
                    &IndexKey::Generation { generation },
                )?
                .ok_or_else(|| {
                    PactError::corrupt("Pact content generation is missing from the index")
                })?;
                decode_u64_index_value(&transaction_address, "content transaction address")?
            }
            _ => {
                return Err(PactError::corrupt(
                    "Pact content id index value has invalid width",
                ));
            }
        };
        Ok(Some((expected_hash, content_locator)))
    }

    pub(crate) fn content_range(
        &self,
        content_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> PactResult<super::search_fields::ContentRange> {
        if max_bytes == 0 || max_bytes > 16 * 1024 * 1024 {
            return Err(PactError::invalid(
                "Pact content range must request between 1 byte and 16 MiB",
            ));
        }
        let Some((_, locator)) = self.content_locator(content_id)? else {
            return Err(PactError::not_found(format!(
                "content {content_id} not found"
            )));
        };
        let Some(descriptor) = super::codec::decode_content_range_descriptor_at(
            &self.shared.mapping,
            locator,
            content_id,
        )?
        else {
            let content = self
                .content(content_id)?
                .ok_or_else(|| PactError::not_found(format!("content {content_id} not found")))?;
            let (payload, is_text) = if let Some(text) = content.storage.inline {
                (text.into_bytes(), true)
            } else if let Some(bytes) = content.storage.inline_bytes {
                (bytes, false)
            } else {
                return Err(PactError::not_found(format!(
                    "content {content_id} has no captured payload"
                )));
            };
            if offset > payload.len() as u64 {
                return Err(PactError::invalid(
                    "Pact content range offset exceeds content length",
                ));
            }
            let start = usize::try_from(offset)
                .map_err(|_| PactError::invalid("Pact content offset exceeds platform limits"))?;
            let end = start.saturating_add(max_bytes).min(payload.len());
            self.counters.record(&IoCounters {
                bytes_read: payload.len() as u64,
                content_ranges_hydrated: 1,
                ..IoCounters::default()
            });
            return finalize_content_range(
                content_id,
                offset,
                payload.len() as u64,
                payload[start..end].to_vec(),
                is_text,
            );
        };
        if offset > descriptor.record.bytes as u64 {
            return Err(PactError::invalid(
                "Pact content range offset exceeds content length",
            ));
        }
        let (first_chunk, mut chunk_start) = super::search_fields::content_chunk_start(
            &descriptor.body_offsets,
            descriptor.body_addresses.len(),
            offset,
        );
        let mut data = Vec::with_capacity(max_bytes);
        let mut decoded_bytes = 0_u64;
        for address in descriptor.body_addresses.into_iter().skip(first_chunk) {
            let chunk_len = super::codec::content_body_decoded_len(&self.shared.mapping, address)?;
            let chunk_end = chunk_start.saturating_add(chunk_len as u64);
            if chunk_end <= offset {
                chunk_start = chunk_end;
                continue;
            }
            if data.len() >= max_bytes {
                break;
            }
            let chunk = super::codec::decode_content_body_record(&self.shared.mapping, address)?;
            decoded_bytes = decoded_bytes.saturating_add(chunk.len() as u64);
            let local_start = usize::try_from(offset.saturating_sub(chunk_start))
                .map_err(|_| PactError::invalid("Pact content offset exceeds platform limits"))?;
            let available = max_bytes.saturating_sub(data.len());
            let local_end = local_start.saturating_add(available).min(chunk.len());
            data.extend_from_slice(&chunk[local_start..local_end]);
            chunk_start = chunk_end;
        }
        self.counters.record(&IoCounters {
            bytes_read: decoded_bytes,
            content_ranges_hydrated: 1,
            ..IoCounters::default()
        });
        finalize_content_range(
            content_id,
            offset,
            descriptor.record.bytes as u64,
            data,
            !descriptor.body_is_bytes,
        )
    }

    fn content_hash(&self, content_id: &str) -> PactResult<Option<String>> {
        let Some(indexed) = read_index_value_from_pages(
            self,
            self.index_root,
            &IndexKey::ContentId {
                id: content_id.to_string(),
            },
        )?
        else {
            return Ok(None);
        };
        let hash: [u8; 32] = indexed
            .get(..32)
            .ok_or_else(|| PactError::corrupt("Pact content id hash is truncated"))?
            .try_into()
            .map_err(|_| PactError::corrupt("Pact content id hash has invalid width"))?;
        Ok(Some(blake3::Hash::from_bytes(hash).to_hex().to_string()))
    }

    pub(crate) fn components_by_type(
        &self,
        branch: &str,
        component_type: &str,
        offset: usize,
        limit: usize,
    ) -> PactResult<(usize, Vec<super::index::IndexedComponent>)> {
        let prefix = IndexKey::type_prefix(branch, component_type)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let (total, entries) = tree.range_window(self, &prefix, &end, offset, limit)?;
        let mut components = Vec::with_capacity(entries.len());
        for (key, facet) in entries {
            let IndexKey::Type { component, .. } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt(
                    "Pact type index returned a non-type key",
                ));
            };
            let indexed = if facet.len() == super::segment::RecordLocator::ENCODED_BYTES {
                self.component_from_facet(&facet)?
            } else {
                self.component(branch, &component)?.ok_or_else(|| {
                    PactError::corrupt("Pact type index references a missing component")
                })?
            };
            components.push(self.attach_child_ids(branch, indexed)?);
        }
        self.record_rows_read(components.len());
        Ok((total, components))
    }

    pub(crate) fn components_by_aux(
        &self,
        branch: &str,
        key: &str,
        value: &serde_json::Value,
        offset: usize,
        limit: usize,
    ) -> PactResult<(usize, Vec<super::index::IndexedComponent>)> {
        let prefix = IndexKey::aux_prefix(branch, key, super::index::index_value_hash(value)?)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let (total, entries) = tree.range_window(self, &prefix, &end, offset, limit)?;
        let mut components = Vec::with_capacity(entries.len());
        for (key, facet) in entries {
            let IndexKey::Aux { component, .. } = IndexKey::decode(&key)? else {
                return Err(PactError::corrupt("Pact aux index returned a non-aux key"));
            };
            let indexed = if facet.len() == super::segment::RecordLocator::ENCODED_BYTES {
                self.component_from_facet(&facet)?
            } else {
                self.component(branch, &component)?.ok_or_else(|| {
                    PactError::corrupt("Pact aux index references a missing component")
                })?
            };
            components.push(self.attach_child_ids(branch, indexed)?);
        }
        self.record_rows_read(components.len());
        Ok((total, components))
    }

    pub(crate) fn components_by_tag_query(
        &self,
        branch: &str,
        query: &str,
    ) -> PactResult<Vec<super::index::IndexedComponent>> {
        const PAGE_ROWS: usize = 4096;
        let prefix = IndexKey::tag_branch_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let mut offset = 0;
        let mut component_ids = std::collections::BTreeSet::new();
        loop {
            let (total, entries) = tree.range_window(self, &prefix, &end, offset, PAGE_ROWS)?;
            if entries.is_empty() {
                break;
            }
            offset = offset.saturating_add(entries.len());
            for (key, _) in entries {
                let IndexKey::Tag { tag, component, .. } = IndexKey::decode(&key)? else {
                    return Err(PactError::corrupt("Pact tag index returned a non-tag key"));
                };
                if crate::selector::tag_matches_query(&tag, query) {
                    component_ids.insert(component);
                }
            }
            if offset >= total {
                break;
            }
        }
        let mut components = Vec::with_capacity(component_ids.len());
        for component_id in component_ids {
            let component = self
                .component_shallow(branch, &component_id)?
                .ok_or_else(|| {
                    PactError::corrupt("Pact tag index references a missing component")
                })?;
            components.push(component);
        }
        self.record_rows_read(components.len());
        Ok(components)
    }

    pub(crate) fn components_shallow_window(
        &self,
        branch: &str,
        offset: usize,
        limit: usize,
    ) -> PactResult<(usize, Vec<super::index::IndexedComponent>)> {
        let prefix = IndexKey::component_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let (total, entries) = tree.range_window(self, &prefix, &end, offset, limit)?;
        let mut components = Vec::with_capacity(entries.len());
        for (_, facet) in entries {
            components.push(self.component_from_facet(&facet)?);
        }
        self.record_rows_read(components.len());
        Ok((total, components))
    }

    pub(crate) fn visit_components_shallow_window<F>(
        &self,
        branch: &str,
        offset: usize,
        limit: usize,
        mut visit: F,
    ) -> PactResult<(usize, usize)>
    where
        F: FnMut(super::index::IndexedComponent) -> PactResult<bool>,
    {
        let prefix = IndexKey::component_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let (total, entries) = tree.range_window(self, &prefix, &end, offset, limit)?;
        let mut visited = 0;
        for (_, facet) in entries {
            let component = self.component_from_facet(&facet)?;
            visited += 1;
            if !visit(component)? {
                break;
            }
        }
        self.record_rows_read(visited);
        Ok((total, visited))
    }

    pub(crate) fn component_count_and_rank(
        &self,
        branch: &str,
        component: &str,
    ) -> PactResult<(usize, usize)> {
        let prefix = IndexKey::component_prefix(branch)?;
        let end = prefix_successor(&prefix)?;
        let tree = super::btree::CowBTree::from_root(Some(self.index_root));
        let (total, _) = tree.range_window(self, &prefix, &end, 0, 0)?;
        let mut component_key = prefix.clone();
        component_key.extend_from_slice(component.as_bytes());
        let (rank, _) = tree.range_window(self, &prefix, &component_key, 0, 0)?;
        Ok((total, rank))
    }

    pub(crate) fn metadata(&self) -> PactResult<super::index::HeadMetadata> {
        let mut metadata = self
            .metadata_optional()?
            .ok_or_else(|| PactError::corrupt("Pact snapshot is missing head metadata"))?;
        metadata.source_file_identity = self.source_file_identity();
        Ok(metadata)
    }

    pub(crate) fn metadata_optional(&self) -> PactResult<Option<super::index::HeadMetadata>> {
        let bytes = read_index_value_from_pages(self, self.index_root, &IndexKey::Metadata)?
            .map(|bytes| super::index::HeadMetadata::decode(&bytes))
            .transpose()?;
        Ok(bytes)
    }

    pub(crate) fn release_oversized_cache_entries(&self) {
        if let Ok(mut pages) = self.caches.pages.lock() {
            pages.release_oversized();
        }
        if let Ok(mut components) = self.caches.components.lock() {
            components.release_oversized();
        }
    }

    pub(crate) fn record_rows_read(&self, count: usize) {
        self.counters.record(&IoCounters {
            rows_materialized: count as u64,
            ..IoCounters::default()
        });
    }

    pub(crate) fn record_component_hydrated(&self) {
        self.counters.record(&IoCounters {
            components_hydrated: 1,
            ..IoCounters::default()
        });
    }

    pub(crate) fn shares_mapping_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    pub(crate) fn latest_transaction(&self) -> PactResult<super::transaction::PactTransaction> {
        let start = usize::try_from(self.transaction_root)
            .map_err(|_| PactError::corrupt("Pact transaction address exceeds platform limits"))?;
        let end = usize::try_from(self.committed_len)
            .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;
        let region = self
            .shared
            .mapping
            .get(start..end)
            .ok_or_else(|| PactError::corrupt("Pact transaction exceeds snapshot watermark"))?;
        let decoded = decode_transaction_region(
            region,
            self.transaction_root,
            self.generation,
            self.committed_len,
            self,
        )?;
        self.counters.record(&IoCounters {
            bytes_read: region.len() as u64,
            records_decoded: 1,
            ..IoCounters::default()
        });
        self.hydrate_transaction(decoded.transaction, decoded.marker.index_root_address)
    }

    pub(crate) fn transaction_at(
        &self,
        address: u64,
    ) -> PactResult<super::transaction::PactTransaction> {
        let (transaction, index_root) = self.decode_transaction_at(address)?;
        self.hydrate_transaction(transaction, index_root)
    }

    pub(crate) fn decode_transaction_at(
        &self,
        address: u64,
    ) -> PactResult<(super::transaction::PactTransaction, u64)> {
        self.decode_transaction_at_with(address, None)
    }

    pub(crate) fn decode_transaction_at_with(
        &self,
        address: u64,
        derived_branches: Option<&std::collections::BTreeSet<String>>,
    ) -> PactResult<(super::transaction::PactTransaction, u64)> {
        let generation = super::codec::decode_record_generation_at(&self.shared.mapping, address)?;
        let decoded = super::codec::decode_transaction_with(
            &self.shared.mapping,
            address,
            generation,
            derived_branches,
        )?;
        self.counters.record(&IoCounters {
            bytes_read: decoded.marker.file_length_watermark.saturating_sub(address),
            records_decoded: 1,
            ..IoCounters::default()
        });
        Ok((decoded.transaction, decoded.marker.index_root_address))
    }

    fn public_traces_unresolved(
        &self,
    ) -> PactResult<BTreeMap<String, crate::store::PactTransactionTrace>> {
        let entries = self.index_range(
            &IndexKey::Generation { generation: 0 },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            usize::try_from(self.generation)
                .map_err(|_| PactError::corrupt("Pact generation exceeds platform limits"))?,
            false,
        )?;
        let mut traces = BTreeMap::<String, crate::store::PactTransactionTrace>::new();
        for (_, address) in entries {
            let address = decode_u64_index_value(&address, "transaction address")?;
            let (transaction, _) = self.decode_transaction_at(address)?;
            if let Some(snapshot) = transaction.head_checkpoint.as_ref() {
                let ledger = if super::head_checkpoint::is_compact_ledger(snapshot) {
                    snapshot.get("ledger").unwrap_or(snapshot)
                } else {
                    snapshot
                };
                if let Some(values) = ledger.pointer("/event_index/transaction_traces") {
                    let baseline = serde_json::from_value::<
                        BTreeMap<String, crate::store::PactTransactionTrace>,
                    >(values.clone())
                    .map_err(|error| {
                        PactError::corrupt(format!(
                            "invalid Pact baseline transaction traces: {error}"
                        ))
                    })?;
                    traces.extend(baseline);
                }
            }
            if transaction
                .ledger_delta
                .as_ref()
                .is_some_and(|delta| delta.record_public_trace)
            {
                let mut public_trace = transaction.public_trace;
                public_trace.replay_trace = Some(transaction.trace);
                traces.insert(public_trace.transaction_id.clone(), public_trace);
            }
        }
        Ok(traces)
    }

    pub(crate) fn public_traces(
        &self,
        branch: &str,
    ) -> PactResult<Vec<crate::store::PactTransactionTrace>> {
        let mut traces = self.public_traces_unresolved()?;
        let marker_ids = traces
            .values()
            .filter_map(|trace| trace.replay_trace.as_ref())
            .flat_map(|trace| &trace.ops)
            .filter_map(|op| match &op.kind {
                crate::transaction_trace::TraceOpKind::InsertComponent { component, .. } => {
                    component
                        .get("$pact_head_component")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                }
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        if !marker_ids.is_empty() {
            let mut components = BTreeMap::new();
            for component_id in marker_ids {
                let component = self.component(branch, &component_id)?.ok_or_else(|| {
                    PactError::corrupt("Pact public trace references a missing canonical component")
                })?;
                components.insert(component_id, component.value);
            }
            super::head_checkpoint::resolve_head_component_refs(&mut traces, &components)?;
        }
        Ok(traces
            .into_values()
            .filter(|trace| trace.branch_id == branch)
            .collect())
    }

    pub(crate) fn baseline_message_order(&self, branch: &str) -> PactResult<Vec<String>> {
        let first = self.index_range(
            &IndexKey::Generation { generation: 0 },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            1,
            false,
        )?;
        let Some((_, address)) = first.into_iter().next() else {
            return Ok(Vec::new());
        };
        let address = decode_u64_index_value(&address, "baseline transaction address")?;
        let (transaction, _) = self.decode_transaction_at(address)?;
        let Some(snapshot) = transaction.head_checkpoint else {
            return Ok(Vec::new());
        };
        let ledger = if super::head_checkpoint::is_compact_ledger(&snapshot) {
            snapshot.get("ledger").unwrap_or(&snapshot)
        } else {
            &snapshot
        };
        serde_json::from_value(
            ledger
                .get("branches")
                .and_then(|branches| branches.get(branch))
                .and_then(|branch| branch.pointer("/current/message_order"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|error| {
            PactError::corrupt(format!("invalid Pact baseline message order: {error}"))
        })
    }

    pub(crate) fn transactions_in_generation_order(
        &self,
    ) -> PactResult<Vec<super::transaction::PactTransaction>> {
        let entries = self.index_range(
            &IndexKey::Generation { generation: 0 },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            usize::try_from(self.generation)
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
                self.transaction_at(u64::from_le_bytes(raw))
            })
            .collect()
    }

    pub(crate) fn replay_ledger(&self) -> PactResult<crate::store::ContextTreeLedger> {
        let derived_branches = self
            .metadata_optional()?
            .map(|metadata| metadata.derived_depth_branches)
            .unwrap_or_default();
        let mut stream = super::transaction_stream::TransactionStream::new(self);
        if !derived_branches.is_empty() {
            stream = stream.skipping_derived_depth(derived_branches.clone());
        }
        let (mut baseline, baseline_index_root) = stream
            .next()
            .ok_or_else(|| PactError::corrupt("Pact file has no baseline transaction"))??;
        let baseline_value = baseline.head_checkpoint.take().ok_or_else(|| {
            PactError::corrupt("latest Pact transaction is missing its ledger checkpoint")
        })?;
        let mut ledger = super::head_checkpoint::hydrate_ledger_typed_from_pages_owned(
            baseline_index_root,
            baseline_value,
            self,
        )?;
        super::strip_derived_depth_ops(&mut ledger, &derived_branches);
        super::replay_transactions_from_baseline(
            ledger,
            baseline.new_content,
            stream.map(|item| item.map(|(transaction, _)| transaction)),
        )
    }

    fn hydrate_transaction(
        &self,
        mut transaction: super::transaction::PactTransaction,
        index_root: u64,
    ) -> PactResult<super::transaction::PactTransaction> {
        if transaction
            .head_checkpoint
            .as_ref()
            .is_some_and(super::head_checkpoint::is_compact_ledger)
        {
            transaction.head_checkpoint = Some(super::head_checkpoint::hydrate_ledger(
                &self.shared.mapping,
                index_root,
                transaction
                    .head_checkpoint
                    .as_ref()
                    .ok_or_else(|| PactError::internal("compact Pact ledger disappeared"))?,
            )?);
        }
        Ok(transaction)
    }
}

fn decode_u64_index_value(value: &[u8], label: &str) -> PactResult<u64> {
    let raw: [u8; 8] = value
        .try_into()
        .map_err(|_| PactError::corrupt(format!("Pact {label} has invalid width")))?;
    Ok(u64::from_le_bytes(raw))
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

impl Snapshot {
    pub(crate) fn committed_len(&self) -> u64 {
        self.committed_len
    }

    pub(crate) fn platform_id(&self) -> (u64, u128) {
        self.caches.identity.1
    }

    pub(crate) fn mapping(&self) -> &[u8] {
        &self.shared.mapping
    }

    pub(crate) fn index_root(&self) -> u64 {
        self.index_root
    }

    pub(crate) fn transaction_root(&self) -> u64 {
        self.transaction_root
    }
}

#[derive(Clone, Eq)]
struct MappingKey {
    path: PathBuf,
    file_len: u64,
    generation: u64,
    committed_len: u64,
    platform_id: (u64, u128),
}

impl MappingKey {
    fn new(
        path: &Path,
        metadata: &std::fs::Metadata,
        generation: u64,
        committed_len: u64,
        platform_id: (u64, u128),
    ) -> Self {
        Self {
            path: path.to_path_buf(),
            file_len: metadata.len(),
            generation,
            committed_len,
            platform_id,
        }
    }
}

impl PartialEq for MappingKey {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.file_len == other.file_len
            && self.generation == other.generation
            && self.committed_len == other.committed_len
            && self.platform_id == other.platform_id
    }
}

impl Hash for MappingKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.file_len.hash(state);
        self.generation.hash(state);
        self.committed_len.hash(state);
        self.platform_id.hash(state);
    }
}

fn shared_mapping(
    key: MappingKey,
    file: &File,
    committed_len: u64,
) -> PactResult<Arc<SharedSnapshotData>> {
    static MAPPINGS: OnceLock<Mutex<MappingRegistry>> = OnceLock::new();
    let registry = MAPPINGS.get_or_init(|| Mutex::new(MappingRegistry::default()));
    let mut registry = registry
        .lock()
        .map_err(|_| PactError::internal("Pact snapshot mapping registry is poisoned"))?;
    registry.open(key, file, committed_len)
}

#[derive(Default)]
struct MappingRegistry {
    mappings: HashMap<MappingKey, Weak<SharedSnapshotData>>,
}

impl MappingRegistry {
    fn open(
        &mut self,
        key: MappingKey,
        file: &File,
        committed_len: u64,
    ) -> PactResult<Arc<SharedSnapshotData>> {
        if let Some(mapping) = self.mappings.get(&key).and_then(Weak::upgrade) {
            return Ok(mapping);
        }
        let len = usize::try_from(committed_len)
            .map_err(|_| PactError::corrupt("Pact committed length exceeds platform limits"))?;

        let mapping = unsafe { MmapOptions::new().len(len).map(file) }?;
        let mapping = Arc::new(SharedSnapshotData { mapping });
        self.mappings.retain(|_, value| value.strong_count() > 0);
        self.mappings.insert(key, Arc::downgrade(&mapping));
        Ok(mapping)
    }
}

struct SharedSnapshotData {
    mapping: Mmap,
}

struct SnapshotCaches {
    identity: ([u8; super::layout::FILE_IDENTITY_BYTES], (u64, u128)),
    options: crate::PactMemoryOptions,
    pages: Mutex<CacheDomain<Vec<[u8; super::btree::PAGE_BYTES]>>>,

    page_entries: Mutex<super::byte_lru::ByteLru<u64, [u8; super::btree::PAGE_BYTES]>>,
    components: Mutex<CacheDomain<BTreeMap<u32, Vec<u8>>>>,
}

impl SnapshotCaches {
    fn new(
        options: &crate::PactMemoryOptions,
        file_identity: [u8; super::layout::FILE_IDENTITY_BYTES],
        platform_id: (u64, u128),
    ) -> Self {
        Self {
            identity: (file_identity, platform_id),
            options: options.clone(),
            pages: Mutex::new(CacheDomain::new(options.page_cache_bytes)),
            page_entries: Mutex::new(super::byte_lru::ByteLru::new(options.page_cache_bytes)),
            components: Mutex::new(CacheDomain::new(options.component_cache_bytes)),
        }
    }
}

struct CacheDomain<V> {
    lru: super::byte_lru::ByteLru<u64, Arc<V>>,
    oversized: Option<(u64, Arc<V>, usize)>,
}

impl<V> CacheDomain<V> {
    fn new(budget: usize) -> Self {
        Self {
            lru: super::byte_lru::ByteLru::new(budget),
            oversized: None,
        }
    }

    fn get_or_decode(
        &mut self,
        key: u64,
        charge: impl FnOnce(&V) -> usize,
        decode: impl FnOnce() -> PactResult<V>,
    ) -> PactResult<Arc<V>> {
        if let Some(value) = self.lru.get(&key) {
            return Ok(Arc::clone(value));
        }
        if let Some((held, value, _)) = &self.oversized
            && *held == key
        {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(decode()?);
        let bytes = charge(&value);
        if self.lru.insert(key, Arc::clone(&value), bytes).is_err() {
            self.release_oversized();
            super::memory_accounting::cache_resident_add(bytes);
            self.oversized = Some((key, Arc::clone(&value), bytes));
        }
        Ok(value)
    }

    fn release_oversized(&mut self) {
        if let Some((_, _, bytes)) = self.oversized.take() {
            super::memory_accounting::cache_resident_sub(bytes);
        }
    }
}

impl<V> Drop for CacheDomain<V> {
    fn drop(&mut self) {
        self.release_oversized();
    }
}

impl super::btree::PageReader for Snapshot {
    fn read_page(&self, address: u64) -> PactResult<[u8; super::btree::PAGE_BYTES]> {
        if let Some(page) = self
            .caches
            .page_entries
            .lock()
            .map_err(|_| PactError::internal("Pact snapshot page cache is poisoned"))?
            .get(&address)
        {
            return Ok(*page);
        }
        let batch_address = super::codec::index_page_batch_address(address);
        let ordinal = super::codec::index_page_ordinal(address);
        let pages = {
            let mut cache = self
                .caches
                .pages
                .lock()
                .map_err(|_| PactError::internal("Pact snapshot page cache is poisoned"))?;
            cache.get_or_decode(
                batch_address,
                |pages| pages.len().saturating_mul(super::btree::PAGE_BYTES),
                || {
                    self.counters.record(&IoCounters {
                        bytes_read: (RECORD_HEADER_BYTES + super::btree::PAGE_BYTES) as u64,
                        pages_read: 1,
                        ..IoCounters::default()
                    });
                    decode_index_page_batch_record(&self.shared.mapping, batch_address)
                },
            )?
        };
        let page = pages
            .get(ordinal)
            .copied()
            .ok_or_else(|| PactError::corrupt("Pact index page ordinal is out of range"))?;
        let _ = self
            .caches
            .page_entries
            .lock()
            .map_err(|_| PactError::internal("Pact snapshot page cache is poisoned"))?
            .insert(address, page, super::btree::PAGE_BYTES + 32);
        Ok(page)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> PactResult<Vec<u8>> {
        let batch = {
            let mut cache = self.caches.components.lock().map_err(|_| {
                PactError::internal("Pact snapshot component batch cache is poisoned")
            })?;
            cache.get_or_decode(
                address,
                |batch| {
                    batch
                        .values()
                        .fold(0_usize, |total, payload| {
                            total.saturating_add(payload.len())
                        })
                        .saturating_add(batch.len().saturating_mul(32))
                },
                || {
                    super::codec::decode_component_payload_batch_record_all(
                        &self.shared.mapping,
                        address,
                    )
                },
            )?
        };
        batch.get(&ordinal).cloned().ok_or_else(|| {
            PactError::corrupt(format!(
                "Pact component payload batch has no ordinal {ordinal}"
            ))
        })
    }
}

impl ContentBodySource for Snapshot {
    fn read_content_body(&self, address: u64) -> PactResult<Vec<u8>> {
        decode_content_body_record(&self.shared.mapping, address)
    }
}

fn finalize_content_range(
    content_id: &str,
    offset: u64,
    total_bytes: u64,
    mut data: Vec<u8>,
    is_text: bool,
) -> PactResult<super::search_fields::ContentRange> {
    if is_text && !data.is_empty() {
        if data[0] & 0b1100_0000 == 0b1000_0000 {
            return Err(PactError::invalid(
                "Pact text content offset is not a UTF-8 boundary",
            ));
        }
        if let Err(error) = std::str::from_utf8(&data) {
            if error.error_len().is_some() {
                return Err(PactError::corrupt("Pact text content is not UTF-8"));
            }
            data.truncate(error.valid_up_to());
        }
    }
    let consumed = offset.saturating_add(data.len() as u64);
    Ok(super::search_fields::ContentRange {
        content_id: content_id.to_string(),
        offset,
        data,
        next_offset: (consumed < total_bytes).then_some(consumed),
    })
}

fn read_range(file: &mut File, address: u64, len: u64) -> PactResult<Vec<u8>> {
    let len = usize::try_from(len)
        .map_err(|_| PactError::corrupt("Pact snapshot read exceeds platform limits"))?;
    let mut bytes = vec![0_u8; len];
    file.seek(SeekFrom::Start(address))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
