use super::{IndexKey, IndexedComponent};

#[test]
fn every_namespaced_index_key_round_trips_without_collision() {
    let keys = vec![
        IndexKey::BranchHead {
            branch: "main".to_string(),
        },
        IndexKey::Revision {
            branch: "main".to_string(),
            revision: 42,
        },
        IndexKey::Component {
            branch: "main".to_string(),
            component: "component".to_string(),
        },
        IndexKey::ParentChild {
            branch: "main".to_string(),
            parent: "parent".to_string(),
            order: 7,
            child: "child".to_string(),
        },
        IndexKey::Type {
            branch: "main".to_string(),
            component_type: "text_content".to_string(),
            component: "component".to_string(),
        },
        IndexKey::Aux {
            branch: "main".to_string(),
            key: "role".to_string(),
            value_hash: [3; 16],
            component: "component".to_string(),
        },
        IndexKey::Tag {
            branch: "main".to_string(),
            tag: "important".to_string(),
            component: "component".to_string(),
        },
        IndexKey::MessageOrder {
            branch: "main".to_string(),
            order: 9,
        },
        IndexKey::ContentId {
            id: "cnt_1234".to_string(),
        },
        IndexKey::ContentHash { hash: [5; 32] },
        IndexKey::TextAddress {
            branch: "main".to_string(),
            revision: 11,
            ordinal: 3,
            component: "component".to_string(),
            field: "content".to_string(),
        },
        IndexKey::Generation { generation: 12 },
        IndexKey::Metadata,
        IndexKey::ParentChildFacet,
        IndexKey::ComponentPayload {
            segment_id: 12,
            ordinal: 3,
        },
        IndexKey::HeadCheckpoint,
        IndexKey::ComponentBatch { segment_id: 12 },
        IndexKey::TextChangeFacet,
        IndexKey::SearchChange {
            branch: "main".to_string(),
            revision: 13,
            ordinal: 5,
            component: "component".to_string(),
        },
        IndexKey::SearchChangeFacet,
        IndexKey::SearchSelectorFacet,
        IndexKey::ExtensionOrder {
            branch: "main".to_string(),
            revision: 21,
            ordinal: 2,
        },
        IndexKey::ExtensionKind {
            branch: "main".to_string(),
            kind: "pactree.fs.checkpoint".to_string(),
            revision: 21,
            ordinal: 2,
        },
        IndexKey::ExtensionKey {
            branch: "main".to_string(),
            kind: "pactree.fs.checkpoint".to_string(),
            key_hash: [6; 32],
            revision: 21,
            ordinal: 2,
        },
        IndexKey::ExtensionRecord {
            branch: "main".to_string(),
            record_hash: [7; 32],
        },
    ];

    let encoded = keys
        .iter()
        .map(IndexKey::encode)
        .collect::<crate::PactResult<Vec<_>>>()
        .expect("encode keys");
    assert_eq!(
        encoded
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        keys.len()
    );
    for (key, bytes) in keys.iter().zip(encoded) {
        assert_eq!(&IndexKey::decode(&bytes).expect("decode key"), key);
    }
}

#[test]
fn typed_baseline_component_ops_match_the_json_differ_exactly() {
    let ledger = crate::pact_file::tests::fixture_ledger(40);
    let snapshot = serde_json::to_value(&ledger).expect("ledger snapshot");
    let expected = super::baseline_component_ops(&snapshot)
        .expect("json baseline ops")
        .into_iter()
        .filter(|op| {
            matches!(
                op.kind,
                crate::transaction_trace::TraceOpKind::InsertComponent { .. }
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        super::baseline_component_ops_from_ledger(&ledger).expect("typed baseline ops"),
        expected
    );
}

#[test]
fn indexed_components_compress_and_round_trip_without_filling_a_leaf() {
    let component = IndexedComponent {
        revision: 9,
        order: Some(4),
        child_count: 0,
        value: serde_json::json!({
            "id": "tool-result",
            "componentType": "tool_result",
            "child_ids": ["grandchild"],
            "aux": {"diagnostic": "repeated-index-value ".repeat(256)}
        }),
    };

    let encoded = component.encode().expect("encode indexed component");

    assert!(encoded.starts_with(b"PIC4"));
    assert!(encoded.len() < 1024);
    let mut expected = component;
    expected.value = crate::Component::from_component_value(&expected.value)
        .expect("canonical component")
        .project_component_value();
    assert_eq!(
        IndexedComponent::decode(&encoded).expect("decode"),
        expected
    );
}

#[test]
fn canonical_component_batches_do_not_nest_per_component_compression() {
    let component = IndexedComponent {
        revision: 9,
        order: None,
        child_count: 0,
        value: serde_json::json!({
            "id": "tool-call",
            "componentType": "tool_call",
            "parameters": {"command": "repeated batch payload ".repeat(512)}
        }),
    };

    let encoded = component
        .encode_for_batch()
        .expect("encode canonical batch component");

    assert!(encoded.starts_with(b"PIC3"));
    let mut expected = component;
    expected.value = crate::Component::from_component_value(&expected.value)
        .expect("canonical component")
        .project_component_value();
    assert_eq!(
        IndexedComponent::decode(&encoded).expect("decode canonical batch component"),
        expected
    );
}

#[test]
fn indexed_component_codec_preserves_consumer_owned_extension_fields() {
    let extension = serde_json::json!({
        "provider": "lens",
        "nested": {"selector_surface": ["alpha", 7, true]}
    });
    let component = IndexedComponent {
        revision: 9,
        order: None,
        child_count: 0,
        value: serde_json::json!({
            "id": "extension-component",
            "componentType": "lens_extension",
            "extension_payload": extension,
        }),
    };

    let encoded = component.encode_for_batch().expect("encode extension");
    let decoded = IndexedComponent::decode(&encoded).expect("decode extension");

    assert_eq!(
        decoded.value["extension_payload"],
        component.value["extension_payload"]
    );
}

#[test]
fn indexed_component_codec_keeps_parent_but_omits_derived_child_lists() {
    let component = IndexedComponent {
        revision: 9,
        order: None,
        child_count: 1,
        value: serde_json::json!({
            "id": "system",
            "componentType": "system_header",
            "parent_id": null,
            "child_ids": ["child"],
            "children": [],
        }),
    };

    let encoded = component.encode_for_batch().expect("encode structure");
    let decoded = IndexedComponent::decode(&encoded).expect("decode structure");

    assert_eq!(decoded.value["parent_id"], serde_json::Value::Null);
    assert_eq!(decoded.value["child_ids"], serde_json::json!([]));
}

use crate::pact_file::btree::{CowBTree, PAGE_BYTES, PageReader, PageWriter};
use crate::pact_file::codec::{
    INDEX_BATCH_TARGET_PAGES, MAX_INDEX_BATCH_PAGES, RECORD_HEADER_BYTES,
    decode_index_page_batch_record, encode_index_page_batch_record, index_page_batch_address,
    index_page_ordinal, index_page_record_len, pack_index_page_address,
};
use crate::pact_file::layout::HEADER_BYTES;

fn overflow_chain_entries(total_pages: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    const KEYS: usize = 20;
    const CHUNK: usize = PAGE_BYTES - 64 - 8;
    assert!(
        total_pages > KEYS,
        "fixture needs at least one chunk per key"
    );
    let chunks = total_pages - 1;
    (0..KEYS)
        .map(|n| {
            let count = chunks / KEYS + usize::from(n < chunks % KEYS);
            (
                format!("k{n:02}").into_bytes(),
                vec![b'a' + n as u8; count * CHUNK],
            )
        })
        .collect()
}

fn staged_with_pages(total_pages: usize) -> (super::StagedPages<'static>, u64) {
    let mut pages = super::StagedPages::new(None, HEADER_BYTES as u64, 7);
    let tree =
        CowBTree::bulk_load(&mut pages, overflow_chain_entries(total_pages)).expect("bulk load");
    let root = tree.root_address().expect("staged root");
    assert_eq!(
        pages.staged.len(),
        total_pages,
        "fixture must stage exactly the requested pages"
    );
    (pages, root)
}

fn index_batch_records(file: &[u8], mut address: u64) -> Vec<(u64, usize)> {
    let mut records = Vec::new();
    while (address as usize) < file.len() {
        let start = address as usize;
        let len = index_page_record_len(&file[start..start + RECORD_HEADER_BYTES])
            .expect("index page record header");
        let pages = decode_index_page_batch_record(file, address).expect("index batch");
        records.push((address, pages.len()));
        address += len;
    }
    records
}

struct CachedExistingPages<'a> {
    file: &'a [u8],
    batches: std::cell::RefCell<std::collections::BTreeMap<u64, Vec<[u8; PAGE_BYTES]>>>,
}

impl PageReader for CachedExistingPages<'_> {
    fn read_page(&self, address: u64) -> crate::PactResult<[u8; PAGE_BYTES]> {
        let batch = index_page_batch_address(address);
        let mut batches = self.batches.borrow_mut();
        if let std::collections::btree_map::Entry::Vacant(entry) = batches.entry(batch) {
            entry.insert(decode_index_page_batch_record(self.file, batch)?);
        }
        batches[&batch]
            .get(index_page_ordinal(address))
            .copied()
            .ok_or_else(|| crate::PactError::corrupt("Pact index page ordinal is out of range"))
    }
}

fn assert_every_value_reachable(file: &[u8], root: u64, total_pages: usize) {
    let reader = CachedExistingPages {
        file,
        batches: std::cell::RefCell::new(std::collections::BTreeMap::new()),
    };
    let tree = CowBTree::from_root(Some(root));
    for (key, expected) in overflow_chain_entries(total_pages) {
        let value = tree
            .get(&reader, &key)
            .expect("get")
            .expect("value present");
        assert_eq!(value, expected);
    }
}

#[test]
fn index_batches_exactly_at_the_ordinal_limit_stay_in_one_record() {
    let (pages, root) = staged_with_pages(MAX_INDEX_BATCH_PAGES);
    let (bytes, packed_root) = pages
        .finish_with_batch_limit(root, MAX_INDEX_BATCH_PAGES)
        .expect("finish");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&bytes);
    let records = index_batch_records(&file, HEADER_BYTES as u64);
    assert_eq!(records, vec![(HEADER_BYTES as u64, MAX_INDEX_BATCH_PAGES)]);

    assert_eq!(index_page_batch_address(packed_root), HEADER_BYTES as u64);
    assert_eq!(index_page_ordinal(packed_root), MAX_INDEX_BATCH_PAGES - 1);
    assert_every_value_reachable(&file, packed_root, MAX_INDEX_BATCH_PAGES);
}

#[test]
fn index_batches_one_past_the_ordinal_limit_split_and_restart_ordinals() {
    let total = MAX_INDEX_BATCH_PAGES + 1;
    let (pages, root) = staged_with_pages(total);
    let (bytes, packed_root) = pages
        .finish_with_batch_limit(root, MAX_INDEX_BATCH_PAGES)
        .expect("finish");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&bytes);
    let records = index_batch_records(&file, HEADER_BYTES as u64);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], (HEADER_BYTES as u64, MAX_INDEX_BATCH_PAGES));
    assert_eq!(records[1].1, 1);

    assert_eq!(index_page_batch_address(packed_root), records[1].0);
    assert_eq!(index_page_ordinal(packed_root), 0);

    assert_every_value_reachable(&file, packed_root, total);
}

#[test]
fn index_batches_default_finish_targets_small_records_old_readers_resolve() {
    let total = 2 * INDEX_BATCH_TARGET_PAGES + 1;
    let (pages, root) = staged_with_pages(total);
    let (bytes, packed_root) = pages.finish(root).expect("finish");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&bytes);
    let records = index_batch_records(&file, HEADER_BYTES as u64);
    assert_eq!(
        records.iter().map(|(_, count)| *count).collect::<Vec<_>>(),
        vec![INDEX_BATCH_TARGET_PAGES, INDEX_BATCH_TARGET_PAGES, 1]
    );
    assert!(
        records
            .iter()
            .all(|(_, count)| *count <= MAX_INDEX_BATCH_PAGES)
    );
    assert_every_value_reachable(&file, packed_root, total);
}

#[test]
fn index_batches_reject_an_empty_transaction_index_like_before() {
    let pages = super::StagedPages::new(None, HEADER_BYTES as u64, 7);
    let error = pages.finish(0).expect_err("no staged pages");
    assert_eq!(error.kind(), crate::PactErrorKind::Invalid);
}

#[test]
fn index_batches_small_transactions_are_byte_identical_to_a_single_record() {
    let total = INDEX_BATCH_TARGET_PAGES - 1;
    let (pages, root) = staged_with_pages(total);
    let (chunked, chunked_root) = pages.finish(root).expect("finish");
    let (pages, root) = staged_with_pages(total);
    let (single, single_root) = pages
        .finish_with_batch_limit(root, MAX_INDEX_BATCH_PAGES)
        .expect("finish");
    assert_eq!(chunked, single);
    assert_eq!(chunked_root, single_root);
}

#[test]
fn staged_virtual_addresses_never_shadow_existing_packed_pages() {
    let existing_page = [b'e'; PAGE_BYTES];
    let record = encode_index_page_batch_record(1, &[existing_page]).expect("encode");
    let mut file = vec![0_u8; HEADER_BYTES];
    file.extend_from_slice(&record);
    let existing = super::ExistingPages { file: &file };
    let packed = pack_index_page_address(HEADER_BYTES as u64, 0).expect("pack");

    let mut staged = super::StagedPages::new(Some(&existing), packed, 2);
    let staged_address = staged.write_page([b's'; PAGE_BYTES]).expect("write");
    assert_ne!(staged_address, packed);
    assert_eq!(
        staged.read_page(packed).expect("existing page"),
        existing_page
    );
}

use crate::pact_file::transaction::PactTransaction;
use crate::trace::BoundaryKind;
use crate::transaction_trace::{TraceOp, TraceOpKind, TransactionTrace};

fn message_insert(component_id: &str, index: usize) -> TraceOp {
    TraceOp::new(
        component_id.to_string(),
        TraceOpKind::InsertComponent {
            parent_id: "root".to_string(),
            index,
            component: serde_json::json!({"componentType": "message_turn", "role": "user"}),
        },
    )
}

fn delta_transaction(from_rev: u64, to_rev: u64, ops: Vec<TraceOp>) -> PactTransaction {
    let transaction_id = format!("tx_test_{to_rev}");
    let public_trace =
        crate::pact_file::synthetic_public_trace("root", "main", &transaction_id, from_rev, to_rev);
    PactTransaction::new(
        transaction_id,
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev,
            to_rev,
            ops,
        },
        public_trace,
        Vec::new(),
    )
}

fn message_orders(file: &[u8], root: u64) -> Vec<(u64, String)> {
    let reader = super::ExistingPages { file };
    let tree = CowBTree::from_root(Some(root));
    let start = IndexKey::MessageOrder {
        branch: "main".to_string(),
        order: 0,
    }
    .encode()
    .unwrap();
    let end = IndexKey::MessageOrder {
        branch: "main".to_string(),
        order: u64::MAX,
    }
    .encode()
    .unwrap();
    tree.range(&reader, &start, &end)
        .expect("range")
        .into_iter()
        .map(|(key, value)| {
            let IndexKey::MessageOrder { order, .. } = IndexKey::decode(&key).expect("key") else {
                panic!("message order range returned another namespace");
            };
            (order, String::from_utf8(value).expect("component id"))
        })
        .collect()
}

#[test]
fn message_order_key_rejects_rank_overflow_before_publication() {
    assert_eq!(super::message_order_key(6, 0).expect("rank 0"), 6 << 16);
    assert_eq!(
        super::message_order_key(6, 65_535).expect("rank 65535"),
        (6 << 16) | 65_535
    );
    let error = super::message_order_key(6, 65_536).expect_err("rank overflow");
    assert_eq!(error.kind(), crate::PactErrorKind::Unsupported);
    assert!(super::message_order_key(u64::MAX, 0).is_err());
}

#[test]
fn message_order_keys_use_message_rank_not_trace_ordinal() {
    let mut ops = vec![message_insert("m1", 0)];
    for n in 0..70_000_u32 {
        ops.push(TraceOp::new(
            format!("old{n}"),
            TraceOpKind::SetField {
                key: "pact_depth".to_string(),
                value: serde_json::json!(1),
            },
        ));
    }
    ops.push(message_insert("m2", 1));
    let transaction = delta_transaction(5, 6, ops);
    let start_address = HEADER_BYTES as u64 + 512;
    let empty = std::collections::BTreeMap::new();
    let index = super::build_transaction_index(
        &transaction,
        9,
        HEADER_BYTES as u64,
        None,
        None,
        start_address,
        super::ContentIndexInputs {
            body_addresses: &empty,
            manifest_addresses: &std::collections::BTreeMap::new(),
            legacy_content_id_values: false,
            component_batch_address: None,
            baseline_branches_override: None,
            text_changes_override: None,
            search_changes_override: None,
        },
    )
    .expect("index");
    let mut file = vec![0_u8; start_address as usize];
    file.extend_from_slice(&index.bytes);
    assert_eq!(
        message_orders(&file, index.root_address),
        vec![
            (6 << 16, "m1".to_string()),
            ((6 << 16) | 1, "m2".to_string())
        ]
    );
}

#[test]
fn eager_message_reordering_preserves_sparse_keys_and_baseline_tie_breaks() {
    let mut pages = super::StagedPages::new(None, 4096 << 15, 1);
    let mut tree = CowBTree::from_root(None);
    let rows = [
        ("z", "parent-z", 0),
        ("a", "parent-a", 0),
        ("root-b", "root", 1),
        ("root-a", "root", 0),
    ];
    for (rank, (id, _, _)) in rows.iter().enumerate() {
        let order = if rank + 1 == rows.len() {
            u64::MAX
        } else {
            super::message_order_key(7, rank as u64).unwrap()
        };
        let value = serde_json::json!({"id": id, "componentType": "message_turn",
            "pact_depth": rank, "offset": rank, "created_at_ns": rank});
        let indexed = IndexedComponent {
            revision: 7,
            order: Some(order),
            child_count: 0,
            value,
        };
        tree = super::write_indexed_component(
            tree,
            &mut pages,
            "main",
            &IndexKey::Component {
                branch: "main".into(),
                component: id.to_string(),
            },
            &indexed,
        )
        .unwrap();
        (tree, _) = tree
            .insert(
                &mut pages,
                IndexKey::MessageOrder {
                    branch: "main".into(),
                    order,
                }
                .encode()
                .unwrap(),
                id.as_bytes().to_vec(),
            )
            .unwrap();
    }
    for (id, parent, ordinal) in rows {
        (tree, _) = tree
            .insert(
                &mut pages,
                IndexKey::ParentChild {
                    branch: "main".into(),
                    parent: parent.to_string(),
                    order: ordinal,
                    child: id.to_string(),
                }
                .encode()
                .unwrap(),
                Vec::new(),
            )
            .unwrap();
    }
    for (ordinal, parent) in ["parent-z", "parent-a"].iter().enumerate() {
        (tree, _) = tree
            .insert(
                &mut pages,
                IndexKey::ParentChild {
                    branch: "main".into(),
                    parent: "root".into(),
                    order: ordinal as u64 + 2,
                    child: parent.to_string(),
                }
                .encode()
                .unwrap(),
                Vec::new(),
            )
            .unwrap();
    }
    tree = super::reorder_message_entries(tree, &mut pages, "main", "root").expect("reorder");
    for (rank, id) in ["root-a", "root-b", "a", "z"].iter().enumerate() {
        let order = if rank + 1 == rows.len() {
            u64::MAX
        } else {
            super::message_order_key(7, rank as u64).unwrap()
        };
        assert_eq!(
            tree.get(
                &pages,
                &IndexKey::MessageOrder {
                    branch: "main".into(),
                    order
                }
                .encode()
                .unwrap()
            )
            .unwrap()
            .unwrap(),
            id.as_bytes()
        );
        let locator = tree
            .get(
                &pages,
                &IndexKey::Component {
                    branch: "main".into(),
                    component: id.to_string(),
                }
                .encode()
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        let (indexed, _) = super::indexed_component_from_facet(&tree, &pages, &locator).unwrap();
        assert_eq!(indexed.order, Some(order));
        assert_eq!(indexed.revision, 7);
    }
    let before = tree.root_address();
    tree =
        super::reorder_message_entries(tree, &mut pages, "main", "root").expect("already ordered");
    assert_eq!(
        tree.root_address(),
        before,
        "ordered messages require no index writes"
    );
}

#[test]
fn tail_relations_are_verified_and_message_order_keys_must_be_fresh() {
    use super::{IndexKey, StagedPages, apply_append_insert, insert_child_relation_tail};
    use crate::pact_file::btree::CowBTree;

    let mut pages = StagedPages::new(None, 4096 << 15, 1);
    let mut tree = CowBTree::from_root(None);
    (tree, _) = tree
        .insert(
            &mut pages,
            IndexKey::ParentChildFacet.encode().unwrap(),
            vec![1],
        )
        .unwrap();
    for (index, child) in ["a", "b"].iter().enumerate() {
        tree = insert_child_relation_tail(tree, &mut pages, "main", "p", index, child).unwrap();
    }
    assert!(
        insert_child_relation_tail(tree.clone(), &mut pages, "main", "p", 1, "dup").is_err(),
        "an occupied ordinal is not a tail"
    );
    assert!(
        insert_child_relation_tail(tree.clone(), &mut pages, "main", "p", 5, "gap").is_err(),
        "a gap before the ordinal is not a tail"
    );
    tree = insert_child_relation_tail(tree, &mut pages, "main", "p", 2, "c").unwrap();
    let children = super::parent_children(&tree, &pages, "main", "p").unwrap();
    assert_eq!(children, vec!["a", "b", "c"]);

    let turn = serde_json::json!({"componentType": "message_turn", "role": "user"});
    let mut rank = 0;
    tree = apply_append_insert(
        tree, &mut pages, "main", 7, &mut rank, "m1", "root", 0, &turn, true,
    )
    .unwrap();
    assert_eq!(rank, 1);
    let mut replay_rank = 0;
    assert!(
        apply_append_insert(
            tree.clone(),
            &mut pages,
            "main",
            7,
            &mut replay_rank,
            "m2",
            "root",
            1,
            &turn,
            true
        )
        .is_err(),
        "the same revision/rank key is already present"
    );
    tree = apply_append_insert(
        tree, &mut pages, "main", 7, &mut rank, "m2", "root", 1, &turn, true,
    )
    .unwrap();
    assert_eq!(rank, 2);
    assert_eq!(
        super::parent_children(&tree, &pages, "main", "root").unwrap(),
        vec!["m1", "m2"]
    );
}

#[test]
fn child_ids_order_is_verified_against_the_relations_by_streaming() {
    use super::{StagedPages, insert_child_relation_tail, verify_child_ids_order};
    use crate::pact_file::btree::CowBTree;

    let mut pages = StagedPages::new(None, 4096 << 15, 1);
    let mut tree = CowBTree::from_root(None);
    let ids = (0..600).map(|i| format!("c{i:04}")).collect::<Vec<_>>();
    for (index, child) in ids.iter().enumerate() {
        tree = insert_child_relation_tail(tree, &mut pages, "main", "p", index, child).unwrap();
    }
    let value = serde_json::json!(ids);
    verify_child_ids_order(&tree, &pages, "main", "p", &value, true).expect("exact order");
    let mut swapped = ids.clone();
    swapped.swap(300, 301);
    let swapped = serde_json::json!(swapped);
    assert!(verify_child_ids_order(&tree, &pages, "main", "p", &swapped, false).is_err());
    let shorter = serde_json::json!(ids[..599]);
    assert!(verify_child_ids_order(&tree, &pages, "main", "p", &shorter, false).is_err());
    let mut longer = ids.clone();
    longer.push("extra".to_string());
    let longer = serde_json::json!(longer);
    assert!(verify_child_ids_order(&tree, &pages, "main", "p", &longer, true).is_err());
    verify_child_ids_order(&tree, &pages, "main", "p", &longer, false)
        .expect("relations may be a prefix of a nested parent's final list");
    let empty = serde_json::json!([]);
    verify_child_ids_order(&tree, &pages, "main", "absent", &empty, true).expect("no relations");
}
