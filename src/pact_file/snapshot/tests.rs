use std::collections::BTreeMap;
use std::sync::Arc;

use super::Snapshot;
use crate::pact_file::backend::CandidateBackend;
use crate::pact_file::backend::{IoCounterSet, PactFileBackend};
use crate::pact_file::transaction::PactTransaction;
use crate::store::{PactRef, PactTransactionTrace, TRANSACTION_TRACE_SCHEMA};
use crate::trace::BoundaryKind;
use crate::transaction_trace::TransactionTrace;

fn transaction(id: &str, from: u64, to: u64) -> PactTransaction {
    let pact_ref = PactRef {
        root_id: "root".to_string(),
        branch_id: "main".to_string(),
        revision: Some(to),
        transaction_id: Some(id.to_string()),
        component_id: None,
        content_ref: None,
    };
    PactTransaction::new(
        id.to_string(),
        "main".to_string(),
        BoundaryKind::Adhoc,
        TransactionTrace {
            from_rev: from,
            to_rev: to,
            ops: Vec::new(),
        },
        PactTransactionTrace {
            schema: TRANSACTION_TRACE_SCHEMA.to_string(),
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            transaction_id: id.to_string(),
            authored_ref: pact_ref.clone(),
            pre_request_ref: pact_ref.clone(),
            post_request_alteration_ref: pact_ref,
            request_ref: None,
            response_ref: None,
            post_response_alteration_ref: None,
            post_response_ref: None,
            alteration_ids: Vec::new(),
            commit: None,
            replay_trace: None,
            aux: BTreeMap::new(),
        },
        Vec::new(),
    )
}

#[test]
fn open_is_fixed_io_and_snapshot_stays_pinned_until_refresh() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("snapshot.pact");
    let backend = CandidateBackend::default();
    backend
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    let counters = Arc::new(IoCounterSet::default());
    let first = Snapshot::open(&path, Arc::clone(&counters)).expect("open first");
    let source_file_identity = first.source_file_identity();
    let opened = counters.snapshot();
    assert!(opened.bytes_read <= 4096 + 2 * 120);
    assert_eq!(opened.records_decoded, 0);
    assert_eq!(first.branch_head("main").expect("head"), Some(1));

    backend
        .commit(&path, &transaction("tx_2", 1, 2))
        .expect("commit");
    assert_eq!(first.generation(), 1);
    assert_eq!(first.branch_head("main").expect("pinned head"), Some(1));
    let refreshed = first.refresh().expect("refresh");
    assert_eq!(refreshed.generation(), 2);
    assert_eq!(refreshed.source_file_identity(), source_file_identity);
    assert_eq!(refreshed.branch_head("main").expect("new head"), Some(2));
}

#[test]
fn independently_created_files_have_distinct_source_identities() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let first_path = temp.path().join("first.pact");
    let second_path = temp.path().join("second.pact");
    let genesis = transaction("tx_1", 0, 1);
    let backend = CandidateBackend::default();
    backend.create(&first_path, &genesis).expect("create first");
    backend
        .create(&second_path, &genesis)
        .expect("create second");

    let first = Snapshot::open(&first_path, Arc::new(IoCounterSet::default())).expect("first");
    let second = Snapshot::open(&second_path, Arc::new(IoCounterSet::default())).expect("second");
    assert_ne!(first.source_file_identity(), second.source_file_identity());
}

#[test]
fn same_generation_snapshots_share_one_mapping() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("shared.pact");
    CandidateBackend::default()
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    let counters = Arc::new(IoCounterSet::default());
    let snapshots = (0..20)
        .map(|_| Snapshot::open(&path, Arc::clone(&counters)).expect("open"))
        .collect::<Vec<_>>();

    assert!(
        snapshots[1..]
            .iter()
            .all(|snapshot| snapshot.shares_mapping_with(&snapshots[0]))
    );

    snapshots[0].branch_head("main").expect("first read");
    let pages_after_first = counters.snapshot().pages_read;
    snapshots[0].branch_head("main").expect("repeat read");
    assert_eq!(counters.snapshot().pages_read, pages_after_first);
    snapshots[1]
        .branch_head("main")
        .expect("another handle's first read");
    assert!(counters.snapshot().pages_read > pages_after_first);
}

#[test]
fn byte_identical_replacement_has_new_mapping_and_preserves_old_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("replacement.pact");
    CandidateBackend::default()
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    let first = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("first snapshot");
    let bytes = std::fs::read(&path).expect("committed bytes");
    crate::atomic_output::publish_bytes(
        &path,
        &bytes,
        crate::atomic_output::PublishMode::ReplaceExisting,
    )
    .expect("replace mapped source");
    let second = first.refresh().expect("refreshed snapshot");
    assert_ne!(first.platform_id(), second.platform_id());
    assert!(!first.shares_mapping_with(&second));
    assert_eq!(first.source_file_identity(), second.source_file_identity());
    assert_eq!(first.branch_head("main").expect("old head"), Some(1));
    assert_eq!(second.branch_head("main").expect("new head"), Some(1));
}

#[test]
fn last_owner_drop_releases_the_mapping_and_prunes_the_registry() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("sequential-shared.pact");
    CandidateBackend::default()
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    let probe = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("open probe");
    let metadata = std::fs::metadata(&path).expect("metadata");
    let key = super::MappingKey::new(
        &std::fs::canonicalize(&path).expect("canonical path"),
        &metadata,
        probe.generation,
        probe.committed_len,
        probe.platform_id(),
    );
    let file = std::fs::File::open(&path).expect("open file");
    let mut registry = super::MappingRegistry::default();

    let first = registry
        .open(key.clone(), &file, probe.committed_len)
        .expect("open first");
    let first_mapping = Arc::downgrade(&first);
    let concurrent = registry
        .open(key.clone(), &file, probe.committed_len)
        .expect("open concurrent");
    assert!(
        Arc::ptr_eq(&first, &concurrent),
        "live handles share one mapping"
    );
    drop(first);
    drop(concurrent);
    assert!(
        first_mapping.upgrade().is_none(),
        "the registry holds no strong reference: the last owner releases the mapping"
    );
    let second = registry
        .open(key, &file, probe.committed_len)
        .expect("open second");
    assert_eq!(registry.mappings.len(), 1, "dead keys are pruned");
    assert_eq!(second.mapping.len(), probe.committed_len as usize);
}

#[test]
fn component_facet_stores_a_locator_not_component_json() {
    let fixture = crate::pact_file::tests::saved_fixture(20);
    let component_id = &fixture.component_id;
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(&fixture.path, Arc::clone(&counters)).expect("snapshot");

    let raw = snapshot
        .raw_component_facet("main", component_id)
        .expect("raw component facet");
    assert_eq!(
        raw.len(),
        crate::pact_file::segment::RecordLocator::ENCODED_BYTES
    );
    assert!(
        !raw.windows(b"componentType".len())
            .any(|window| window == b"componentType")
    );
    let component = snapshot
        .component("main", component_id)
        .expect("component")
        .expect("present component");
    assert_eq!(component.value["id"], component_id.as_str());
    let secondary = snapshot
        .index_range(
            &crate::pact_file::index::IndexKey::Type {
                branch: "main".to_string(),
                component_type: "text_content".to_string(),
                component: String::new(),
            },
            &crate::pact_file::index::IndexKey::Type {
                branch: "main".to_string(),
                component_type: "text_content".to_string(),
                component: "\u{10ffff}".to_string(),
            },
            100,
            false,
        )
        .expect("type facet")
        .into_iter()
        .find(|(key, _)| {
            matches!(
                crate::pact_file::index::IndexKey::decode(key),
                Ok(crate::pact_file::index::IndexKey::Type { component, .. })
                    if component == *component_id
            )
        })
        .expect("component type facet");
    assert_eq!(
        secondary.1.len(),
        crate::pact_file::segment::RecordLocator::ENCODED_BYTES
    );
    assert!(counters.snapshot().records_decoded <= 1);
}

#[test]
#[ignore = "requires PACTREE_LENSKEEPER_ARTIFACT evidence path"]
fn lenskeeper_tool_result_index_values_decode_as_structured_json() {
    let path = std::path::PathBuf::from(
        std::env::var("PACTREE_LENSKEEPER_ARTIFACT").expect("artifact path"),
    );
    let snapshot = Snapshot::open(&path, Arc::new(IoCounterSet::default())).expect("snapshot");
    let entries = snapshot
        .index_range(
            &crate::pact_file::index::IndexKey::Type {
                branch: "main".to_string(),
                component_type: "tool_result".to_string(),
                component: String::new(),
            },
            &crate::pact_file::index::IndexKey::Type {
                branch: "main".to_string(),
                component_type: "tool_result".to_string(),
                component: "\u{10ffff}".to_string(),
            },
            10_000,
            false,
        )
        .expect("tool result ids");
    assert_eq!(entries.len(), 705);
    for (key, _) in entries {
        let crate::pact_file::index::IndexKey::Type { component, .. } =
            crate::pact_file::index::IndexKey::decode(&key).expect("type key")
        else {
            panic!("unexpected key");
        };
        if let Err(error) = snapshot.component("main", &component) {
            let raw = crate::pact_file::index::read_index_value_from_pages(
                &snapshot,
                snapshot.index_root,
                &crate::pact_file::index::IndexKey::Component {
                    branch: "main".to_string(),
                    component: component.clone(),
                },
            )
            .expect("raw component")
            .expect("component entry");
            let decoded = if let Some(payload) = raw.strip_prefix(b"PIC2") {
                let raw_len = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize;
                zstd::bulk::decompress(&payload[4..], raw_len).expect("decompress component")
            } else {
                raw
            };
            panic!(
                "component {component}: {error}; raw={}",
                String::from_utf8_lossy(&decoded)
            );
        }
    }
}

#[test]
fn source_file_identity_differs_from_root_and_survives_append_and_compaction() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("identity.pact");
    let backend = CandidateBackend::default();
    backend
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");
    let counters = Arc::new(IoCounterSet::default());
    let snapshot = Snapshot::open(&path, Arc::clone(&counters)).expect("open");
    let identity = snapshot.source_file_identity();
    assert_eq!(identity.len(), 64, "identity must be 32 bytes hex-encoded");
    assert!(identity.chars().all(|c| c.is_ascii_hexdigit()));

    let fixture = crate::pact_file::tests::saved_fixture(1);
    let metadata_snapshot = Snapshot::open(&fixture.path, Arc::new(IoCounterSet::default()))
        .expect("metadata snapshot");
    let metadata = metadata_snapshot.metadata().expect("metadata");
    assert_ne!(
        metadata_snapshot.source_file_identity(),
        metadata.root_id,
        "file identity must differ from root_id"
    );
    assert_eq!(
        metadata.source_file_identity,
        metadata_snapshot.source_file_identity()
    );

    for rev in 1..20 {
        backend
            .commit(
                &path,
                &transaction(&format!("tx_{}", rev + 1), rev, rev + 1),
            )
            .expect("commit append");
    }
    let appended = Snapshot::open(&path, Arc::clone(&counters)).expect("open appended");
    assert_eq!(
        appended.source_file_identity(),
        identity,
        "identity must survive append"
    );

    let compact_path = temp.path().join("identity-compaction.pact");
    let mut tree = crate::ContextTree::with_id("identity_compaction_root").expect("tree");
    tree.save_file(&compact_path).expect("save");
    let before_compaction = Snapshot::open(&compact_path, Arc::new(IoCounterSet::default()))
        .expect("before compaction");
    let compact_identity = before_compaction.source_file_identity();
    for revision in 1..=20 {
        tree.insert(
            crate::Component::from_component_value(&serde_json::json!({
                "id": format!("identity_{revision}"),
                "componentType": "text_content",
                "content": format!("identity payload {revision}: {}", "repeat ".repeat(32)),
            }))
            .expect("component"),
            "root",
        )
        .expect("insert");
        tree.send(&crate::ProviderThreadOptions::default())
            .expect("commit");
    }
    let compacted =
        Snapshot::open(&compact_path, Arc::new(IoCounterSet::default())).expect("compacted");
    assert!(
        compacted.generation() <= 16,
        "maintenance must have compacted"
    );
    assert_eq!(compacted.source_file_identity(), compact_identity);
}

#[test]
fn missing_candidate_identity_is_derived_then_persisted_on_append() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("candidate-without-identity.pact");
    let backend = CandidateBackend::default();
    backend
        .create(&path, &transaction("tx_1", 0, 1))
        .expect("create");

    let mut bytes = std::fs::read(&path).expect("read");
    let start = crate::pact_file::layout::FILE_IDENTITY_START;
    let end = start + crate::pact_file::layout::FILE_IDENTITY_BYTES;
    bytes[start..end].fill(0);
    std::fs::write(&path, bytes).expect("write identity-free candidate");

    let derived = Snapshot::open(&path, Arc::new(IoCounterSet::default()))
        .expect("open candidate")
        .source_file_identity();
    backend
        .commit(&path, &transaction("tx_2", 1, 2))
        .expect("append");

    let persisted_bytes = std::fs::read(&path).expect("read persisted");
    assert!(persisted_bytes[start..end].iter().any(|byte| *byte != 0));
    assert_eq!(
        Snapshot::open(&path, Arc::new(IoCounterSet::default()))
            .expect("reopen")
            .source_file_identity(),
        derived
    );
}
