use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

use crate::error::{PactError, PactResult};

pub struct PactFileReadHandle {
    snapshot: crate::pact_file::snapshot::Snapshot,
    path: PathBuf,
    branch_id: String,
    hydrated: Mutex<Option<crate::ContextTree>>,
}

impl PactFileReadHandle {
    pub fn open(path: impl AsRef<Path>, branch_id: Option<&str>) -> PactResult<Self> {
        Self::open_with_memory_options(path, branch_id, crate::PactMemoryOptions::default())
    }

    pub fn open_with_memory_options(
        path: impl AsRef<Path>,
        branch_id: Option<&str>,
        options: crate::PactMemoryOptions,
    ) -> PactResult<Self> {
        let path = path.as_ref().to_path_buf();
        let snapshot = crate::pact_file::snapshot::Snapshot::open_with_memory_options(
            &path,
            Arc::new(crate::pact_file::backend::IoCounterSet::default()),
            &options,
        )?;
        let metadata = snapshot.metadata()?;
        let branch_id = branch_id
            .map(str::to_string)
            .unwrap_or_else(|| metadata.current_branch_id.clone());
        if snapshot.branch_head(&branch_id)?.is_none() {
            return Err(PactError::not_found(format!(
                "branch not found: {branch_id}"
            )));
        }
        Ok(Self {
            snapshot,
            path,
            branch_id,
            hydrated: Mutex::new(None),
        })
    }

    pub fn info(&self) -> PactResult<Value> {
        let metadata = self.snapshot.metadata()?;
        let revision = self
            .snapshot
            .branch_head(&self.branch_id)?
            .ok_or_else(|| PactError::not_found(format!("branch not found: {}", self.branch_id)))?;
        let stats = metadata
            .stats
            .get(&self.branch_id)
            .ok_or_else(|| PactError::corrupt("Pact head metadata is missing branch stats"))?;
        let source_file_identity = self.snapshot.source_file_identity();
        Ok(json!({
            "context_tree_id": metadata.root_id,
            "source_file_identity": source_file_identity,
            "branch_id": self.branch_id,
            "revision": revision,
            "stats": stats,
            "capabilities": [
                "project",
                "component",
                "content_payload",
                "search_selector_eligibility",
                "search_source_page",
                "read_search_field",
                "hydrate_search_hits",
                "traces",
                "stream",
                "extensions",
                "extension",
                "rewind_targets",
                "refresh",
            ],
        }))
    }

    pub fn search_snapshot_info(&self) -> PactResult<crate::PactSearchSnapshotInfo> {
        if !self.snapshot.has_search_change_facet()? {
            return Err(PactError::unsupported(
                "Pact file predates the bounded search-change facet; rewrite it with the current candidate format",
            ));
        }
        let metadata = self.snapshot.metadata()?;
        let head_revision = self
            .snapshot
            .branch_head(&self.branch_id)?
            .ok_or_else(|| PactError::not_found(format!("branch not found: {}", self.branch_id)))?;
        let (revision, ordinal) = self
            .snapshot
            .latest_search_change_position(&self.branch_id)?
            .unwrap_or((0, 0));
        let cursor = crate::PactSearchChangeCursor {
            source_file_identity: self.snapshot.source_file_identity(),
            root_id: metadata.root_id.clone(),
            branch_id: self.branch_id.clone(),
            revision,
            ordinal,
        };
        Ok(crate::PactSearchSnapshotInfo {
            source_file_identity: cursor.source_file_identity.clone(),
            root_id: metadata.root_id,
            branch_id: self.branch_id.clone(),
            head_revision,
            search_cursor: cursor,
            physical_generation: self.snapshot.generation(),
        })
    }

    pub fn search_selector_eligibility(
        &self,
        selector: &str,
        allow_expensive_selector: bool,
    ) -> PactResult<crate::search_contract::PactSearchSelectorEligibility> {
        let plan = crate::pact_file::search_selector::execute(
            &self.snapshot,
            &self.branch_id,
            selector,
            allow_expensive_selector,
        )?;
        Ok((&plan).into())
    }

    pub fn search_source_page(
        &self,
        after_ordinal: Option<u64>,
        max_components: usize,
        max_metadata_bytes: usize,
    ) -> PactResult<crate::PactSearchSourcePage> {
        if max_components == 0 || max_components > 4096 {
            return Err(PactError::invalid(
                "Pact search source limit must be between 1 and 4096",
            ));
        }
        if max_metadata_bytes == 0 || max_metadata_bytes > 16 * 1024 * 1024 {
            return Err(PactError::invalid(
                "Pact search source metadata budget must be between 1 byte and 16 MiB",
            ));
        }
        let offset = usize::try_from(after_ordinal.unwrap_or(0)).map_err(|_| {
            PactError::invalid("Pact search source ordinal exceeds platform limits")
        })?;
        let metadata = self.snapshot.metadata()?;
        if metadata.composition_branches.contains(&self.branch_id) {
            return Err(PactError::unsupported(
                "Pact search source requires a directly indexed branch",
            ));
        }
        let total = metadata
            .node_counts
            .get(&self.branch_id)
            .copied()
            .ok_or_else(|| PactError::not_found(format!("branch not found: {}", self.branch_id)))?
            .checked_sub(1)
            .ok_or_else(|| PactError::corrupt("Pact component surface has no context root"))?;
        if offset > total {
            return Err(PactError::invalid(
                "Pact search source ordinal exceeds the component surface",
            ));
        }
        let total_components = u64::try_from(total)
            .map_err(|_| PactError::invalid("Pact component count exceeds u64"))?;
        let mut components = Vec::with_capacity(max_components.min(total.saturating_sub(offset)));
        let mut encoded_component_bytes = 0_usize;
        let terminal_len = encoded_search_source_page_len(total_components, 0, 0, None)?;
        if offset == total && terminal_len > max_metadata_bytes {
            return Err(PactError::invalid(
                "Pact search source metadata budget is smaller than the terminal page",
            ));
        }
        let mut budget_exhausted = false;
        let (indexed_total, visited) = self.snapshot.visit_components_shallow_window(
            &self.branch_id,
            offset,
            max_components.min(total.saturating_sub(offset)),
            |indexed| {
                let rank = offset + components.len();
                let ordinal = u64::try_from(rank + 1)
                    .map_err(|_| PactError::invalid("Pact search source ordinal exceeds u64"))?;
                let component = search_source_component(ordinal, &indexed.value)?;
                let next_ordinal = (ordinal < total_components).then_some(ordinal);
                let component_bytes = serde_json::to_vec(&component)
                    .map_err(|error| {
                        PactError::codec(format!("encode Pact search source: {error}"))
                    })?
                    .len();
                let candidate_len = encoded_search_source_page_len(
                    total_components,
                    encoded_component_bytes.saturating_add(component_bytes),
                    components.len() + 1,
                    next_ordinal,
                )?;
                if candidate_len > max_metadata_bytes {
                    budget_exhausted = true;
                    return Ok(false);
                }
                encoded_component_bytes = encoded_component_bytes.saturating_add(component_bytes);
                components.push(component);
                Ok(true)
            },
        )?;
        if indexed_total != total {
            return Err(PactError::corrupt(
                "Pact component index count disagrees with head metadata",
            ));
        }
        if budget_exhausted && components.is_empty() {
            return Err(PactError::invalid(
                "Pact search source metadata budget is smaller than the next component",
            ));
        }
        if visited == 0 && offset < total {
            return Err(PactError::corrupt(
                "Pact component index ended before its declared count",
            ));
        }
        let last_ordinal = components.last().map(|component| component.ordinal);
        let next_ordinal = last_ordinal
            .filter(|ordinal| *ordinal < total_components)
            .or_else(|| (components.is_empty() && offset < total).then_some(offset as u64));
        Ok(crate::PactSearchSourcePage {
            total_components,
            components,
            next_ordinal,
        })
    }

    pub fn stream_content_range(
        &self,
        content_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> PactResult<crate::ContentRange> {
        self.snapshot.content_range(content_id, offset, max_bytes)
    }

    pub fn read_search_field(
        &self,
        locator: &crate::SearchFieldLocator,
        max_bytes: usize,
    ) -> PactResult<crate::SearchFieldPage> {
        let component = self
            .snapshot
            .component_shallow(&self.branch_id, &locator.component_id)?
            .ok_or_else(|| {
                PactError::not_found(format!("component {} not found", locator.component_id))
            })?;
        self.snapshot.record_component_hydrated();
        if let Some(ref_field) = search_field_ref_name(&locator.field)
            && let Some(content_id) = component.value.get(ref_field).and_then(content_ref_id)
        {
            let range = self.stream_content_range(content_id, locator.offset, max_bytes)?;
            return Ok(crate::SearchFieldPage {
                locator: locator.clone(),
                data: range.data,
                next_offset: range.next_offset,
            });
        }
        let value = field_value(&component.value, &locator.field).ok_or_else(|| {
            PactError::not_found(format!(
                "component {} has no field {}",
                locator.component_id, locator.field
            ))
        })?;
        let bytes = match value {
            Value::String(text) => text.as_bytes().to_vec(),
            other => serde_json::to_vec(other)
                .map_err(|error| PactError::codec(format!("encode Pact search field: {error}")))?,
        };
        let range = bounded_utf8_bytes(&bytes, locator.offset, max_bytes)?;
        Ok(crate::SearchFieldPage {
            locator: locator.clone(),
            data: range.0,
            next_offset: range.1,
        })
    }

    pub fn hydrate_search_hits(
        &self,
        hits: &[crate::SearchHitLocator],
        max_total_bytes: usize,
    ) -> PactResult<Vec<crate::HydratedSearchHit>> {
        if max_total_bytes == 0 || max_total_bytes > 16 * 1024 * 1024 {
            return Err(PactError::invalid(
                "Pact search hydration budget must be between 1 byte and 16 MiB",
            ));
        }
        let mut remaining = max_total_bytes;
        let mut hydrated = Vec::with_capacity(hits.len());
        for hit in hits {
            if remaining == 0 {
                return Err(PactError::invalid(
                    "Pact search hydration exceeds its total byte budget",
                ));
            }
            let field = self.read_search_field(
                &crate::SearchFieldLocator {
                    component_id: hit.component_id.clone(),
                    field: hit.field.clone(),
                    offset: hit.offset,
                },
                hit.max_bytes.min(remaining),
            )?;
            remaining = remaining.saturating_sub(field.data.len());
            hydrated.push(crate::HydratedSearchHit {
                component_id: hit.component_id.clone(),
                field,
            });
        }
        Ok(hydrated)
    }

    pub fn stream(
        &self,
        mode: &str,
        anchor_row_id: Option<&str>,
        anchor_seq: Option<u64>,
        direction: &str,
        limit: usize,
    ) -> PactResult<Value> {
        self.stream_branch(mode, None, anchor_row_id, anchor_seq, direction, limit)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stream_branch(
        &self,
        mode: &str,
        branch_id: Option<&str>,
        anchor_row_id: Option<&str>,
        anchor_seq: Option<u64>,
        direction: &str,
        limit: usize,
    ) -> PactResult<Value> {
        let normalized_mode = crate::file_format::normalize_stream_mode(mode)?;
        let branch_id = branch_id.unwrap_or(&self.branch_id);
        let requires_composition = self
            .snapshot
            .metadata()?
            .composition_branches
            .contains(branch_id);
        if normalized_mode == "message_surface" {
            if requires_composition {
                return Err(PactError::unsupported(
                    "message_surface requires a directly indexed branch",
                ));
            }
            return crate::pact_file::message_stream::window(
                &self.snapshot,
                &self.path.display().to_string(),
                branch_id,
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                "held_tree",
            );
        }
        if normalized_mode == "current_surface" && !requires_composition {
            return crate::file_format::indexed_current_stream_from_snapshot(
                &self.snapshot,
                &self.path.display().to_string(),
                Some(branch_id),
                anchor_row_id,
                anchor_seq,
                &direction.trim().to_lowercase(),
                limit,
                "held_tree",
                true,
            );
        }
        if branch_id != self.branch_id {
            return Err(PactError::invalid(
                "activity_surface branch override is not supported",
            ));
        }
        self.with_tree(|tree| {
            let engine_mode = if normalized_mode == "current_surface" {
                "__pactree_read_handle_current_surface"
            } else {
                mode
            };
            tree.stream(
                engine_mode,
                anchor_row_id,
                anchor_seq,
                direction,
                limit,
                Some(&self.path.display().to_string()),
            )
        })
    }

    pub fn component(&self, component_id: &str) -> PactResult<Value> {
        let requires_composition = self
            .snapshot
            .metadata()?
            .composition_branches
            .contains(&self.branch_id);
        if requires_composition || !self.snapshot.has_parent_child_facet()? {
            return self.with_tree(|tree| tree.project_component(component_id));
        }

        let mut candidates = vec![component_id.to_string()];
        if !component_id.contains(':') {
            candidates.push(format!(
                "{}:{component_id}",
                self.snapshot.metadata()?.root_id
            ));
        }
        let mut resolved = None;
        for candidate in candidates {
            if let Some(value) = self
                .snapshot
                .component_subtree(&self.branch_id, &candidate)?
            {
                resolved = Some((candidate, value));
                break;
            }
        }
        let (component_id, mut value) = resolved
            .ok_or_else(|| PactError::not_found(format!("component {component_id} not found")))?;
        self.hydrate_component_content(&mut value)?;
        if self
            .snapshot
            .metadata()?
            .derived_depth_branches
            .contains(&self.branch_id)
            && let Some(order) = self.owning_turn_order(&component_id)?
        {
            let depth = crate::pact_file::message_stream::derived_depth_of_order(
                &self.snapshot,
                &self.branch_id,
                order,
            )?;
            crate::pact_file::message_stream::set_derived_depth(&mut value, depth);
        }
        Ok(value)
    }

    fn owning_turn_order(&self, component_id: &str) -> PactResult<Option<u64>> {
        let root_id = self.snapshot.metadata()?.root_id;
        let mut current = component_id.to_string();
        for _ in 0..4096 {
            let Some(indexed) = self.snapshot.component_shallow(&self.branch_id, &current)? else {
                return Ok(None);
            };
            match indexed.value.get("parent_id").and_then(Value::as_str) {
                Some(parent) if parent == root_id => return Ok(indexed.order),
                Some(parent) => current = parent.to_string(),
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    pub fn project(
        &self,
        revision: Option<u64>,
    ) -> PactResult<crate::file_format::PactFileProjection> {
        self.with_tree(|tree| {
            if let Some(revision) = revision {
                return tree.project_at_revision(revision);
            }
            let view = match tree.project(crate::ProjectionRequest::View {
                mode: crate::ViewMode::Current,
            })? {
                crate::ProjectionOutput::View(view) => view,
                _ => {
                    return Err(PactError::internal(
                        "read handle projection returned wrong output kind",
                    ));
                }
            };
            Ok(crate::file_format::PactFileProjection {
                schema: "pact.projection.v1".to_string(),
                branch_id: self.branch_id.clone(),
                revision: view.head_revision,
                scope: "head".to_string(),
                filters: crate::file_format::PactFileProjectionFilters {
                    view: None,
                    component_id: None,
                },
                branch: view,
                range: None,
                view_record: None,
                source_branch_id: None,
                projection_branch_id: None,
                projection_revision: None,
            })
        })
    }

    pub fn content_payload(&self, content_id: &str) -> PactResult<Value> {
        let content = self
            .snapshot
            .content(content_id)?
            .ok_or_else(|| PactError::not_found(format!("content {content_id} not found")))?;
        let text = if let Some(inline) = content.storage.inline {
            inline
        } else if let Some(bytes) = content.storage.inline_bytes {
            String::from_utf8(bytes).map_err(|error| {
                PactError::codec(format!("content {content_id} is not UTF-8: {error}"))
            })?
        } else {
            return Err(PactError::not_found(format!(
                "content {content_id} has no projectable chunks"
            )));
        };
        Ok(json!({
            "schema": format!("{}.content_payload.v1", crate::file_format::PACT_FILE_SCHEMA),
            "content_id": content.id,
            "kind": content.kind,
            "hash": content.hash,
            "bytes": text.len(),
            "chars": text.chars().count(),
            "text": text,
        }))
    }

    pub fn traces(&self) -> PactResult<Value> {
        serde_json::to_value(self.snapshot.public_traces(&self.branch_id)?).map_err(|error| {
            PactError::codec(format!("failed to encode transaction traces: {error}"))
        })
    }

    pub fn extensions(
        &self,
        kind: Option<&str>,
        index_key: Option<&str>,
        anchor: Option<&crate::PactExtensionCursor>,
        direction: &str,
        limit: usize,
    ) -> PactResult<crate::PactExtensionPage> {
        self.extensions_at(kind, index_key, anchor, direction, limit, None)
    }

    pub fn extensions_at(
        &self,
        kind: Option<&str>,
        index_key: Option<&str>,
        anchor: Option<&crate::PactExtensionCursor>,
        direction: &str,
        limit: usize,
        at_revision: Option<u64>,
    ) -> PactResult<crate::PactExtensionPage> {
        crate::pact_file::extension::window_at(
            &self.snapshot,
            &self.branch_id,
            crate::pact_file::extension::ExtensionWindow {
                kind,
                index_key,
                anchor,
                direction,
                limit,
                at_revision,
            },
        )
    }

    pub fn extension(&self, record_id: &str) -> PactResult<crate::PactExtensionRow> {
        self.extension_at(record_id, None)
    }

    pub fn extension_at(
        &self,
        record_id: &str,
        at_revision: Option<u64>,
    ) -> PactResult<crate::PactExtensionRow> {
        crate::pact_file::extension::record_at(
            &self.snapshot,
            &self.branch_id,
            record_id,
            at_revision,
        )
    }

    pub fn rewind_targets(&self) -> PactResult<Value> {
        let metadata = self.snapshot.metadata()?;
        let head_revision = self
            .snapshot
            .branch_head(&self.branch_id)?
            .ok_or_else(|| PactError::not_found(format!("branch not found: {}", self.branch_id)))?;
        let message_count = metadata
            .message_counts
            .get(&self.branch_id)
            .copied()
            .unwrap_or_default();
        let entries = self.snapshot.index_range(
            &crate::pact_file::index::IndexKey::MessageOrder {
                branch: self.branch_id.clone(),
                order: 0,
            },
            &crate::pact_file::index::IndexKey::MessageOrder {
                branch: self.branch_id.clone(),
                order: u64::MAX,
            },
            message_count,
            false,
        )?;
        let mut indexed_order = Vec::with_capacity(entries.len());
        let mut roles = std::collections::BTreeMap::new();
        for (_, component_id) in entries {
            let component_id = String::from_utf8(component_id)
                .map_err(|_| PactError::corrupt("Pact message component id is not UTF-8"))?;
            let component = self
                .snapshot
                .component_shallow(&self.branch_id, &component_id)?
                .ok_or_else(|| {
                    PactError::corrupt("Pact message-order index references a missing component")
                })?;
            let role = component
                .value
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string);
            indexed_order.push(component_id.clone());
            roles.insert(component_id, role);
        }
        let mut messages = Vec::with_capacity(indexed_order.len());
        for component_id in self.snapshot.baseline_message_order(&self.branch_id)? {
            if let Some(role) = roles.remove(&component_id) {
                messages.push((component_id, role));
            }
        }
        for component_id in indexed_order {
            if let Some(role) = roles.remove(&component_id) {
                messages.push((component_id, role));
            }
        }
        let traces = self.snapshot.public_traces(&self.branch_id)?;
        Ok(crate::context_tree::build_rewind_targets_value(
            &metadata.root_id,
            &self.branch_id,
            head_revision,
            messages,
            traces.iter(),
        ))
    }

    pub fn refresh(&mut self) -> PactResult<()> {
        self.snapshot = self.snapshot.refresh()?;
        if self.snapshot.branch_head(&self.branch_id)?.is_none() {
            return Err(PactError::not_found(format!(
                "branch not found: {}",
                self.branch_id
            )));
        }
        *self
            .hydrated
            .get_mut()
            .map_err(|_| PactError::internal("Pact read handle cache is poisoned"))? = None;
        Ok(())
    }

    fn with_tree<T>(
        &self,
        operation: impl FnOnce(&crate::ContextTree) -> PactResult<T>,
    ) -> PactResult<T> {
        let mut tree = self.hydrated_tree()?;
        operation(tree.as_mut().ok_or_else(|| {
            PactError::internal("Pact read handle failed to hydrate its held snapshot")
        })?)
    }

    fn hydrate_component_content(&self, value: &mut Value) -> PactResult<()> {
        crate::pact_file::message_stream::hydrate_content(&self.snapshot, value)
    }

    fn hydrated_tree(&self) -> PactResult<MutexGuard<'_, Option<crate::ContextTree>>> {
        let mut tree = self
            .hydrated
            .lock()
            .map_err(|_| PactError::internal("Pact read handle cache is poisoned"))?;
        if tree.is_none() {
            let ledger = self
                .current_shared_ledger()
                .unwrap_or_else(|| self.replay_pinned_ledger())?;
            *tree = Some(crate::ContextTree::from_file_snapshot(
                ledger,
                self.path.clone(),
                &self.branch_id,
            )?);
        }
        Ok(tree)
    }

    fn current_shared_ledger(&self) -> Option<PactResult<Arc<crate::store::ContextTreeLedger>>> {
        let ledger = crate::pact_file::open_shared_ledger(&self.path).ok()?;
        let current = crate::pact_file::snapshot::Snapshot::open(
            &self.path,
            Arc::new(crate::pact_file::backend::IoCounterSet::default()),
        )
        .ok()?;
        self.snapshot
            .shares_mapping_with(&current)
            .then_some(Ok(ledger))
    }

    fn replay_pinned_ledger(&self) -> PactResult<Arc<crate::store::ContextTreeLedger>> {
        #[cfg(test)]
        crate::tests::instrumentation::LEDGER_HYDRATION_COUNT
            .with(|count| count.set(count.get() + 1));
        self.snapshot.replay_ledger().map(Arc::new)
    }
}

fn search_source_component(
    ordinal: u64,
    value: &Value,
) -> PactResult<crate::PactSearchSourceComponent> {
    let component_id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PactError::corrupt("indexed Pact component has no string id"))?;
    let component_type = value
        .get("componentType")
        .or_else(|| value.get("node_type"))
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut fields = [
        ("content", "content_ref", false),
        ("parameters", "parameters_ref", true),
        ("result", "result_ref", true),
        ("error_message", "error_ref", false),
    ]
    .into_iter()
    .filter(|(field, ref_field, _)| {
        value.get(*field).is_some_and(|field| !field.is_null())
            || value.get(*ref_field).and_then(content_ref_id).is_some()
    })
    .map(|(field, _, is_json)| crate::PactSearchSourceField {
        field: field.to_string(),
        is_json,
    })
    .collect::<Vec<_>>();
    fields.sort_by(|left, right| left.field.cmp(&right.field));
    Ok(crate::PactSearchSourceComponent {
        ordinal,
        component_id: component_id.to_string(),
        parent_id: value
            .get("parent_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        component_type: component_type.to_string(),
        role: value
            .get("role")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_name: value
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_call_id: value
            .get("tool_call_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        success: value.get("success").and_then(Value::as_bool),
        cycle: value
            .get("cycle")
            .and_then(Value::as_u64)
            .or_else(|| value.get("pact_cycle").and_then(Value::as_u64))
            .or_else(|| value.get("attrs")?.get("cycle").and_then(Value::as_u64))
            .or_else(|| {
                value
                    .get("attrs")?
                    .get("pact_cycle")
                    .and_then(Value::as_u64)
            }),
        fields,
    })
}

fn encoded_search_source_page_len(
    total_components: u64,
    encoded_component_bytes: usize,
    component_count: usize,
    next_ordinal: Option<u64>,
) -> PactResult<usize> {
    let empty = serde_json::to_vec(&crate::PactSearchSourcePage {
        total_components,
        components: Vec::new(),
        next_ordinal,
    })
    .map_err(|error| PactError::codec(format!("encode Pact search source: {error}")))?
    .len();
    Ok(empty
        .saturating_add(encoded_component_bytes)
        .saturating_add(component_count.saturating_sub(1)))
}

fn search_field_ref_name(field: &str) -> Option<&'static str> {
    match field {
        "content" => Some("content_ref"),
        "parameters" => Some("parameters_ref"),
        "result" => Some("result_ref"),
        "error_message" => Some("error_ref"),
        _ => None,
    }
}

fn content_ref_id(value: &Value) -> Option<&str> {
    if let Some(content_id) = value.as_str() {
        return Some(content_id);
    }
    let object = value.as_object()?;
    ["id", "content_id", "ref_id", "content_ref"]
        .into_iter()
        .find_map(|key| object.get(key).and_then(Value::as_str))
}

fn field_value<'a>(component: &'a Value, field: &str) -> Option<&'a Value> {
    if let Some(key) = field.strip_prefix("aux.") {
        return component.get("aux")?.get(key);
    }
    component.get(field)
}

fn bounded_utf8_bytes(
    bytes: &[u8],
    offset: u64,
    max_bytes: usize,
) -> PactResult<(Vec<u8>, Option<u64>)> {
    if max_bytes == 0 || max_bytes > 16 * 1024 * 1024 {
        return Err(PactError::invalid(
            "Pact search field range must request between 1 byte and 16 MiB",
        ));
    }
    let start = usize::try_from(offset)
        .map_err(|_| PactError::invalid("Pact search field offset exceeds platform limits"))?;
    if start > bytes.len() {
        return Err(PactError::invalid(
            "Pact search field offset exceeds field length",
        ));
    }
    if start < bytes.len() && bytes[start] & 0b1100_0000 == 0b1000_0000 {
        return Err(PactError::invalid(
            "Pact search field offset is not a UTF-8 boundary",
        ));
    }
    let mut end = start.saturating_add(max_bytes).min(bytes.len());
    while end > start && std::str::from_utf8(&bytes[start..end]).is_err() {
        end -= 1;
    }
    let data = bytes[start..end].to_vec();
    Ok((
        data,
        (end < bytes.len()).then_some(
            u64::try_from(end)
                .map_err(|_| PactError::invalid("Pact search field offset exceeds u64"))?,
        ),
    ))
}
