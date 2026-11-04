use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::{AppendMessage, PreparedTransaction};
use crate::pact_file::append_state::AppendState;
use crate::pact_file::backend::IoCounterSet;
use crate::pact_file::snapshot::Snapshot;
use crate::pact_file::transaction::LedgerDelta;
use crate::resolver::message_authoring::DeterministicIdentity;
use crate::store::{ContentRecord, PactTransactionTrace};
use crate::trace::BoundaryKind;
use crate::transaction_trace::{TraceOp, decode_transaction_trace};
use crate::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs,
    PactMemoryOptions,
};

fn options(prefix: &str) -> DeterministicAuthoringOptions {
    DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1).expect("options")
}

fn fixture(dir: &Path, turns: usize) -> PathBuf {
    let path = dir.join("prepare.pact");
    let tree_options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(options("as_")),
    };
    let mut tree = ContextTree::with_id_and_options("as_root", tree_options).expect("tree");
    tree.save_file(&path).expect("save");
    for turn in 0..turns {
        let role = if turn % 2 == 0 { "user" } else { "assistant" };
        let parts = (0..2)
            .map(|part| {
                let options = tree.next_authoring_component_options();
                Component::text_authoring(format!("turn {turn} part {part}"), options)
            })
            .collect();
        tree.add_message(role, parts, PactAttrs::new())
            .expect("stage");
        tree.seal_staged(BoundaryKind::Adhoc).expect("seal");
    }
    path
}

struct Message {
    role: &'static str,
    parts: Vec<Value>,
    aux: BTreeMap<String, Value>,
}

fn message(role: &'static str, parts: Vec<Value>) -> Message {
    Message {
        role,
        parts,
        aux: BTreeMap::new(),
    }
}

fn parts_of(message: &Message) -> Vec<Component> {
    message
        .parts
        .iter()
        .map(|value| Component::from_component_value(value).expect("part"))
        .collect()
}

struct Eager {
    ops: Vec<TraceOp>,
    from_rev: u64,
    to_rev: u64,
    public_trace: PactTransactionTrace,
    delta: LedgerDelta,
    new_content: Vec<ContentRecord>,
    head_metadata: crate::pact_file::index::HeadMetadata,
}

fn eager_append(path: &Path, messages: &[Message]) -> Eager {
    let copy = path.with_extension("eager.pact");
    std::fs::copy(path, &copy).expect("copy");
    let tree_options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(options("rs_")),
    };
    let mut tree = ContextTree::open_file_with_options(&copy, None, tree_options).expect("open");
    tree.resolver_mut().record_file_commits();
    for message in messages {
        tree.add_message(message.role, parts_of(message), message.aux.clone())
            .expect("stage");
    }
    tree.seal_staged(BoundaryKind::Adhoc)
        .expect("seal")
        .expect("a commit");
    let mut commits = tree.resolver_mut().take_file_commits();
    assert_eq!(commits.len(), 1, "one seal, one file commit");
    let commit = commits.remove(0);

    let mut public_trace = commit.public_trace.expect("public trace");
    public_trace.replay_trace = None;
    Eager {
        ops: commit.trace.ops,
        from_rev: commit.trace.from_rev,
        to_rev: commit.trace.to_rev,
        public_trace,
        delta: commit.delta,
        new_content: commit.new_content,
        head_metadata: commit.head_metadata,
    }
}

fn prepared_append(path: &Path, messages: Vec<Message>) -> PreparedTransaction {
    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let memory = PactMemoryOptions::default();
    let mut state = AppendState::bootstrap(&snapshot, "main", &memory).expect("bootstrap");
    let mut identity = DeterministicIdentity::new(options("rs_")).expect("identity");
    let batch = messages
        .into_iter()
        .map(|message| AppendMessage {
            role: message.role.to_string(),
            parts: parts_of(&message),
            aux: message.aux,
        })
        .collect();
    state
        .prepare(&snapshot, batch, Vec::new(), &mut identity, &memory)
        .expect("prepare")
}

fn assert_parity(eager: &Eager, prepared: &PreparedTransaction) {
    let mut emitted = Vec::new();
    prepared.trace.emit(&mut emitted).expect("emit");
    let decoded = decode_transaction_trace(&emitted).expect("decode");
    assert_eq!(
        (decoded.from_rev, decoded.to_rev),
        (eager.from_rev, eager.to_rev)
    );
    assert_eq!(
        (prepared.from_revision, prepared.to_revision),
        (eager.from_rev, eager.to_rev)
    );
    assert_eq!(decoded.ops.len(), eager.ops.len(), "op count");
    for (index, (mine, theirs)) in decoded.ops.iter().zip(&eager.ops).enumerate() {
        assert_eq!(mine, theirs, "op {index}");
    }
    assert_eq!(prepared.public_trace, eager.public_trace, "public trace");
    assert_eq!(prepared.delta, eager.delta, "ledger delta");
    assert_eq!(prepared.new_content, eager.new_content, "new content");
    assert_eq!(prepared.transaction_id, eager.public_trace.transaction_id);
    assert_eq!(
        prepared.metadata.encode().expect("encode"),
        eager.head_metadata.encode().expect("encode"),
        "head metadata"
    );
}

fn run(turns: usize, messages: Vec<Message>) -> (Eager, PreparedTransaction) {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), turns);
    let eager = eager_append(&path, &messages);
    let prepared = prepared_append(&path, messages);
    assert_parity(&eager, &prepared);
    (eager, prepared)
}

fn text(content: &str) -> Value {
    json!({"componentType": "text_content", "content": content})
}

#[test]
fn two_text_messages_match_the_eager_seal_exactly() {
    let (eager, prepared) = run(
        3,
        vec![
            message("user", vec![text("hello"), text("there")]),
            message("assistant", vec![text("hi")]),
        ],
    );

    assert_eq!(prepared.shift_ops, 9 + 9 + 3);
    assert_eq!(prepared.inserts.len(), 2);
    assert_eq!(
        prepared.inserts[0].index, 4,
        "system header + 3 turns come first"
    );
    assert_eq!(prepared.inserts[1].index, 5);
    assert_eq!(eager.delta.first_seen_additions.len(), 5);
    assert!(eager.new_content.is_empty());
}

#[test]
fn large_bodies_reasoning_and_aux_externalize_identically() {
    let big = "b".repeat(3_000);
    let mut aux = BTreeMap::new();
    aux.insert("note".to_string(), json!("n".repeat(2_000)));
    aux.insert("k".to_string(), json!({"nested": [1, 2, 3]}));
    let mut with_aux = message(
        "assistant",
        vec![
            json!({"componentType": "reasoning_content", "content": "thinking"}),
            text(&big),
            text(&big),
            json!({"componentType": "text_content", "content": "small", "aux": {"blob": "z".repeat(1_500)}}),
        ],
    );
    with_aux.aux = aux;
    let (eager, prepared) = run(2, vec![with_aux]);
    assert!(
        eager.new_content.len() >= 2,
        "the big body and the big aux values became content records"
    );
    assert_eq!(prepared.new_content.len(), eager.new_content.len());
    let body_records = eager
        .new_content
        .iter()
        .filter(|record| record.kind == "node.content")
        .count();
    assert_eq!(
        body_records, 1,
        "identical bodies deduplicate to one record"
    );
    assert!(
        eager.public_trace.authored_ref.content_ref.is_some(),
        "the first produced content ref becomes the authored ref"
    );
}

#[test]
fn tool_pairs_and_nested_containers_match() {
    let params = json!({"query": "q".repeat(2_000), "limit": 3});
    let result =
        json!({"rows": (0..300).map(|i| json!({"i": i, "v": "x".repeat(8)})).collect::<Vec<_>>()});
    let (_, prepared) = run(
        1,
        vec![
            message(
                "assistant",
                vec![
                    text("calling"),
                    json!({"componentType": "tool_call", "tool_call_id": "call_1", "tool_name": "search", "parameters": params}),
                ],
            ),
            message(
                "user",
                vec![
                    json!({"componentType": "tool_result", "tool_call_id": "call_1", "tool_name": "search", "success": true, "result": result}),
                    json!({"componentType": "container", "type": "group", "children": [text("inner a"), json!({"componentType": "container", "type": "group", "children": [text("deep")]})]}),
                ],
            ),
        ],
    );
    assert_eq!(
        prepared.inserts[1].produced_ids.len(),
        6,
        "turn + result + group + a + group + deep"
    );
}

#[test]
fn system_messages_append_under_the_header_without_shifting() {
    let (eager, prepared) = run(
        2,
        vec![
            message("system", vec![text("rule one"), text("rule two")]),
            message("user", vec![text("after")]),
        ],
    );
    assert_eq!(prepared.inserts[0].parent_id, "system");
    assert_eq!(prepared.inserts[0].index, 0);
    assert_eq!(
        prepared.shift_ops, 6,
        "only the user turn shifts the two old turns"
    );
    assert_eq!(eager.delta.first_seen_additions.len(), 3);
}

#[test]
fn explicit_and_reused_ids_follow_the_native_collision_rule() {
    let (eager, _) = run(
        2,
        vec![message(
            "user",
            vec![
                json!({"componentType": "text_content", "content": "explicit", "id": "custom_part"}),
                json!({"componentType": "text_content", "content": "reused", "id": "elsewhere:as_00000001"}),
                json!({"componentType": "text_content", "content": "dup", "id": "custom_part"}),
            ],
        )],
    );
    let inserted = eager
        .ops
        .iter()
        .filter(|op| {
            matches!(
                op.kind,
                crate::transaction_trace::TraceOpKind::InsertComponent { .. }
            )
        })
        .map(|op| op.component_id.as_str())
        .collect::<Vec<_>>();
    assert!(inserted.contains(&"as_root:custom_part"));
    assert!(!inserted.contains(&"as_root:as_00000001"));
}

#[test]
fn a_fresh_genesis_is_ineligible_until_the_eager_first_commit_prunes_its_hollow_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 0);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let Err(error) = AppendState::bootstrap(&snapshot, "main", &PactMemoryOptions::default())
    else {
        panic!("a hollow turn must be refused");
    };
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);
    assert!(error.to_string().contains("hollow"), "{error}");

    let messages = vec![message("user", vec![text("first ever")])];
    let eager = eager_append(&path, &messages);

    assert_eq!(eager.delta.base_ref.revision, Some(0));
    assert!(eager.delta.base_ref.content_ref.is_some());
    assert!(
        eager.ops.iter().any(|op| op.component_id == "active"
            && matches!(
                op.kind,
                crate::transaction_trace::TraceOpKind::DeleteComponent
            )),
        "and prunes the hollow active turn"
    );
}

#[test]
fn media_parts_match_including_commit_time_externalization() {
    let data = "A".repeat(4_000);
    let (eager, prepared) = run(
        1,
        vec![message(
            "user",
            vec![
                json!({"componentType": "media_content", "media_kind": "image", "mime_type": "image/png", "data": data}),
                text("caption"),
            ],
        )],
    );
    assert_eq!(prepared.new_content.len(), eager.new_content.len());
}

#[test]
fn invalid_batches_are_refused_before_any_state_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let memory = PactMemoryOptions::default();
    let mut state = AppendState::bootstrap(&snapshot, "main", &memory).expect("bootstrap");
    let mut identity = DeterministicIdentity::new(options("rs_")).expect("identity");
    let refuse = |state: &mut AppendState,
                  identity: &mut DeterministicIdentity,
                  batch: Vec<AppendMessage>| {
        state
            .prepare(&snapshot, batch, Vec::new(), identity, &memory)
            .err()
            .expect("refused")
    };
    let empty = refuse(&mut state, &mut identity, Vec::new());
    assert_eq!(empty.kind(), crate::PactErrorKind::Invalid);
    let no_parts = refuse(
        &mut state,
        &mut identity,
        vec![AppendMessage {
            role: "user".into(),
            parts: Vec::new(),
            aux: BTreeMap::new(),
        }],
    );
    assert_eq!(no_parts.kind(), crate::PactErrorKind::Invalid);
    let bad_role = refuse(
        &mut state,
        &mut identity,
        vec![AppendMessage {
            role: "tool".into(),
            parts: vec![Component::from_component_value(&text("x")).unwrap()],
            aux: BTreeMap::new(),
        }],
    );
    assert_eq!(bad_role.kind(), crate::PactErrorKind::Invalid);
    let empty_system = refuse(
        &mut state,
        &mut identity,
        vec![AppendMessage {
            role: "system".into(),
            parts: vec![Component::from_component_value(&text("")).unwrap()],
            aux: BTreeMap::new(),
        }],
    );
    assert_eq!(empty_system.kind(), crate::PactErrorKind::Invalid);
    let with_ttl = refuse(
        &mut state,
        &mut identity,
        vec![AppendMessage {
            role: "user".into(),
            parts: vec![
                Component::from_component_value(
                    &json!({"componentType": "text_content", "content": "x", "ttl": 3}),
                )
                .unwrap(),
            ],
            aux: BTreeMap::new(),
        }],
    );
    assert_eq!(with_ttl.kind(), crate::PactErrorKind::Unsupported);
    let hollow = refuse(
        &mut state,
        &mut identity,
        vec![AppendMessage {
            role: "user".into(),
            parts: vec![
                Component::from_component_value(
                    &json!({"componentType": "message_container", "children": []}),
                )
                .unwrap(),
            ],
            aux: BTreeMap::new(),
        }],
    );
    assert_eq!(hollow.kind(), crate::PactErrorKind::Unsupported);
    assert_eq!(state.root_child_count, 2, "nothing was admitted");
}

struct ImageReader(Vec<u8>);

impl crate::pact_file::btree::PageReader for ImageReader {
    fn read_page(
        &self,
        address: u64,
    ) -> crate::PactResult<[u8; crate::pact_file::btree::PAGE_BYTES]> {
        crate::pact_file::codec::decode_index_page_record(&self.0, address)
    }

    fn read_component_payload(&self, address: u64, ordinal: u32) -> crate::PactResult<Vec<u8>> {
        crate::pact_file::codec::decode_component_payload_batch_record(&self.0, address, ordinal)
    }
}

fn staged_index_parity(
    path: &Path,
    eager: &Eager,
    prepared: &PreparedTransaction,
    in_flight_bytes: usize,
) {
    use crate::pact_file::btree::{CowBTree, PageReader};
    use crate::pact_file::index::{
        AppendIndexInputs, ContentIndexInputs, IndexKey, build_transaction_index,
        indexed_component_from_facet, message_order_key, parent_children_from_pages,
        read_index_value_from_pages, stage_append_index,
    };
    use crate::pact_file::transaction::PactTransaction;
    use crate::transaction_trace::TransactionTrace;

    let snapshot = Snapshot::open(path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let generation = snapshot.generation() + 1;
    let transaction_address = snapshot.committed_len();
    let index_start_address = transaction_address;
    let empty_bodies = BTreeMap::new();
    let empty_manifests = BTreeMap::new();
    assert!(
        prepared.new_content.is_empty(),
        "index parity fixtures carry no new content (content records are laid out by the commit)"
    );

    let mut eager_transaction = PactTransaction::delta(
        eager.public_trace.transaction_id.clone(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: eager.from_rev,
            to_rev: eager.to_rev,
            ops: eager.ops.clone(),
        },
        eager.public_trace.clone(),
        eager.new_content.clone(),
        eager.delta.clone(),
    );
    eager_transaction.head_metadata = Some(eager.head_metadata.clone());
    let eager_batch = build_transaction_index(
        &eager_transaction,
        generation,
        transaction_address,
        Some(&snapshot),
        Some(snapshot.index_root()),
        index_start_address,
        ContentIndexInputs {
            body_addresses: &empty_bodies,
            manifest_addresses: &empty_manifests,
            legacy_content_id_values: false,
            component_batch_address: None,
            baseline_branches_override: None,
            text_changes_override: None,
            search_changes_override: None,
        },
    )
    .expect("eager index");

    let shell = PactTransaction::new(
        prepared.transaction_id.clone(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: prepared.from_revision,
            to_rev: prepared.to_revision,
            ops: Vec::new(),
        },
        prepared.public_trace.clone(),
        Vec::new(),
    );
    let extensions =
        crate::extension::indexed_records_from_transaction(&shell).expect("extensions");
    let ops = prepared
        .index_ops
        .iter()
        .map(|(_, op)| op.clone())
        .collect::<Vec<_>>();
    let staged = stage_append_index(
        &snapshot,
        AppendIndexInputs {
            branch_id: "main",
            revision: prepared.to_revision,
            generation,
            transaction_address,
            index_start_address,
            existing_root: snapshot.index_root(),
            metadata: &prepared.metadata,
            ops: &ops,
            extensions: &extensions,
            text_changes: &prepared.text_changes,
            search_changes: &prepared.search_changes,
            new_content: &prepared.new_content,
            body_addresses: &empty_bodies,
            manifest_addresses: &empty_manifests,
            in_flight_bytes,
            io_chunk_bytes: 4096,
            dangling_relations: &[],
            order_repair: None,
        },
    )
    .expect("stage index");
    assert!(staged.staged_page_count() > 0);
    assert_eq!(
        staged.spilled(),
        in_flight_bytes < staged.staged_page_count() * 768
    );
    let mine_batch = staged.finish().expect("finish");

    let base = {
        let mut image = std::fs::read(path).expect("read");
        image.truncate(transaction_address as usize);
        image
    };
    let image = |batch: &crate::pact_file::index::IndexBatch| {
        let mut bytes = base.clone();
        bytes.extend_from_slice(&batch.bytes);
        ImageReader(bytes)
    };
    let (mine, eager_image) = (image(&mine_batch), image(&eager_batch));
    let (reader, eager_reader): (&dyn PageReader, &dyn PageReader) = (&mine, &eager_image);
    let (root, eager_root) = (mine_batch.root_address, eager_batch.root_address);

    let facet = |pages: &dyn PageReader, root: u64, id: &str| {
        let key = IndexKey::Component {
            branch: "main".into(),
            component: id.into(),
        };
        let bytes = read_index_value_from_pages(pages, root, &key)
            .expect("facet read")
            .unwrap_or_else(|| panic!("facet {id} present"));
        let (indexed, _) =
            indexed_component_from_facet(&CowBTree::from_root(Some(root)), pages, &bytes)
                .expect("facet decode");
        indexed
    };
    let mut parents = vec!["as_root".to_string()];
    for insert in &prepared.inserts {
        for id in &insert.produced_ids {
            assert_eq!(
                facet(reader, root, id),
                facet(eager_reader, eager_root, id),
                "component facet {id}"
            );
            parents.push(id.clone());
        }
        parents.push(insert.parent_id.clone());
    }
    parents.sort();
    parents.dedup();
    for parent in &parents {
        assert_eq!(
            parent_children_from_pages(reader, root, "main", parent).expect("children"),
            parent_children_from_pages(eager_reader, eager_root, "main", parent).expect("children"),
            "children of {parent}"
        );
    }
    let range = |pages: &dyn PageReader, root: u64, start: &IndexKey, end: &IndexKey| {
        CowBTree::from_root(Some(root))
            .range_limit(
                pages,
                &start.encode().unwrap(),
                &end.encode().unwrap(),
                100_000,
                false,
            )
            .expect("range")
    };
    let order_start = IndexKey::MessageOrder {
        branch: "main".into(),
        order: message_order_key(prepared.to_revision, 0).unwrap(),
    };
    let order_end = IndexKey::MessageOrder {
        branch: "main".into(),
        order: message_order_key(prepared.to_revision + 1, 0).unwrap(),
    };
    let staged_orders = range(reader, root, &order_start, &order_end);
    assert_eq!(
        staged_orders,
        range(eager_reader, eager_root, &order_start, &order_end),
        "message order rows"
    );
    assert_eq!(staged_orders.len() as u64, prepared.inserted_messages);
    let text_start = IndexKey::TextAddress {
        branch: "main".into(),
        revision: prepared.to_revision,
        ordinal: 0,
        component: String::new(),
        field: String::new(),
    };
    let text_end = IndexKey::TextAddress {
        branch: "main".into(),
        revision: prepared.to_revision + 1,
        ordinal: 0,
        component: String::new(),
        field: String::new(),
    };
    let text_rows = range(reader, root, &text_start, &text_end);
    assert_eq!(
        text_rows,
        range(eager_reader, eager_root, &text_start, &text_end),
        "text change rows"
    );
    assert_eq!(text_rows.len(), prepared.text_changes.len());
    let search_start = IndexKey::SearchChange {
        branch: "main".into(),
        revision: prepared.to_revision,
        ordinal: 0,
        component: String::new(),
    };
    let search_end = IndexKey::SearchChange {
        branch: "main".into(),
        revision: prepared.to_revision + 1,
        ordinal: 0,
        component: String::new(),
    };
    let search_rows = range(reader, root, &search_start, &search_end);
    assert_eq!(
        search_rows,
        range(eager_reader, eager_root, &search_start, &search_end),
        "search change rows"
    );
    assert_eq!(search_rows.len(), prepared.search_changes.len());
    for key in [
        IndexKey::Metadata,
        IndexKey::Revision {
            branch: "main".into(),
            revision: prepared.to_revision,
        },
        IndexKey::Generation { generation },
        IndexKey::TextChangeFacet,
        IndexKey::SearchChangeFacet,
        IndexKey::ParentChildFacet,
    ] {
        assert_eq!(
            read_index_value_from_pages(reader, root, &key).expect("row"),
            read_index_value_from_pages(eager_reader, eager_root, &key).expect("row"),
            "{key:?}"
        );
    }
}

#[test]
fn staged_index_matches_the_eager_index_for_text_and_system_messages() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 3);
    let messages = vec![
        message("user", vec![text("hello"), text("there")]),
        message("system", vec![text("rule")]),
        message("assistant", vec![text("hi"), text(&"y".repeat(900))]),
    ];
    let eager = eager_append(&path, &messages);
    let prepared = prepared_append(&path, messages);
    assert_parity(&eager, &prepared);
    assert_eq!(prepared.inserted_messages, 2);
    assert_eq!(prepared.inserted_nodes, 3 + 1 + 3);
    assert!(
        prepared.trace_symbols > 25,
        "session symbols follow the well-known prefix"
    );
    assert!(
        prepared.trace_scratch_bytes > 0,
        "the trace lives on scratch"
    );
    assert!(!prepared.text_changes.is_empty());
    assert!(!prepared.search_changes.is_empty());
    assert!(
        prepared
            .index_ops
            .iter()
            .all(|(_, op)| !matches!(&op.kind, crate::transaction_trace::TraceOpKind::SetField { key, .. } if key == "pact_depth")),
        "derived depth ops never reach the index"
    );
    staged_index_parity(
        &path,
        &eager,
        &prepared,
        PactMemoryOptions::default().in_flight_bytes,
    );
}

#[test]
fn staged_index_reads_the_same_through_a_scratch_spill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 2);
    let messages = vec![message("user", vec![text("spill me")])];
    let eager = eager_append(&path, &messages);
    let prepared = prepared_append(&path, messages);
    assert_parity(&eager, &prepared);

    staged_index_parity(&path, &eager, &prepared, 768);
}

#[test]
fn message_order_preflight_refuses_a_batch_beyond_the_rank_field() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = fixture(temp.path(), 1);
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let memory = PactMemoryOptions::default();
    let mut state = AppendState::bootstrap(&snapshot, "main", &memory).expect("bootstrap");
    let mut identity = DeterministicIdentity::new(options("rs_")).expect("identity");
    let part = Component::from_component_value(&text("x")).expect("part");
    let batch = (0..65_537)
        .map(|_| AppendMessage {
            role: "user".to_string(),
            parts: vec![part.clone()],
            aux: BTreeMap::new(),
        })
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let Err(error) = state.prepare(&snapshot, batch, Vec::new(), &mut identity, &memory) else {
        panic!("65,537 message turns exceed the 16-bit rank field");
    };
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);
    assert!(error.to_string().contains("65536"), "{error}");
    assert_eq!(state.root_child_count, 2, "nothing advanced");
    assert!(
        started.elapsed().as_secs() < 5,
        "the preflight is a count, not a prepare"
    );

    let nested = Component::from_component_value(
        &json!({"componentType": "message_turn", "role": "user", "children": [text("inner")]}),
    )
    .expect("nested");
    let batch = (0..32_769)
        .map(|_| AppendMessage {
            role: "user".to_string(),
            parts: vec![nested.clone()],
            aux: BTreeMap::new(),
        })
        .collect::<Vec<_>>();
    assert!(
        state
            .prepare(&snapshot, batch, Vec::new(), &mut identity, &memory)
            .is_err()
    );

    let prepared = state
        .prepare(
            &snapshot,
            vec![
                AppendMessage {
                    role: "user".into(),
                    parts: vec![part.clone()],
                    aux: BTreeMap::new(),
                },
                AppendMessage {
                    role: "assistant".into(),
                    parts: vec![part],
                    aux: BTreeMap::new(),
                },
            ],
            Vec::new(),
            &mut identity,
            &memory,
        )
        .expect("prepare");
    assert!(prepared.shift_ops > 0);
    assert_eq!(prepared.inserted_messages, 2);
}
