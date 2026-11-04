use serde_json::Value;

use super::btree::CowBTree;
use super::index::{HeadMetadata, IndexKey, indexed_component_from_facet};
use super::layout::{FILE_IDENTITY_BYTES, FILE_IDENTITY_START};
use super::scratch::{ScratchPages, ScratchRange};
use super::snapshot::Snapshot;
use crate::components::Component;
use crate::error::{PactError, PactResult};
use crate::memory::PactMemoryOptions;
use crate::store::PactRef;

#[cfg(test)]
mod tests;

const SCAN_PAGE_ENTRIES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct NodeRow {
    pub(crate) parent: String,

    pub(crate) depth: Option<i64>,
    pub(crate) offset: i64,
    pub(crate) kind: String,

    pub(crate) turn: String,

    pub(crate) epoch: u64,

    pub(crate) subtree_len: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Admission {
    pub(crate) first_seen_added: Vec<String>,

    pub(crate) lifecycle_added: Vec<String>,
}

pub(crate) struct AppendState {
    pub(crate) branch_id: String,
    pub(crate) root_id: String,
    pub(crate) file_identity: String,
    pub(crate) generation: u64,
    pub(crate) committed_len: u64,
    pub(crate) index_root: u64,
    pub(crate) head_revision: u64,
    pub(crate) lifecycle_tick: u64,
    pub(crate) base_ref: PactRef,

    pub(crate) derived_depth: bool,
    pub(crate) root_child_count: u64,
    pub(crate) next_offset: i64,
    pub(crate) node_count: u64,
    pub(crate) first_seen_count: u64,
    pub(crate) lifecycle_seen_count: u64,

    pub(crate) first_seen_missing: u64,

    pub(crate) lifecycle_missing: u64,

    pub(crate) lifecycle_stale: u64,

    pub(crate) epoch: u64,

    pub(crate) dangling_relations: Vec<(String, String)>,
    pages: ScratchPages,
    nodes: CowBTree,
    turns: CowBTree,
    turn_nodes: CowBTree,
    first_seen: CowBTree,
    lifecycle_seen: CowBTree,
}

pub(crate) fn persisted_identity_present(file: &[u8]) -> bool {
    file.get(FILE_IDENTITY_START..FILE_IDENTITY_START + FILE_IDENTITY_BYTES)
        .is_some_and(|identity| identity.iter().any(|byte| *byte != 0))
}

pub(crate) fn check_metadata_eligibility(
    metadata: &HeadMetadata,
    branch_id: &str,
) -> PactResult<()> {
    if metadata.branch_count != 1 {
        return Err(PactError::unsupported(format!(
            "bounded append supports exactly one branch; file has {}",
            metadata.branch_count
        )));
    }
    if metadata.current_branch_id != branch_id {
        return Err(PactError::unsupported(format!(
            "bounded append targets the current branch {}, not {branch_id}",
            metadata.current_branch_id
        )));
    }
    if !metadata.composition_branches.is_empty() {
        return Err(PactError::unsupported(
            "bounded append does not support composed branches",
        ));
    }
    Ok(())
}

pub(crate) fn branch_scalars(branch: &Value) -> PactResult<(u64, u64, PactRef)> {
    if branch
        .get("parent_branch_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(PactError::unsupported(
            "bounded append does not support a branch with a parent composition",
        ));
    }
    if branch.get("fork_ref").is_some_and(|value| !value.is_null()) {
        return Err(PactError::unsupported(
            "bounded append does not support a forked branch",
        ));
    }
    let head_revision = branch
        .get("head_revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| PactError::corrupt("Pact baseline branch has no head_revision"))?;
    let lifecycle_tick = branch
        .get("lifecycle_tick")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let base_ref: PactRef = serde_json::from_value(
        branch
            .get("base_ref")
            .cloned()
            .ok_or_else(|| PactError::corrupt("Pact baseline branch has no base_ref"))?,
    )
    .map_err(|error| PactError::corrupt(format!("invalid Pact baseline base_ref: {error}")))?;
    Ok((head_revision, lifecycle_tick, base_ref))
}

fn has_lifecycle_policy(value: &Value) -> bool {
    let policy = |field: &Value| !field.is_null();
    if value.get("ttl").is_some_and(policy) || value.get("cad").is_some_and(policy) {
        return true;
    }
    value
        .get("lifecycle")
        .and_then(Value::as_object)
        .is_some_and(|lifecycle| {
            lifecycle.get("ttl").is_some_and(policy) || lifecycle.get("cad").is_some_and(policy)
        })
}

pub(crate) fn classify_component(id: &str, value: &Value, has_children: bool) -> PactResult<()> {
    if has_lifecycle_policy(value) {
        return Err(PactError::unsupported(format!(
            "bounded append does not support lifecycle policies (component {id})"
        )));
    }
    let kind = component_kind(value);
    let has_content = value
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty());
    let has_content_ref = value
        .get("content_ref")
        .is_some_and(|value| !value.is_null());
    if crate::resolver::is_hollow_context_container(
        kind,
        has_children,
        has_content,
        has_content_ref,
    ) {
        return Err(PactError::unsupported(format!(
            "bounded append does not support a hollow message turn (component {id})"
        )));
    }
    Ok(())
}

pub(crate) fn classify_incoming(component: &Component) -> PactResult<()> {
    if component.lifecycle.policy.ttl.is_some() || component.lifecycle.policy.cad.is_some() {
        return Err(PactError::unsupported(format!(
            "bounded append does not support lifecycle policies (component {})",
            component.id
        )));
    }
    if crate::resolver::is_hollow_context_container(
        component.component_type(),
        !component.children.is_empty(),
        component.content().is_some_and(|text| !text.is_empty()),
        component.content_ref().is_some(),
    ) {
        return Err(PactError::unsupported(format!(
            "bounded append does not support a hollow message turn (component {})",
            component.id
        )));
    }
    for child in &component.children {
        classify_incoming(child)?;
    }
    Ok(())
}

fn component_kind(value: &Value) -> &str {
    value
        .get("componentType")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn depth_of(value: &Value) -> Option<i64> {
    value
        .get("pact_depth")
        .or_else(|| value.get("depth"))
        .and_then(Value::as_i64)
}

fn offset_of(value: &Value) -> i64 {
    value.get("offset").and_then(Value::as_i64).unwrap_or(0)
}

fn turn_node_key(turn: &str, ordinal: u64) -> Vec<u8> {
    let mut key = turn.as_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(&ordinal.to_be_bytes());
    key
}

pub(crate) fn is_shiftable(row: &NodeRow) -> bool {
    crate::mutation::is_turn_container_kind(&row.kind) && row.depth.is_some_and(|depth| depth >= 0)
}

impl AppendState {
    pub(crate) fn bootstrap(
        snapshot: &Snapshot,
        branch_id: &str,
        options: &PactMemoryOptions,
    ) -> PactResult<Self> {
        options.validate()?;
        let metadata = snapshot.metadata()?;
        check_metadata_eligibility(&metadata, branch_id)?;
        if !persisted_identity_present(snapshot.mapping()) {
            return Err(PactError::unsupported(
                "bounded append requires a file with a persisted logical identity",
            ));
        }
        let file_identity = snapshot.source_file_identity();
        let head_revision = metadata
            .head_revisions
            .get(branch_id)
            .copied()
            .ok_or_else(|| PactError::corrupt("Pact head metadata is missing the branch head"))?;
        let index_root = snapshot.index_root();
        let mut state = Self {
            branch_id: branch_id.to_string(),
            root_id: metadata.root_id.clone(),
            file_identity,
            generation: snapshot.generation(),
            committed_len: snapshot.committed_len(),
            index_root,
            head_revision,
            lifecycle_tick: 0,
            base_ref: PactRef {
                root_id: metadata.root_id.clone(),
                branch_id: branch_id.to_string(),
                revision: None,
                transaction_id: None,
                component_id: None,
                content_ref: None,
            },
            derived_depth: false,
            root_child_count: 0,
            next_offset: 0,
            node_count: 0,
            first_seen_count: 0,
            lifecycle_seen_count: 0,
            first_seen_missing: 0,
            lifecycle_missing: 0,
            lifecycle_stale: 0,
            epoch: 0,
            dangling_relations: Vec::new(),
            pages: ScratchPages::create(None, options.io_chunk_bytes, options.scratch_cache_bytes)?,
            nodes: CowBTree::from_root(None),
            turns: CowBTree::from_root(None),
            turn_nodes: CowBTree::from_root(None),
            first_seen: CowBTree::from_root(None),
            lifecycle_seen: CowBTree::from_root(None),
        };
        let later = state.load_branch_history(snapshot)?;
        state.load_topology(snapshot)?;

        let scan_traces =
            !(state.derived_depth && metadata.lifecycle_policy_free_branches.contains(branch_id));
        if scan_traces {
            state.replay_positional_history(snapshot, &later, options)?;
        }
        if state.derived_depth {
            state.derive_root_depths()?;
        }
        state.reconcile_membership()?;
        Ok(state)
    }

    fn load_baseline_membership(&mut self, snapshot: &Snapshot) -> PactResult<()> {
        let first = snapshot.index_range(
            &IndexKey::Generation { generation: 0 },
            &IndexKey::Generation {
                generation: u64::MAX,
            },
            1,
            false,
        )?;
        let address = first
            .first()
            .map(|(_, value)| super::codec::decode_u64_value(value, "Pact transaction address"))
            .transpose()?
            .ok_or_else(|| PactError::corrupt("Pact file has no baseline transaction"))?;
        let streamed =
            super::stream_decode::baseline_checkpoint_reader(snapshot.mapping(), address)?;
        let Some((reader, in_sections)) = streamed else {
            return self.load_baseline_membership_decoded(snapshot);
        };
        let branch_id = self.branch_id.clone();
        let scalars = super::checkpoint_stream::stream_branch_membership(
            reader,
            in_sections,
            &branch_id,
            self,
        )?;
        if scalars.has_parent_branch {
            return Err(PactError::unsupported(
                "bounded append does not support a branch with a parent composition",
            ));
        }
        if scalars.has_fork_ref {
            return Err(PactError::unsupported(
                "bounded append does not support a forked branch",
            ));
        }
        scalars
            .head_revision
            .ok_or_else(|| PactError::corrupt("Pact baseline branch has no head_revision"))?;
        self.lifecycle_tick = scalars.lifecycle_tick;
        self.derived_depth = scalars.derived_depth;
        self.base_ref = scalars
            .base_ref
            .ok_or_else(|| PactError::corrupt("Pact baseline branch has no base_ref"))?;
        Ok(())
    }

    fn load_baseline_membership_decoded(&mut self, snapshot: &Snapshot) -> PactResult<()> {
        let mut stream = super::transaction_stream::TransactionStream::new(snapshot);
        let (baseline, _) = stream
            .next()
            .ok_or_else(|| PactError::corrupt("Pact file has no baseline transaction"))??;
        let checkpoint = baseline
            .head_checkpoint
            .ok_or_else(|| PactError::corrupt("Pact baseline transaction has no checkpoint"))?;
        let ledger = checkpoint.get("ledger").unwrap_or(&checkpoint);
        let branch = ledger
            .get("branches")
            .and_then(|branches| branches.get(&self.branch_id))
            .ok_or_else(|| PactError::corrupt("Pact baseline has no entry for the branch"))?;
        let (_, lifecycle_tick, base_ref) = branch_scalars(branch)?;
        self.lifecycle_tick = lifecycle_tick;
        self.base_ref = base_ref;
        self.derived_depth = branch
            .get("aux")
            .and_then(|aux| aux.get(crate::store::PACT_DERIVED_DEPTH_AUX_KEY))
            .and_then(Value::as_bool)
            == Some(true);
        if let Some(first_seen) = branch
            .pointer("/current/tree/first_seen")
            .and_then(Value::as_object)
        {
            for (id, revision) in first_seen {
                self.insert_first_seen(id, revision.as_u64().unwrap_or_default())?;
            }
        }
        if let Some(seen) = branch
            .pointer("/current/lifecycle_seen")
            .and_then(Value::as_object)
        {
            for (id, tick) in seen {
                self.set_lifecycle_seen(id, tick.as_u64().unwrap_or_default())?;
            }
        }
        Ok(())
    }

    fn load_branch_history(&mut self, snapshot: &Snapshot) -> PactResult<Vec<u64>> {
        let mut later = Vec::new();
        self.load_baseline_membership(snapshot)?;

        let mut next_generation = 1_u64;
        let mut first = true;
        loop {
            let entries = snapshot.index_range(
                &IndexKey::Generation {
                    generation: next_generation,
                },
                &IndexKey::Generation {
                    generation: u64::MAX,
                },
                super::transaction_stream::GENERATION_PAGE_ENTRIES,
                false,
            )?;
            if entries.is_empty() {
                break;
            }
            let mut last_generation = next_generation;
            for (key, value) in &entries {
                let IndexKey::Generation { generation } = IndexKey::decode(key)? else {
                    return Err(PactError::corrupt(
                        "Pact generation range returned another namespace",
                    ));
                };
                last_generation = generation;
                if first {
                    first = false;
                    continue;
                }
                let raw: [u8; 8] = value.as_slice().try_into().map_err(|_| {
                    PactError::corrupt("Pact transaction address has invalid width")
                })?;
                let address = u64::from_le_bytes(raw);
                later.push(address);
                if let Some(delta) =
                    super::stream_decode::transaction_ledger_delta(snapshot.mapping(), address)?
                {
                    self.apply_ledger_delta(&delta)?;
                }
            }
            if entries.len() < super::transaction_stream::GENERATION_PAGE_ENTRIES {
                break;
            }
            next_generation = last_generation
                .checked_add(1)
                .ok_or_else(|| PactError::corrupt("Pact generation overflow"))?;
        }
        Ok(later)
    }

    fn replay_positional_history(
        &mut self,
        snapshot: &Snapshot,
        later: &[u64],
        options: &PactMemoryOptions,
    ) -> PactResult<()> {
        if later.is_empty() {
            return Ok(());
        }
        let root_children = self
            .root_child_ids()?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let mut depths = std::collections::BTreeMap::<String, i64>::new();
        let well_known = super::stream_decode::WELL_KNOWN_SYMBOLS;
        let depth_tag = well_known
            .iter()
            .position(|tag| *tag == "pact_depth")
            .map(|index| index as u64);
        for address in later {
            #[cfg(test)]
            crate::tests::instrumentation::POSITIONAL_REPLAY_TRANSACTIONS
                .with(|count| count.set(count.get() + 1));
            let bytes =
                super::stream_decode::transaction_trace_bytes(snapshot.mapping(), *address)?;
            let mut pages =
                ScratchPages::create(None, options.io_chunk_bytes, options.scratch_cache_bytes)?;
            let mut stream = match super::stream_decode::PttrStream::open(&bytes, &mut pages) {
                Ok(stream) => stream,
                Err(_) => {
                    let trace = crate::transaction_trace::decode_transaction_trace(&bytes)?;
                    for op in &trace.ops {
                        self.observe_positional_op(op, &root_children, &mut depths)?;
                    }
                    continue;
                }
            };

            let mut root_symbols = std::collections::BTreeSet::<u64>::new();
            for ordinal in well_known.len() as u64..stream.symbol_count() {
                if root_children.contains(&stream.symbol_text(&pages, ordinal)?) {
                    root_symbols.insert(ordinal);
                }
            }
            while let Some(op) = stream.next_op()? {
                match op.kind {
                    super::stream_decode::StreamOpKind::SetField { key, value } => {
                        let is_depth = match depth_tag {
                            Some(tag) => key == tag,
                            None => stream.symbol_text(&pages, key)? == "pact_depth",
                        };
                        if !is_depth || !root_symbols.contains(&op.component) {
                            continue;
                        }
                        let id = stream.symbol_text(&pages, op.component)?;
                        let text = stream.symbol_text(&pages, value)?;
                        let depth: Value = serde_json::from_str(&text).map_err(|error| {
                            PactError::corrupt(format!("invalid Pact depth value: {error}"))
                        })?;
                        if let Some(depth) = depth.as_i64() {
                            depths.insert(id, depth);
                        }
                    }
                    super::stream_decode::StreamOpKind::SetLifecycle { value } => {
                        let id = stream.symbol_text(&pages, op.component)?;
                        if !self.is_current_id(&id)? {
                            continue;
                        }
                        let text = stream.symbol_text(&pages, value)?;
                        let lifecycle: Value = serde_json::from_str(&text).map_err(|error| {
                            PactError::corrupt(format!("invalid Pact lifecycle value: {error}"))
                        })?;
                        if has_lifecycle_policy(&lifecycle) {
                            return Err(PactError::unsupported(format!(
                                "bounded append does not support lifecycle policies (component {id})"
                            )));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (id, depth) in depths {
            if let Some(mut row) = self.node(&id)? {
                row.depth = Some(depth);
                self.write_node(&id, &row)?;
            }
        }
        Ok(())
    }

    fn derive_root_depths(&mut self) -> PactResult<()> {
        let mut shiftable = Vec::new();
        let mut turns = self.turn_pages(SCAN_PAGE_ENTRIES);
        loop {
            let page = turns.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (_, value) in page {
                let id = String::from_utf8(value)
                    .map_err(|_| PactError::corrupt("Pact turn id is not UTF-8"))?;
                if let Some(row) = self.node(&id)?
                    && is_shiftable(&row)
                {
                    shiftable.push((id, row));
                }
            }
        }
        let count = shiftable.len();
        for (index, (id, mut row)) in shiftable.into_iter().enumerate() {
            let rank = (count - 1 - index) as i64;
            row.depth = Some(rank - self.epoch.saturating_sub(row.epoch) as i64);
            self.write_node(&id, &row)?;
        }
        Ok(())
    }

    fn observe_positional_op(
        &self,
        op: &crate::transaction_trace::TraceOp,
        root_children: &std::collections::BTreeSet<String>,
        depths: &mut std::collections::BTreeMap<String, i64>,
    ) -> PactResult<()> {
        match &op.kind {
            crate::transaction_trace::TraceOpKind::SetField { key, value }
                if key == "pact_depth" && root_children.contains(&op.component_id) =>
            {
                if let Some(depth) = value.as_i64() {
                    depths.insert(op.component_id.clone(), depth);
                }
            }
            crate::transaction_trace::TraceOpKind::SetLifecycle { value }
                if self.is_current_id(&op.component_id)? && has_lifecycle_policy(value) =>
            {
                return Err(PactError::unsupported(format!(
                    "bounded append does not support lifecycle policies (component {})",
                    op.component_id
                )));
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_ledger_delta(&mut self, delta: &Value) -> PactResult<()> {
        if let Some(tick) = delta.get("lifecycle_tick").and_then(Value::as_u64) {
            self.lifecycle_tick = tick;
        }
        if delta.get("derived_depth").and_then(Value::as_bool) == Some(true) {
            self.derived_depth = true;
        }
        if let Some(base_ref) = delta.get("base_ref") {
            self.base_ref = serde_json::from_value(base_ref.clone()).map_err(|error| {
                PactError::corrupt(format!("invalid Pact ledger delta base_ref: {error}"))
            })?;
        }
        if let Some(additions) = delta.get("first_seen_additions").and_then(Value::as_object) {
            for (id, revision) in additions {
                self.insert_first_seen(id, revision.as_u64().unwrap_or_default())?;
            }
        }
        if let Some(additions) = delta
            .get("lifecycle_seen_additions")
            .and_then(Value::as_object)
        {
            for (id, tick) in additions {
                self.set_lifecycle_seen(id, tick.as_u64().unwrap_or_default())?;
            }
        }
        if let Some(removals) = delta
            .get("lifecycle_seen_removals")
            .and_then(Value::as_array)
        {
            for id in removals {
                let id = id.as_str().ok_or_else(|| {
                    PactError::corrupt("Pact ledger delta lifecycle removal is not a string")
                })?;
                self.remove_lifecycle_seen(id)?;
            }
        }
        Ok(())
    }

    fn set_lifecycle_seen(&mut self, id: &str, tick: u64) -> PactResult<bool> {
        let fresh = self
            .lifecycle_seen
            .get(&self.pages, id.as_bytes())?
            .is_none();
        let (tree, _) = std::mem::replace(&mut self.lifecycle_seen, CowBTree::from_root(None))
            .insert(
                &mut self.pages,
                id.as_bytes().to_vec(),
                tick.to_le_bytes().to_vec(),
            )?;
        self.lifecycle_seen = tree;
        if fresh {
            self.lifecycle_seen_count += 1;
        }
        Ok(fresh)
    }

    fn remove_lifecycle_seen(&mut self, id: &str) -> PactResult<bool> {
        let (tree, _, removed) =
            std::mem::replace(&mut self.lifecycle_seen, CowBTree::from_root(None))
                .delete(&mut self.pages, id.as_bytes())?;
        self.lifecycle_seen = tree;
        if removed.is_some() {
            self.lifecycle_seen_count -= 1;
        }
        Ok(removed.is_some())
    }

    pub(crate) fn lifecycle_seen(&self, id: &str) -> PactResult<Option<u64>> {
        let Some(bytes) = self.lifecycle_seen.get(&self.pages, id.as_bytes())? else {
            return Ok(None);
        };
        let raw: [u8; 8] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| PactError::corrupt("Pact lifecycle tick has invalid width"))?;
        Ok(Some(u64::from_le_bytes(raw)))
    }

    pub(crate) fn lifecycle_seen_pages(&self, page_entries: usize) -> ScratchRange {
        ScratchRange::new(&self.lifecycle_seen, &[], &[0xff; 256], page_entries)
    }

    pub(crate) fn first_seen(&self, id: &str) -> PactResult<Option<u64>> {
        let Some(bytes) = self.first_seen.get(&self.pages, id.as_bytes())? else {
            return Ok(None);
        };
        let raw: [u8; 8] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| PactError::corrupt("Pact first-seen revision has invalid width"))?;
        Ok(Some(u64::from_le_bytes(raw)))
    }

    fn insert_first_seen(&mut self, id: &str, revision: u64) -> PactResult<bool> {
        let fresh = self.first_seen.get(&self.pages, id.as_bytes())?.is_none();
        let (tree, _) = std::mem::replace(&mut self.first_seen, CowBTree::from_root(None)).insert(
            &mut self.pages,
            id.as_bytes().to_vec(),
            revision.to_le_bytes().to_vec(),
        )?;
        self.first_seen = tree;
        if fresh {
            self.first_seen_count += 1;
        }
        Ok(fresh)
    }

    fn read_component(&self, snapshot: &Snapshot, id: &str) -> PactResult<Option<Value>> {
        let key = IndexKey::Component {
            branch: self.branch_id.clone(),
            component: id.to_string(),
        };
        let Some(facet) =
            super::index::read_index_value_from_pages(snapshot, self.index_root, &key)?
        else {
            return Ok(None);
        };
        let tree = CowBTree::from_root(Some(self.index_root));
        let (indexed, _) = indexed_component_from_facet(&tree, snapshot, &facet)?;
        Ok(Some(indexed.value))
    }

    fn children_of(&self, snapshot: &Snapshot, parent: &str) -> PactResult<Vec<String>> {
        super::index::parent_children_from_pages(snapshot, self.index_root, &self.branch_id, parent)
    }

    fn write_node(&mut self, id: &str, row: &NodeRow) -> PactResult<bool> {
        let fresh = self.nodes.get(&self.pages, id.as_bytes())?.is_none();
        let encoded = serde_json::to_vec(row)
            .map_err(|error| PactError::codec(format!("encode Pact node row: {error}")))?;
        let (nodes, _) = std::mem::replace(&mut self.nodes, CowBTree::from_root(None)).insert(
            &mut self.pages,
            id.as_bytes().to_vec(),
            encoded,
        )?;
        self.nodes = nodes;
        if fresh {
            self.node_count += 1;
        }
        Ok(fresh)
    }

    fn write_turn_node(&mut self, turn: &str, ordinal: u64, id: &str) -> PactResult<()> {
        let (turn_nodes, _) = std::mem::replace(&mut self.turn_nodes, CowBTree::from_root(None))
            .insert(
                &mut self.pages,
                turn_node_key(turn, ordinal),
                id.as_bytes().to_vec(),
            )?;
        self.turn_nodes = turn_nodes;
        Ok(())
    }

    fn write_turn(&mut self, ordinal: u64, id: &str) -> PactResult<()> {
        let (turns, _) = std::mem::replace(&mut self.turns, CowBTree::from_root(None)).insert(
            &mut self.pages,
            ordinal.to_be_bytes().to_vec(),
            id.as_bytes().to_vec(),
        )?;
        self.turns = turns;
        Ok(())
    }

    fn load_topology(&mut self, snapshot: &Snapshot) -> PactResult<()> {
        let root_id = self.root_id.clone();
        let root_children = self.children_of(snapshot, &root_id)?;
        let mut max_offset: Option<i64> = None;
        let mut ordinal = 0_u64;
        for child_id in &root_children {
            let Some(value) = self.read_component(snapshot, child_id)? else {
                self.dangling_relations
                    .push((root_id.clone(), child_id.clone()));
                continue;
            };
            let grandchildren = self.children_of(snapshot, child_id)?;
            classify_component(child_id, &value, !grandchildren.is_empty())?;
            let offset = offset_of(&value);
            if offset >= 0 {
                max_offset = Some(max_offset.map_or(offset, |current| current.max(offset)));
            }
            let mut subtree_len = 1_u64;
            let mut stack = grandchildren
                .into_iter()
                .rev()
                .map(|id| (child_id.clone(), id))
                .collect::<Vec<_>>();
            self.write_turn_node(child_id, 0, child_id)?;
            while let Some((parent, id)) = stack.pop() {
                let Some(value) = self.read_component(snapshot, &id)? else {
                    self.dangling_relations.push((parent.clone(), id.clone()));
                    continue;
                };
                let children = self.children_of(snapshot, &id)?;
                classify_component(&id, &value, !children.is_empty())?;
                let row = NodeRow {
                    parent: parent.clone(),
                    depth: depth_of(&value),
                    offset: offset_of(&value),
                    kind: component_kind(&value).to_string(),
                    turn: child_id.clone(),
                    epoch: 0,
                    subtree_len: 0,
                };
                self.write_node(&id, &row)?;
                self.write_turn_node(child_id, subtree_len, &id)?;
                subtree_len += 1;
                stack.extend(children.into_iter().rev().map(|next| (id.clone(), next)));
            }
            let row = NodeRow {
                parent: root_id.clone(),
                depth: depth_of(&value),
                offset,
                kind: component_kind(&value).to_string(),
                turn: child_id.clone(),
                epoch: 0,
                subtree_len,
            };
            self.write_node(child_id, &row)?;
            self.write_turn(ordinal, child_id)?;
            ordinal += 1;
        }
        self.root_child_count = ordinal;
        self.next_offset = max_offset.map_or(0, |offset| offset.saturating_add(1));
        Ok(())
    }

    fn reconcile_membership(&mut self) -> PactResult<()> {
        let root_id = self.root_id.clone();
        let mut first_seen_missing = 0;
        let mut lifecycle_missing = 0;
        if self.first_seen(&root_id)?.is_none() {
            first_seen_missing += 1;
        }
        if self.lifecycle_seen(&root_id)?.is_none() {
            lifecycle_missing += 1;
        }
        let mut nodes = ScratchRange::new(&self.nodes, &[], &[0xff; 256], SCAN_PAGE_ENTRIES);
        loop {
            let page = nodes.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (key, _) in page {
                if self.first_seen.get(&self.pages, &key)?.is_none() {
                    first_seen_missing += 1;
                }
                if self.lifecycle_seen.get(&self.pages, &key)?.is_none() {
                    lifecycle_missing += 1;
                }
            }
        }
        let mut stale = 0;
        let mut seen = self.lifecycle_seen_pages(SCAN_PAGE_ENTRIES);
        loop {
            let page = seen.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (key, _) in page {
                if key != root_id.as_bytes() && self.nodes.get(&self.pages, &key)?.is_none() {
                    stale += 1;
                }
            }
        }
        self.first_seen_missing = first_seen_missing;
        self.lifecycle_missing = lifecycle_missing;
        self.lifecycle_stale = stale;
        Ok(())
    }

    pub(crate) fn verify_pinned(&self, snapshot: &Snapshot) -> PactResult<()> {
        if snapshot.source_file_identity() != self.file_identity
            || snapshot.generation() != self.generation
            || snapshot.committed_len() != self.committed_len
        {
            return Err(PactError::invalid(format!(
                "Pact append state is stale: pinned generation {} watermark {}, file is at generation {} watermark {}",
                self.generation,
                self.committed_len,
                snapshot.generation(),
                snapshot.committed_len()
            )));
        }
        Ok(())
    }

    pub(crate) fn node(&self, id: &str) -> PactResult<Option<NodeRow>> {
        let Some(bytes) = self.nodes.get(&self.pages, id.as_bytes())? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| PactError::corrupt(format!("invalid Pact node row: {error}")))
    }

    pub(crate) fn is_current_id(&self, id: &str) -> PactResult<bool> {
        Ok(id == self.root_id || self.nodes.get(&self.pages, id.as_bytes())?.is_some())
    }

    pub(crate) fn is_known_id(&self, id: &str) -> PactResult<bool> {
        Ok(self.is_current_id(id)? || self.first_seen.get(&self.pages, id.as_bytes())?.is_some())
    }

    pub(crate) fn current_depth(&self, row: &NodeRow) -> PactResult<Option<i64>> {
        let turn = if row.parent == self.root_id {
            row.clone()
        } else {
            self.node(&row.turn)?.ok_or_else(|| {
                PactError::corrupt(format!("Pact turn {} is missing from the state", row.turn))
            })?
        };
        if !is_shiftable(&turn) {
            return Ok(row.depth);
        }

        let turn_depth = turn.depth.unwrap_or_default();
        let turn_shifts = self.epoch.saturating_sub(turn.epoch) as i64;
        Ok(Some(turn_depth + turn_shifts))
    }

    pub(crate) fn turn_pages(&self, page_entries: usize) -> ScratchRange {
        ScratchRange::new(&self.turns, &[], &[0xff; 8], page_entries)
    }

    pub(crate) fn turn_node_pages(&self, turn_id: &str, page_entries: usize) -> ScratchRange {
        let mut end = turn_id.as_bytes().to_vec();
        end.push(1);
        ScratchRange::new(
            &self.turn_nodes,
            &turn_node_key(turn_id, 0),
            &end,
            page_entries,
        )
    }

    pub(crate) fn direct_children(
        &self,
        turn_id: &str,
        parent: &str,
    ) -> PactResult<(Vec<String>, Option<i64>)> {
        let mut children = Vec::new();
        let mut max_offset = None;
        let mut range = self.turn_node_pages(turn_id, SCAN_PAGE_ENTRIES);
        loop {
            let page = range.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (_, value) in page {
                let id = String::from_utf8(value)
                    .map_err(|_| PactError::corrupt("Pact node id is not UTF-8"))?;
                if id == parent {
                    continue;
                }
                let row = self.node(&id)?.ok_or_else(|| {
                    PactError::corrupt(format!("Pact node {id} is missing from the state"))
                })?;
                if row.parent == parent {
                    if row.offset >= 0 {
                        max_offset = Some(
                            max_offset.map_or(row.offset, |current: i64| current.max(row.offset)),
                        );
                    }
                    children.push(id);
                }
            }
        }
        Ok((children, max_offset))
    }

    pub(crate) fn admit_root_turn(
        &mut self,
        component: &Component,
        revision: u64,
    ) -> PactResult<Admission> {
        self.epoch += 1;
        let root_id = self.root_id.clone();
        let ordinal = self.root_child_count;
        let mut admission = Admission::default();
        let mut subtree_len = 0;
        let mut stack = vec![(root_id.clone(), component)];
        while let Some((parent, node)) = stack.pop() {
            let row = NodeRow {
                parent,
                depth: node.pact_depth.map(i64::from),
                offset: i64::from(node.offset),
                kind: node.component_type().to_string(),
                turn: component.id.clone(),
                epoch: self.epoch,
                subtree_len: 0,
            };
            if !self.write_node(&node.id, &row)? {
                return Err(PactError::invalid(format!(
                    "Pact component id {} is already present",
                    node.id
                )));
            }
            self.write_turn_node(&component.id, subtree_len, &node.id)?;
            subtree_len += 1;
            self.admit_membership(&node.id, revision, &mut admission)?;
            stack.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|child| (node.id.clone(), child)),
            );
        }
        let mut turn_row = self
            .node(&component.id)?
            .ok_or_else(|| PactError::corrupt("Pact turn row disappeared during admission"))?;
        turn_row.subtree_len = subtree_len;
        self.write_node(&component.id, &turn_row)?;
        self.write_turn(ordinal, &component.id)?;
        self.root_child_count += 1;
        if i64::from(component.offset) >= self.next_offset {
            self.next_offset = i64::from(component.offset).saturating_add(1);
        }
        Ok(admission)
    }

    pub(crate) fn admit_child(
        &mut self,
        turn_id: &str,
        component: &Component,
        revision: u64,
    ) -> PactResult<Admission> {
        let mut turn_row = self
            .node(turn_id)?
            .ok_or_else(|| PactError::invalid(format!("insert parent `{turn_id}` not found")))?;
        let parent = component
            .parent_id
            .clone()
            .ok_or_else(|| PactError::invalid("Pact child insert has no parent"))?;
        let mut admission = Admission::default();
        let mut stack = vec![(parent, component)];
        while let Some((parent, node)) = stack.pop() {
            let row = NodeRow {
                parent,
                depth: node.pact_depth.map(i64::from),
                offset: i64::from(node.offset),
                kind: node.component_type().to_string(),
                turn: turn_id.to_string(),
                epoch: self.epoch,
                subtree_len: 0,
            };
            if !self.write_node(&node.id, &row)? {
                return Err(PactError::invalid(format!(
                    "Pact component id {} is already present",
                    node.id
                )));
            }
            self.write_turn_node(turn_id, turn_row.subtree_len, &node.id)?;
            turn_row.subtree_len += 1;
            self.admit_membership(&node.id, revision, &mut admission)?;
            stack.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|child| (node.id.clone(), child)),
            );
        }
        self.write_node(turn_id, &turn_row)?;
        Ok(admission)
    }

    fn admit_membership(
        &mut self,
        id: &str,
        revision: u64,
        admission: &mut Admission,
    ) -> PactResult<()> {
        if self.insert_first_seen(id, revision)? {
            admission.first_seen_added.push(id.to_string());
        }
        if self.set_lifecycle_seen(id, self.lifecycle_tick + 1)? {
            admission.lifecycle_added.push(id.to_string());
        }
        Ok(())
    }

    pub(crate) fn repair_membership(&mut self, revision: u64) -> PactResult<Admission> {
        let mut admission = Admission::default();
        if self.first_seen_missing == 0 && self.lifecycle_missing == 0 {
            return Ok(admission);
        }
        let root_id = self.root_id.clone();
        let mut ids = vec![root_id.into_bytes()];
        let mut nodes = ScratchRange::new(&self.nodes, &[], &[0xff; 256], SCAN_PAGE_ENTRIES);
        loop {
            let page = nodes.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            ids.extend(page.into_iter().map(|(key, _)| key));
        }
        for key in ids {
            let id = String::from_utf8(key)
                .map_err(|_| PactError::corrupt("Pact node id is not UTF-8"))?;
            self.admit_membership(&id, revision, &mut admission)?;
        }
        self.first_seen_missing = 0;
        self.lifecycle_missing = 0;
        Ok(admission)
    }

    pub(crate) fn stale_lifecycle_ids(&mut self) -> PactResult<Vec<String>> {
        if self.lifecycle_stale == 0 {
            return Ok(Vec::new());
        }
        let mut stale = Vec::new();
        let mut seen = self.lifecycle_seen_pages(SCAN_PAGE_ENTRIES);
        loop {
            let page = seen.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (key, _) in page {
                let id = String::from_utf8(key)
                    .map_err(|_| PactError::corrupt("Pact lifecycle id is not UTF-8"))?;
                if !self.is_current_id(&id)? {
                    stale.push(id);
                }
            }
        }
        for id in &stale {
            self.remove_lifecycle_seen(id)?;
        }
        self.lifecycle_stale = 0;
        Ok(stale)
    }

    pub(crate) fn advance(
        &mut self,
        revision: u64,
        generation: u64,
        committed_len: u64,
        index_root: u64,
    ) {
        self.head_revision = revision;
        self.generation = generation;
        self.committed_len = committed_len;
        self.index_root = index_root;
    }

    pub(crate) fn root_child_ids(&self) -> PactResult<Vec<String>> {
        let mut ids = Vec::with_capacity(self.root_child_count as usize);
        let mut turns = self.turn_pages(SCAN_PAGE_ENTRIES);
        loop {
            let page = turns.next_page(&self.pages)?;
            if page.is_empty() {
                break;
            }
            for (_, value) in page {
                ids.push(
                    String::from_utf8(value)
                        .map_err(|_| PactError::corrupt("Pact turn id is not UTF-8"))?,
                );
            }
        }
        Ok(ids)
    }

    pub(crate) fn pages(&self) -> &ScratchPages {
        &self.pages
    }

    pub(crate) fn scratch_bytes(&self) -> u64 {
        self.pages.scratch_bytes()
    }
}

impl super::checkpoint_stream::MembershipSink for AppendState {
    fn first_seen(&mut self, component_id: &str, revision: u64) -> PactResult<()> {
        self.insert_first_seen(component_id, revision).map(|_| ())
    }

    fn lifecycle_seen(&mut self, component_id: &str, tick: u64) -> PactResult<()> {
        self.set_lifecycle_seen(component_id, tick).map(|_| ())
    }
}
