use super::inspect::symbol_info;
#[cfg(feature = "compression-zstd")]
use super::inspect::{expanded_record_jsonl_bytes, gzip_len, semantic_archive_zstd_bytes};
use super::validation::validate_semantic_records;
use super::*;
use crate::store::{
    content_store_put_inline_json, ledger_create_branch_at_revision_json, new_ledger_json,
    new_ledger_json_without_base,
};
use crate::tree::ComponentTree;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "compression-zstd")]
const DEFAULT_CHUNK_BYTES: usize = 16 * 1024;

pub(super) fn sample_root_store() -> String {
    let tree = r#"{
          "id": "root",
          "componentType": "root",
          "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "attrs":{"cycle":1},
            "children": [{
              "id":"text_1",
              "componentType":"text_content",
              "type":"text_content",
              "attrs":{"cycle":1},
              "content":"hello from a compact pact file"
            }, {
              "id":"tool_call",
              "componentType":"tool_call",
              "type":"tool_call",
              "tool_call_id":"toolu_1",
              "attrs":{"cycle":1,"aux":{"app.activity":{"kind":"querying"}}}
            }, {
              "id":"tool_result",
              "componentType":"tool_result",
              "type":"tool_result",
              "attrs":{"cycle":1},
              "tool_call_id":"toolu_1"
            }]
          }]
        }"#;
    let root = new_ledger_json(tree, Some("main"), 8).expect("root store");
    append_text_2_to_turn_1(&root)
}



fn append_text_2_to_turn_1(root_json: &str) -> String {
    use crate::components::Component;
    use crate::ops::{PactOperation, PactOperationEnvelope, SourceActor};
    use crate::resolver::PactResolver;
    use crate::trace::BoundaryKind;

    let ledger = crate::store::decode_context_tree_ledger(root_json).expect("decode root store");
    let component = Component::from_component_value(&serde_json::json!({
        "id": "text_2",
        "componentType": "text_content",
        "type": "text_content",
        "content": "later",
    }))
    .expect("text_2 component");
    let op = PactOperation::Append {
        envelope: PactOperationEnvelope {
            branch_id: "main".to_string(),
            source: SourceActor::Host,
            source_id: Some("file_format_test".to_string()),
            host_timestamp_ns: None,
            idempotency_key: None,
            aux: std::collections::BTreeMap::new(),
        },
        target_component_id: Some("turn_1".to_string()),
        target_selector: None,
        offset: None,
        component,
    };
    let mut resolver = PactResolver::from_ledger(ledger);
    let outcome = resolver
        .resolve(vec![op], "main", BoundaryKind::Authored)
        .expect("resolve append");
    let candidate = outcome.commit_candidate.expect("commit candidate");
    resolver.seal(&candidate).expect("seal append");
    serde_json::to_string(&resolver.into_ledger()).expect("encode ledger")
}

fn medium_agentic_root_store() -> String {
    let turns = (0..20)
        .map(|index| {
            json!({
                "id": format!("turn_{index}"),
                "componentType": "message_turn",
                "type": "message_turn",
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "pact_depth": index + 1,
                "attrs": {"cycle": index},
                "children": [{
                    "id": format!("text_{index}"),
                    "componentType": "text_content",
                    "type": "text_content",
                    "content": format!("agentic message {index}: {}", "working context ".repeat(12))
                }, {
                    "id": format!("call_{index}"),
                    "componentType": "tool_call",
                    "type": "tool_call",
                    "tool_call_id": format!("toolu_{index}"),
                    "content": "{\"command\":\"pwd\",\"limit\":2}",
                    "attrs": {
                        "tool_call_id": format!("toolu_{index}"),
                        "tool_name": "generic",
                        "parameters": {"command": "pwd", "limit": 2}
                    }
                }, {
                    "id": format!("result_{index}"),
                    "componentType": "tool_result",
                    "type": "tool_result",
                    "tool_call_id": format!("toolu_{index}"),
                    "content": "{\"stdout\":\"/tmp/project\",\"exit_code\":0}",
                    "attrs": {
                        "tool_call_id": format!("toolu_{index}"),
                        "tool_name": "generic",
                        "success": true,
                        "result": {"stdout": "/tmp/project", "exit_code": 0}
                    }
                }]
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": turns,
    });
    new_ledger_json(&tree.to_string(), Some("main"), 96).expect("medium root store")
}

fn sample_root_store_with_trace() -> String {
    let mut root: Value = serde_json::from_str(&sample_root_store()).expect("root json");
    root["event_index"]["transaction_traces"]["tx_info"] = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_info",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":2},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":3},
        "request_ref": {"root_id":"root","branch_id":"main","revision":4},
        "alteration_ids": ["a1"],
        "aux": {
            "latency_ms": 12.5,
            "note": "trace info",
            "events": [
                {"op_kind": "activity_event", "aux": {"event_type": "notification", "message": "queued"}}
            ]
        }
    });
    root.to_string()
}

pub(super) fn temp_path(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}_{nanos}.pact"))
}

#[derive(Debug)]
struct MemoryProbe {
    baseline_rss_bytes: u64,
    peak_rss_bytes: u64,
    delta_rss_bytes: u64,
}

fn current_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        let kb = value.split_whitespace().next()?.parse::<u64>().ok()?;
        Some(kb * 1024)
    })
}

fn run_with_rss_probe<T>(operation: impl FnOnce() -> T) -> (T, Option<MemoryProbe>) {
    let baseline = current_rss_bytes();
    let output = operation();
    let after = current_rss_bytes();
    let probe = baseline.zip(after).map(|(baseline_rss_bytes, after)| {
        let peak_rss_bytes = baseline_rss_bytes.max(after);
        MemoryProbe {
            baseline_rss_bytes,
            peak_rss_bytes,
            delta_rss_bytes: peak_rss_bytes.saturating_sub(baseline_rss_bytes),
        }
    });
    (output, probe)
}

fn assert_rss_delta_under(label: &str, probe: Option<MemoryProbe>, budget_bytes: u64) {
    if let Some(probe) = probe {
        assert!(
            probe.delta_rss_bytes <= budget_bytes,
            "{label} RSS delta exceeded budget: baseline={} peak={} delta={} budget={}",
            probe.baseline_rss_bytes,
            probe.peak_rss_bytes,
            probe.delta_rss_bytes,
            budget_bytes
        );
    } else {
        eprintln!("RSS probe unavailable for {label}; behavioral assertion still ran");
    }
}

fn rss_peak_and_delta(probe: &Option<MemoryProbe>) -> (u64, u64) {
    probe
        .as_ref()
        .map(|probe| (probe.peak_rss_bytes, probe.delta_rss_bytes))
        .unwrap_or((0, 0))
}

fn run_rss_child_test(test_name: &str) {
    let output = std::process::Command::new(std::env::current_exe().expect("current test binary"))
        .arg("--exact")
        .arg(test_name)
        .arg("--ignored")
        .arg("--nocapture")
        .env("PACTREE_RSS_CHILD", "1")
        .env("RUST_TEST_THREADS", "1")
        .output()
        .expect("run RSS child test");
    assert!(
        output.status.success(),
        "RSS child test {test_name} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn should_run_rss_child() -> bool {
    std::env::var("PACTREE_RSS_CHILD").as_deref() == Ok("1")
}

fn write_commit_fixture(path: &Path) {
    let root = sample_root_store();
    file_create_from_ledger_json(path, &root, 8).expect("create commit fixture");
}

fn find_node_by_id<'a>(node: &'a Value, component_id: &str) -> Option<&'a Value> {
    if node.get("id").and_then(Value::as_str) == Some(component_id) {
        return Some(node);
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(found) = find_node_by_id(child, component_id) {
            return Some(found);
        }
    }
    None
}

fn insert_text_commit(
    cycle: u64,
    revision: u64,
    component_id: &str,
    content: &str,
) -> (String, String) {
    let node = json!({
        "id": component_id,
        "componentType": "text_content",
        "type": "text_content",
        "offset": revision as i64,
        "content": content
    });
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle,
        revision,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: component_id.to_string(),
            position: Some(revision as i64),
        }],
    };
    (
        serde_json::to_string(&commit).expect("commit json"),
        serde_json::to_string(&vec![node]).expect("nodes json"),
    )
}

#[test]
fn test_commit_record_roundtrip() {
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 7,
        revision: 11,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::ReplaceContentRange {
            component_id: "n1".to_string(),
            range: PactFileRange { start: 1, end: 3 },
            blob_id: "blob_1".to_string(),
        }],
    };

    let encoded = serde_json::to_string(&commit).expect("encode commit");
    let decoded: CommitRecord = serde_json::from_str(&encoded).expect("decode commit");

    assert_eq!(decoded, commit);
    assert!(!encoded.contains("host_payload_body"));
    assert!(!encoded.contains("rendered_host_view"));
}

#[test]
fn compact_commit_ops_round_trip_to_public_json() {
    let commit = json!({
        "schema": COMMIT_RECORD_SCHEMA,
        "branch_id": "main",
        "cycle": 2,
        "revision": 2,
        "ops": [
            {"op":"insert_component","parent_id":"root","node_ref":"n1","position":0},
            {"op":"delete_component","component_id":"old"},
            {"op":"attach_content_ref","component_id":"n1","blob_id":"cnt_1"}
        ]
    });
    let compact = compact_canonical_record_payload(RecordKind::Commit, commit.clone());
    let compact_items = compact.as_array().expect("compact commit array");
    let compact_ops = compact_items[4].as_array().expect("compact ops");
    assert!(compact_ops[0].is_array());
    assert_eq!(compact_ops[0][0], 1);
    assert!(!compact_ops[0].to_string().contains("insert_component"));

    let expanded = expand_canonical_record_payload("commit", compact).expect("expand commit");
    let decoded: CommitRecord = serde_json::from_value(expanded).expect("expanded compact commit");
    let expected: CommitRecord = serde_json::from_value(commit).expect("expected commit");
    assert_eq!(decoded, expected);
}

#[test]
fn appending_trace_requires_referenced_content_records_first() {
    let path = temp_path("appending_trace_requires_referenced_content_records_first");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_missing_content",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":1},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":2},
        "alteration_ids": ["insert_missing_content"],
        "replay_trace": {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "text_missing",
                "op": "set_content",
                "content_ref": "cnt_missing_trace_payload"
            }]
        },
        "aux": {}
    });

    let err = file_append_trace_record_json(&path, &trace.to_string())
        .expect_err("trace append should require content record first");
    assert!(
        err.to_string().contains("cnt_missing_trace_payload"),
        "missing content id should be reported: {err}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn test_commit_record_small_append() {
    let path = temp_path("test_commit_record_small_append");
    write_commit_fixture(&path);
    let before = fs::read(&path).expect("before bytes");
    let (commit, nodes) = insert_text_commit(1, 1, "small_append", "small");

    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let after = fs::read(&path).expect("after bytes");

    assert!(after.len() > before.len());
    assert!(
        after.len() - before.len() <= 1024,
        "append grew by {} bytes",
        after.len() - before.len()
    );
    let _ = fs::remove_file(path);
}

#[test]
fn commit_records_hydrate_transaction_traces_for_patchless_projection() {
    let path = temp_path("commit_records_hydrate_transaction_traces");
    let root = new_ledger_json(
        &json!({
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "turn_1",
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "user",
                "children": []
            }]
        })
        .to_string(),
        Some("main"),
        8,
    )
    .expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let (commit, nodes) = insert_text_commit(1, 1, "trace_from_commit", "trace backed");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    let mut root = ledger_from_records(&scanned.records).expect("ledger from records");
    let branch = root.branches.get("main").expect("main branch").clone();
    assert!(
        !branch.transaction_traces.is_empty(),
        "commit records must hydrate replayable transaction traces"
    );
    assert!(branch.transaction_traces.iter().any(|trace| {
        trace.ops.iter().any(|op| {
            op.component_id == "trace_from_commit"
                && matches!(
                    op.kind,
                    crate::transaction_trace::TraceOpKind::InsertComponent { .. }
                )
        })
    }));

    root.branches.insert("main".to_string(), branch.clone());
    let projected =
        crate::store::project_branch_component_tree(&root, &branch, Some(branch.head_revision))
            .expect("trace projection")
            .project_value();
    let node = find_node_by_id(&projected, "trace_from_commit").expect("trace node");
    assert_eq!(
        node.get("content").and_then(Value::as_str),
        Some("trace backed")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn test_commit_record_count_matches_appends() {
    let path = temp_path("test_commit_record_count_matches_appends");
    write_commit_fixture(&path);

    for index in 0..100 {
        let (commit, nodes) = insert_text_commit(
            index + 1,
            index + 1,
            &format!("node_{index}"),
            &format!("content {index}"),
        );
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    let bytes = fs::read(&path).expect("read");
    let scanned = scan_file(&bytes, true);
    assert_eq!(scanned.manifest.commit_count, 100);
    assert_eq!(
        scanned
            .records
            .iter()
            .filter(|record| record.kind == "commit")
            .count(),
        100
    );
    assert_eq!(scanned.manifest.root_count, 1);
    let _ = fs::remove_file(path);
}

#[test]
fn live_commit_writes_commit_record_not_branch_snapshot() {
    let path = temp_path("live_commit_writes_commit_record_not_branch_snapshot");
    write_commit_fixture(&path);
    let before = scan_file(&fs::read(&path).expect("read before"), true);
    let (commit, nodes) = insert_text_commit(3, 3, "append_only_node", "append only");

    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let after = scan_file(&fs::read(&path).expect("read after"), true);

    assert_eq!(after.manifest.branch_count, before.manifest.branch_count);
    assert_eq!(
        after
            .records
            .iter()
            .filter(|record| record.kind == "branch")
            .count(),
        before
            .records
            .iter()
            .filter(|record| record.kind == "branch")
            .count()
    );
    assert_eq!(
        after.manifest.commit_count,
        before.manifest.commit_count + 1
    );
    assert_eq!(after.manifest.head_count, before.manifest.head_count + 1);

    let _ = fs::remove_file(path);
}

#[test]
fn live_commit_content_refs_use_chunk_level_dedupe() {
    let path = temp_path("live_commit_content_refs_use_chunk_level_dedupe");
    write_commit_fixture(&path);

    let shared_a = "alpha\n".repeat(400);
    let shared_b = "beta\n".repeat(400);
    let first_body = format!("{shared_a}{shared_b}first tail");
    let second_body = format!("{shared_a}{shared_b}second tail");
    let mut content = ContentStore::default();
    let first = content.put_inline("node.content", &first_body, 64);
    let second = content.put_inline("node.content", &second_body, 64);
    let nodes = serde_json::to_string(&vec![
        json!({
            "id": "chunked_first",
            "componentType": "text_content",
            "type": "text_content",
            "offset": 1,
            "content_ref": first.id,
        }),
        json!({
            "id": "chunked_second",
            "componentType": "text_content",
            "type": "text_content",
            "offset": 2,
            "content_ref": second.id,
        }),
    ])
    .expect("nodes json");
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 2,
        revision: 2,
        lifecycle_tick: None,
        ops: vec![
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "chunked_first".to_string(),
                position: Some(1),
            },
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "chunked_second".to_string(),
                position: Some(2),
            },
        ],
    };

    file_append_commit_with_content_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &nodes,
        &serde_json::to_string(&content).expect("content json"),
    )
    .expect("append commit with content");

    let info: Value =
        serde_json::from_str(&file_info_json(&path).expect("info")).expect("info json");
    assert!(info["content"]["chunk_count"].as_u64().unwrap_or(0) > 0);
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    let ref_chunks = scanned
        .records
        .iter()
        .filter(|record| {
            record.kind == "content_chunk"
                && record.payload.get("codec").and_then(Value::as_str) == Some("ref")
        })
        .count();
    assert!(
        ref_chunks > 0,
        "expected near-duplicate content to reuse chunk refs"
    );
    content_chunks_from_records(&scanned.records).expect("content chunks decode");
    let payload: Value =
        serde_json::from_str(&file_content_payload_json(&path, &first.id).expect("payload"))
            .expect("payload json");
    assert_eq!(payload["text"], first_body);

    let _ = fs::remove_file(path);
}

#[test]
fn live_node_content_uses_chunk_level_dedupe() {
    let shared = (0..700)
        .map(|index| format!("shared shell line {index}\n"))
        .collect::<String>();
    let first_body = format!(
        "<shell session=\"abc\" history_count=\"8\"><shell_history>\n{shared}</shell_history><current_output>first</current_output></shell>"
    );
    let second_body = format!(
        "<shell session=\"abc\" history_count=\"9\"><shell_history>\n{shared}</shell_history><current_output>second</current_output></shell>"
    );
    let mut content = ContentCollector::with_compression();

    let first_id = content
        .put_text("node.content", "shell_first", &first_body)
        .expect("first content");
    let second_id = content
        .put_text("node.content", "shell_second", &second_body)
        .expect("second content");

    assert_ne!(first_id, second_id);
    assert!(
        content
            .by_hash
            .values()
            .any(|payload| payload.id == first_id && payload.codec == "chunked")
    );
    assert!(
        content
            .by_hash
            .values()
            .any(|payload| payload.id == second_id && payload.codec == "chunked")
    );
    assert!(
        content
            .chunks
            .iter()
            .any(|chunk| chunk.codec == "ref" && chunk.content_id == second_id),
        "expected repeated live node content chunks to be referenced, not re-stored"
    );
}

#[test]
fn managed_tool_output_commits_generic_delete_and_insert_ops() {
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 9,
        revision: 9,
        lifecycle_tick: None,
        ops: vec![
            CommitRecordOp::DeleteComponent {
                component_id: "old_tool_output".to_string(),
            },
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "new_tool_output".to_string(),
                position: Some(1),
            },
        ],
    };
    let encoded = serde_json::to_string(&commit).expect("commit json");

    assert!(encoded.contains("delete_component"));
    assert!(encoded.contains("insert_component"));
}

#[test]
fn commit_records_rebuild_current_head() {
    let path = temp_path("commit_records_rebuild_current_head");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(4, 4, "rebuilt_head_node", "rebuilt");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let current: Value = serde_json::from_str(&file_project_head_json(&path).expect("current"))
        .expect("current json");
    assert!(find_node_by_id(&current, "rebuilt_head_node").is_some());

    let _ = fs::remove_file(path);
}

#[test]
fn file_repack_preserves_history_and_writes_ref_head_only() {
    let path = temp_path("file_repack_preserves_history_and_writes_ref_head_only");
    let repacked_path =
        temp_path("file_repack_preserves_history_and_writes_ref_head_only_repacked");
    write_commit_fixture(&path);
    let shared = (0..700)
        .map(|index| format!("shared live content line {index}\n"))
        .collect::<String>();
    let (first_commit, first_nodes) =
        insert_text_commit(2, 2, "large_live_node_a", &format!("{shared}first"));
    let (second_commit, second_nodes) =
        insert_text_commit(3, 3, "large_live_node_b", &format!("{shared}second"));
    file_append_commit_json(&path, &first_commit, &first_nodes).expect("append first");
    file_append_commit_json(&path, &second_commit, &second_nodes).expect("append second");
    let before_bytes = fs::read(&path).expect("read source before");
    let source_head = file_project_head_json(&path).expect("source head");

    let report_json = file_repack_json(&path, &repacked_path).expect("repack");
    let report: Value = serde_json::from_str(&report_json).expect("report json");
    let after_bytes = fs::read(&path).expect("read source after");
    let repacked = scan_file(&fs::read(&repacked_path).expect("read repacked"), true);

    assert_eq!(before_bytes, after_bytes);
    assert_eq!(report["commit_history_preserved"], true);
    assert_eq!(report["head_preserved"], true);
    assert_eq!(repacked.manifest.commit_count, 2);
    assert_eq!(repacked.manifest.head_count, 1);
    assert!(repacked.manifest.index_count >= 1);
    assert_eq!(
        source_head,
        file_project_head_json(&repacked_path).expect("repacked head")
    );

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(repacked_path);
}

#[test]
fn repair_renumbers_non_monotonic_live_commits_without_flattening_history() {
    let path = temp_path("repair_renumbers_non_monotonic_live_commits");
    let repaired_path = temp_path("repair_renumbers_non_monotonic_live_commits_repaired");
    write_commit_fixture(&path);

    let (unrelated_commit, unrelated_nodes) =
        insert_text_commit(1, 1, "unrelated_live_node", "one");
    file_append_commit_json(&path, &unrelated_commit, &unrelated_nodes).expect("append unrelated");

    let parent = json!({
        "id": "late_parent",
        "componentType": "message_turn",
        "type": "message_turn",
        "offset": 2,
        "parent_id": "turn_1",
        "children": []
    });
    let parent_commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 2,
        revision: 2,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: "late_parent".to_string(),
            position: Some(2),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&parent_commit).expect("parent commit"),
        &serde_json::to_string(&vec![parent]).expect("parent nodes"),
    )
    .expect("append parent");

    let child = json!({
        "id": "late_child",
        "componentType": "text_content",
        "type": "text_content",
        "offset": 0,
        "parent_id": "late_parent",
        "content": "child"
    });
    let child_commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 3,
        revision: 3,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "late_parent".to_string(),
            node_ref: "late_child".to_string(),
            position: Some(0),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&child_commit).expect("child commit"),
        &serde_json::to_string(&vec![child.clone()]).expect("child nodes"),
    )
    .expect("append child");

    let bad_late_commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::ReplaceComponent {
            component_id: "late_child".to_string(),
            node_ref: "late_child".to_string(),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&bad_late_commit).expect("bad late commit"),
        &serde_json::to_string(&vec![child]).expect("bad late nodes"),
    )
    .expect("append bad late commit");

    assert!(file_project_head_json(&path).is_err());

    let repair_json = file_repair_json(&path, &repaired_path, false).expect("repair");
    let repair: PactFileRepairReport = serde_json::from_str(&repair_json).expect("repair json");
    assert!(repair.repaired_validation.ok);
    assert_eq!(repair.repaired_validation.manifest.commit_count, 4);
    assert_eq!(repair.repaired_validation.manifest.head_count, 1);
    let current: Value =
        serde_json::from_str(&file_project_head_json(&repaired_path).expect("current"))
            .expect("current json");
    assert!(find_node_by_id(&current, "late_child").is_some());

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(repaired_path);
}

#[test]
fn repair_failure_never_replaces_valid_input() {
    let source = temp_path("repair_failure_corrupt_source");
    let destination = temp_path("repair_failure_valid_destination");
    fs::write(&source, b"not a Pact file").expect("write corrupt source");
    write_commit_fixture(&destination);
    let before = fs::read(&destination).expect("read valid destination");

    file_repair_json(&source, &destination, false).expect_err("repair must fail");

    assert_eq!(
        fs::read(&destination).expect("read preserved destination"),
        before
    );
    assert!(
        serde_json::from_str::<PactFileValidation>(
            &file_validate_json(&destination).expect("validate")
        )
        .expect("validation JSON")
        .ok
    );
    fs::remove_file(source).ok();
    fs::remove_file(destination).ok();
}

#[test]
fn pact_info_reports_live_shape() {
    let path = temp_path("pact_info_reports_live_shape");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(2, 2, "live_shape_node", "shape");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");
    let live_shape = &info["live_shape"];

    assert_eq!(live_shape["commit_record_count"].as_u64(), Some(1));
    assert_eq!(live_shape["head_record_count"].as_u64(), Some(1));
    assert!(
        live_shape["shape"]
            .as_str()
            .is_some_and(|shape| shape.contains("commit-head"))
    );
    assert!(live_shape["trace_decoded_bytes"].is_number());
    assert!(live_shape["content_metadata_bytes"].is_number());
    assert_eq!(
        live_shape["anchor_policy"]["default_freeze_cycle_interval"].as_u64(),
        Some(DEFAULT_FREEZE_CYCLE_INTERVAL)
    );

    let _ = fs::remove_file(path);
}

#[test]
fn live_fixture_size_baseline_is_recorded() {
    let path = temp_path("live_fixture_size_baseline_is_recorded");
    let root = sample_root_store_with_trace();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let content_store_json =
        content_store_put_inline_json(None, "tool.output", &"large output ".repeat(512), 48)
            .expect("content store");
    let content_store: ContentStore =
        serde_json::from_str(&content_store_json).expect("content store json");
    let content_id = content_store
        .records
        .keys()
        .next()
        .expect("content id")
        .clone();
    let node = json!({
        "id": "large_tool_result",
        "componentType": "tool_result",
        "type": "tool_result",
        "tool_call_id": "toolu_large",
        "content_ref": content_id,
        "attrs": {"tool_name": "generic", "success": true}
    });
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 2,
        revision: 2,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: "large_tool_result".to_string(),
            position: Some(2),
        }],
    };
    file_append_commit_with_content_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &serde_json::to_string(&vec![node]).expect("nodes json"),
        &content_store_json,
    )
    .expect("append commit with content");

    let info: Value = serde_json::from_str(&file_live_shape_info_json(&path).expect("live shape"))
        .expect("live shape json");
    let raw_jsonl = json!([
        {"role":"user","content":"hello from a compact pact file"},
        {"role":"assistant","tool_calls":[{"id":"toolu_large","name":"generic"}]},
        {"role":"tool","tool_call_id":"toolu_large","content":"large output ".repeat(512)}
    ])
    .to_string()
    .len();
    let file_bytes = fs::metadata(&path).expect("metadata").len() as usize;

    assert!(raw_jsonl > 0);
    assert!(file_bytes > 0);
    assert!(info["live_shape"]["max_provider_visible_head_bytes"].is_number());
    assert!(
        info["live_shape"]["content_body_record_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );

    let _ = fs::remove_file(path);
}

#[test]
fn commit_node_content_ref_projects_without_hot_inline_body() {
    let path = temp_path("commit_node_content_ref_projects_without_hot_inline_body");
    write_commit_fixture(&path);
    let large = "provider visible body ".repeat(256);
    let content_store_json =
        content_store_put_inline_json(None, "node.content", &large, 32).expect("content store");
    let content_store: ContentStore =
        serde_json::from_str(&content_store_json).expect("content store json");
    let content_id = content_store
        .records
        .keys()
        .next()
        .expect("content id")
        .clone();
    let node = json!({
        "id": "large_text_ref",
        "componentType": "text_content",
        "type": "text_content",
        "content_ref": content_id
    });
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 3,
        revision: 3,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: "large_text_ref".to_string(),
            position: Some(3),
        }],
    };

    file_append_commit_with_content_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &serde_json::to_string(&vec![node]).expect("nodes json"),
        &content_store_json,
    )
    .expect("append content-ref commit");

    let scanned = scan_file(&fs::read(&path).expect("read pact"), true);
    let stored_node = scanned
        .records
        .iter()
        .find(|record| {
            record.kind == "node"
                && record.payload.get("id").and_then(Value::as_str) == Some("large_text_ref")
        })
        .expect("stored node");
    assert_eq!(
        stored_node.payload.get("content").and_then(Value::as_str),
        None
    );
    assert_eq!(
        ref_id_from_content_ref_value(stored_node.payload.get("content_ref")).as_deref(),
        Some(content_id.as_str())
    );
    assert!(validate_semantic_records(&scanned.records).is_empty());

    let current: Value = serde_json::from_str(&file_project_head_json(&path).expect("current"))
        .expect("current json");
    let hydrated = find_node_by_id(&current, "large_text_ref").expect("hydrated node");
    assert_eq!(
        hydrated.get("content").and_then(Value::as_str),
        Some(large.as_str())
    );
    assert!(hydrated.get("content_ref").is_none());
    assert!(hydrated.get("content_preview").is_none());

    let _ = fs::remove_file(path);
}

#[test]
fn test_file_create_does_not_write_default_frozen_anchor() {
    let path = temp_path("test_file_create_does_not_write_default_frozen_anchor");
    write_commit_fixture(&path);

    let scanned = scan_file(&fs::read(&path).expect("read"), true);

    assert_eq!(scanned.manifest.frozen_cycle_count, 0);
    assert!(!scanned.records.iter().any(|record| {
        record.kind == "content"
            && record.payload.get("kind").and_then(Value::as_str) == Some("frozen.root")
    }));
    let _ = fs::remove_file(path);
}

#[test]
fn test_file_create_omits_rendered_self_xml_from_hot_records() {
    let path = temp_path("test_file_create_omits_rendered_self_xml_from_hot_records");
    let tree = r#"{
          "id": "root",
          "componentType": "root",
          "children": [{
            "id": "panel_1",
            "componentType": "panel",
            "type": "panel",
            "content": "<panel secret=\"yes\">rendered body</panel>",
            "org": {"text": "<panel secret=\"yes\">rendered body</panel>"}
          }, {
            "id": "text_1",
            "componentType": "text_content",
            "type": "text_content",
            "content": "ordinary text survives"
          }]
        }"#;
    let root = new_ledger_json(tree, Some("main"), 8).expect("root store");

    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let bytes = fs::read(&path).expect("read");
    let file_text = String::from_utf8_lossy(&bytes);
    let scanned = scan_file(&bytes, true);
    let panel = file_query_json(&path, "node", Some("panel_1")).expect("query panel");
    let projected = file_project_tree_json(&path, Some("main"), None).expect("project pact");

    assert!(!file_text.contains("<panel secret="));
    assert!(!panel.contains("rendered body"));
    assert!(projected.contains("ordinary text survives"));
    assert_eq!(scanned.manifest.content_chunk_count, 0);
    let _ = fs::remove_file(path);
}

#[test]
fn packed_symbol_dictionary_round_trips_records() {
    let path = temp_path("packed_symbol_dictionary_round_trips_records");
    write_commit_fixture(&path);

    let scanned = scan_file(&fs::read(&path).expect("read"), true);

    assert_eq!(scanned.manifest.symbol_count, 1);
    let symbol_record = scanned
        .records
        .iter()
        .find(|record| record.kind == "symbol")
        .expect("packed symbol dictionary record");
    assert_eq!(symbol_record.payload["id"], "dict");
    assert!(
        symbol_record
            .payload
            .get("values")
            .and_then(Value::as_array)
            .map(|values| !values.is_empty())
            .unwrap_or(false)
    );
    assert!(file_query_json(&path, "node", Some("turn_1")).is_ok());
    let info = symbol_info(&scanned.records);
    assert_eq!(info["symbol_record_count"].as_u64().unwrap_or_default(), 1);
    assert!(info["dictionary_bytes"].as_u64().unwrap_or_default() > 0);
    assert!(
        info["substituted_occurrence_count"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    let _ = fs::remove_file(path);
}

#[test]
fn test_one_off_strings_do_not_create_dynamic_symbol_records() {
    let path = temp_path("test_one_off_strings_do_not_create_dynamic_symbol_records");
    let unique_value = "one-off-symbol-scream-test-value";
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "message_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "attrs": {
                "unique_marker": unique_value
            },
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "hello"
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(
        scanned
            .records
            .iter()
            .filter(|record| record.kind == "symbol")
            .all(|record| !record
                .payload
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values
                    .iter()
                    .any(|value| value.as_str() == Some(unique_value)))
                .unwrap_or(false))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn one_off_strings_do_not_enter_dynamic_symbols() {
    let path = temp_path("one_off_strings_do_not_enter_dynamic_symbols");
    let unique_value = "single-use-symbol-value";
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "message_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "attrs": {"unique_marker": unique_value},
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "hello"
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);

    assert!(
        scanned
            .records
            .iter()
            .filter(|record| record.kind == "symbol")
            .all(|record| !record
                .payload
                .get("values")
                .and_then(Value::as_array)
                .map(|values| values
                    .iter()
                    .any(|value| value.as_str() == Some(unique_value)))
                .unwrap_or(false))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn test_ref_node_omits_duplicate_content_preview() {
    let path = temp_path("test_ref_node_omits_duplicate_content_preview");
    let text = "large text ".repeat(64);
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text_1",
            "componentType": "text_content",
            "type": "text_content",
            "content": text
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");

    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    let node_json = file_query_json(&path, "node", Some("text_1")).expect("node query");
    let node_query: PactFileQueryResult =
        serde_json::from_str(&node_json).expect("node query json");
    let node = node_query.matches.first().expect("node match");

    assert!(node.get("content_ref").is_some());
    assert!(node.get("content_preview").is_none());
    assert!(scanned.records.iter().any(|record| {
        record.kind == "content"
            && record
                .payload
                .get("preview_inline")
                .and_then(Value::as_str)
                .is_none()
            && record.payload.get("preview").is_none()
    }));
    let branch = scanned
        .records
        .iter()
        .find(|record| record.kind == "branch")
        .expect("branch record");
    let branch_content_refs = branch
        .payload
        .get("current")
        .and_then(|current| current.get("content_refs"))
        .and_then(Value::as_object)
        .expect("branch content refs");
    assert!(
        branch_content_refs
            .values()
            .all(|summary| summary.get("preview").is_none())
    );
    let _ = fs::remove_file(path);
}

#[test]
fn no_base_root_store_file_projects_from_node_records() {
    let path = temp_path("no_base_root_store_file_projects_from_node_records");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "hello from compact import"
            }]
        }]
    });
    let root =
        new_ledger_json_without_base(&tree.to_string(), Some("main"), 8).expect("root store");

    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.records.iter().all(|record| {
        record.kind != "content"
            || record.payload.get("kind").and_then(Value::as_str) != Some("pact.branch.base")
    }));

    let root_store = ledger_from_records(&scanned.records).expect("root store from records");
    assert!(root_store.branches["main"].base_ref.content_ref.is_none());

    let projected = file_project_head_json(&path).expect("project current");
    let projected: Value = serde_json::from_str(&projected).expect("projected json");
    assert_eq!(
        projected["children"][0]["children"][0]["content"],
        "hello from compact import"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn file_create_omits_duplicate_import_base_and_reports_bytes() {
    let path = temp_path("file_create_omits_duplicate_import_base_and_reports_bytes");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text_1",
            "componentType": "text_content",
            "type": "text_content",
            "content": "duplicated import base"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");

    let report_json = file_create_from_ledger_json(&path, &root, 8).expect("create");
    let report: Value = serde_json::from_str(&report_json).expect("report json");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);

    assert!(
        report["slimming"]["omitted_duplicate_branch_base_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(scanned.records.iter().all(|record| {
        record.kind != "content"
            || record.payload.get("kind").and_then(Value::as_str) != Some("pact.branch.base")
    }));
    assert_eq!(
        serde_json::from_str::<Value>(&file_project_head_json(&path).expect("head"))
            .expect("head json")["children"][0]["content"],
        "duplicated import base"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn optional_preview_index_is_readable_by_inspector() {
    let path = temp_path("optional_preview_index_is_readable_by_inspector");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text_1",
            "componentType": "text_content",
            "type": "text_content",
            "content": "preview index content"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");

    file_create_from_ledger_with_previews_json(&path, &root, 7).expect("create");
    let info: Value =
        serde_json::from_str(&file_info_json(&path).expect("info")).expect("info json");

    assert!(
        info["content"]["previews"]
            .as_array()
            .expect("previews")
            .iter()
            .any(|preview| {
                preview.get("source").and_then(Value::as_str) == Some("content_preview_index")
                    && preview.get("preview").and_then(Value::as_str) == Some("preview")
            })
    );

    let _ = fs::remove_file(path);
}

#[test]
fn media_base64_imports_as_binary_backed_image_content() {
    let path = temp_path("media_base64_imports_as_binary_backed_image_content");
    let png_bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 1, 2, 3];
    let encoded = base64_encode(&png_bytes);
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active",
            "componentType": "active_message",
            "type": "message_turn",
            "role": "user",
            "children": [{
                "id": "img",
                "componentType": "image_content",
                "type": "image_content",
                "content": "fixture.png",
                "attrs": {
                    "media_content": {"data": encoded, "encoding": "base64"},
                    "mime_type": "image/png",
                    "filename": "fixture.png",
                    "width": 2,
                    "height": 2
                }
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");

    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let file_bytes = fs::read(&path).expect("read");
    let scanned = scan_file(&file_bytes, true);
    let node = file_query_json(&path, "node", Some("img")).expect("node");
    assert!(
        !node.contains("\"data\"") && !node.contains(&encoded),
        "hot node should not carry the base64 media wrapper"
    );
    assert!(scanned.records.iter().any(|record| {
        record.kind == "content"
            && record.payload.get("kind").and_then(Value::as_str) == Some("media.image")
            && record.payload.get("codec").and_then(Value::as_str) == Some("base64")
    }));

    let projected: Value = serde_json::from_str(&file_project_head_json(&path).expect("project"))
        .expect("projected json");
    assert_eq!(
        projected["children"][0]["children"][0]["aux"]["media_content"]["data"],
        encoded
    );
    assert_eq!(
        projected["children"][0]["children"][0]["aux"]["filename"],
        "fixture.png"
    );
    let provider_thread = crate::provider_thread::build_provider_thread(
        &projected,
        &crate::provider_thread::ProviderThreadOptions::default(),
    )
    .expect("provider thread");
    assert_eq!(
        provider_thread["messages"][0]["content"][0]["media_content"]["data"],
        encoded
    );

    let _ = fs::remove_file(path);
}

#[test]
fn text_address_index_resolves_exact_windows() {
    let path = temp_path("text_address_index_resolves_exact_windows");
    let text = "alpha\nbeta\ngamma\n";
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "result",
            "componentType": "tool_result",
            "type": "tool_result",
            "tool_call_id": "tc_text",
            "tool_name": "shell",
            "success": true,
            "content": text
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let addresses: PactFileQueryResult =
        serde_json::from_str(&file_query_json(&path, "text-address", None).expect("addresses"))
            .expect("addresses json");
    let entries = addresses.matches[0].as_object().expect("address entries");
    let address = entries
        .values()
        .find(|entry| entry["kind"] == "node.content")
        .expect("node content address");
    let content_id = address["content_id"].as_str().expect("content id");
    assert_eq!(address["byte_start"], 0);
    assert!(
        address["owner_refs"]
            .as_array()
            .expect("owners")
            .iter()
            .any(|owner| owner.as_str() == Some("node:result"))
    );

    let key = format!("{content_id}:6:10");
    let window: PactFileQueryResult =
        serde_json::from_str(&file_query_json(&path, "text-window", Some(&key)).expect("window"))
            .expect("window json");
    assert_eq!(window.matches[0]["text"], "beta");
    assert_eq!(window.matches[0]["byte_start"], 6);
    assert_eq!(window.matches[0]["byte_end"], 10);
    let bad_hash_key = format!("{content_id}:6:10:not_the_hash");
    let err = file_query_json(&path, "text-window", Some(&bad_hash_key))
        .expect_err("hash mismatch should reject");
    assert!(err.to_string().contains("decoded hash mismatch"));

    let _ = fs::remove_file(path);
}

#[test]
fn text_address_index_resolves_json_string_leaves() {
    let path = temp_path("text_address_index_resolves_json_string_leaves");
    let body = "shell output line\n".repeat(80);
    let params = json!({
        "cmd": "cat file.txt",
        "payload": {"body": body}
    });
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "call",
            "componentType": "tool_call",
            "type": "tool_call",
            "tool_call_id": "tc_json_leaf",
            "tool_name": "shell",
            "parameters": params
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 32).expect("root store");
    file_create_from_ledger_json(&path, &root, 32).expect("create");

    let addresses: PactFileQueryResult =
        serde_json::from_str(&file_query_json(&path, "text-address", None).expect("addresses"))
            .expect("addresses json");
    let entries = addresses.matches[0].as_object().expect("address entries");
    let leaf = entries
        .values()
        .find(|entry| entry["json_pointer"] == "/payload/body")
        .expect("json leaf address");
    let content_id = leaf["content_id"].as_str().expect("content id");
    let key = format!("{content_id}:/payload/body");
    let resolved: PactFileQueryResult =
        serde_json::from_str(&file_query_json(&path, "json-leaf", Some(&key)).expect("leaf"))
            .expect("leaf json");

    assert_eq!(resolved.matches[0]["text"], body);
    assert_eq!(resolved.matches[0]["json_pointer"], "/payload/body");

    let _ = fs::remove_file(path);
}

#[test]
fn test_hot_node_record_cannot_store_body_preview_and_content_ref() {
    let records = vec![
        ScannedRecord {
            kind: "content".to_string(),
            payload: json!({
                "id": "cnt_1",
                "kind": "node.content",
                "hash": "abc",
                "chunk_count": 0
            }),
            payload_bytes: 0,
            sequence: 0,
        },
        ScannedRecord {
            kind: "node".to_string(),
            payload: json!({
                "id": "text_1",
                "componentType": "text_content",
                "content": "duplicate hot body",
                "content_ref": "cnt_1",
                "content_preview": "duplicate preview"
            }),
            payload_bytes: 0,
            sequence: 0,
        },
    ];

    let errors = validate_semantic_records(&records);

    assert!(
        errors
            .iter()
            .any(|error| error.contains("stores both content and content_ref"))
    );
    assert!(
        errors
            .iter()
            .any(|error| error.contains("stores content_preview beside content_ref"))
    );
}

#[test]
fn content_ref_preview_is_stored_once() {
    let path = temp_path("content_ref_preview_is_stored_once");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");

    assert!(info["content"]["previews"].as_array().is_some());
    assert_eq!(
        info["payload_slots"]["oversized_inline_count"]
            .as_u64()
            .unwrap_or_default(),
        0
    );

    let _ = fs::remove_file(path);
}

#[test]
fn operation_nodes_do_not_duplicate_structured_body_as_content() {
    let path = temp_path("operation_nodes_do_not_duplicate_structured_body_as_content");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "call_1",
            "componentType": "tool_call",
            "tool_call_id": "toolu_1",
            "content": "{\"command\":\"pwd\",\"limit\":2}",
            "attrs": {
                "tool_call_id": "toolu_1",
                "tool_name": "shell",
                "parameters": {
                    "command": "pwd",
                    "limit": 2
                }
            }
        }, {
            "id": "result_1",
            "componentType": "tool_result",
            "tool_call_id": "toolu_1",
            "content": "{\"stdout\":\"/tmp\",\"exit_code\":0}",
            "attrs": {
                "tool_call_id": "toolu_1",
                "tool_name": "shell",
                "result": {
                    "stdout": "/tmp",
                    "exit_code": 0
                }
            }
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let call = file_query_json(&path, "node", Some("call_1")).expect("query call");
    let call: PactFileQueryResult = serde_json::from_str(&call).expect("call query json");
    let call_node = call.matches.first().expect("call node");
    assert!(call_node.get("content").is_none());
    assert_eq!(call_node["parameters"]["command"], "pwd");

    let result = file_query_json(&path, "node", Some("result_1")).expect("query result");
    let result: PactFileQueryResult = serde_json::from_str(&result).expect("result query json");
    let result_node = result.matches.first().expect("result node");
    assert!(result_node.get("content").is_none());
    assert_eq!(result_node["result"]["stdout"], "/tmp");

    let _ = fs::remove_file(path);
}

#[test]
fn hot_pact_rejects_rendered_view_payloads() {
    let derived_key = ["rendered", "xml"].join("_");
    let records = vec![ScannedRecord {
        kind: "node".to_string(),
        payload: json!({
            "id": "view_leak",
            "componentType": "text_content",
            "attrs": {
                derived_key.clone(): "<view>derived</view>"
            }
        }),
        payload_bytes: 0,
        sequence: 0,
    }];

    let errors = validate_semantic_records(&records);

    assert!(errors.iter().any(|error| {
        error.contains("forbidden derived/provider payload") && error.contains(&derived_key)
    }));
}

#[test]
fn hot_pact_rejects_provider_wire_snapshots() {
    let records = vec![ScannedRecord {
        kind: "trace".to_string(),
        payload: json!({
            "branch_id": "main",
            "transaction_id": "tx_wire",
            "provider_request": {"messages": []}
        }),
        payload_bytes: 0,
        sequence: 0,
    }];

    let errors = validate_semantic_records(&records);

    assert!(errors.iter().any(|error| {
        error.contains("forbidden derived/provider payload") && error.contains("provider_request")
    }));
}

#[test]
fn hot_trace_rejects_provider_wire_body() {
    let records = vec![ScannedRecord {
        kind: "trace".to_string(),
        payload: json!({
            "branch_id": "main",
            "transaction_id": "tx_wire_body",
            "provider_response": {"choices": []}
        }),
        payload_bytes: 0,
        sequence: 0,
    }];

    let errors = validate_semantic_records(&records);

    assert!(errors.iter().any(|error| {
        error.contains("forbidden derived/provider payload") && error.contains("provider_response")
    }));
}

#[test]
fn trace_record_size_independent_of_large_payload() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_large_ref",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":2},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":3},
        "request_ref": {
            "root_id":"root",
            "branch_id":"main",
            "revision":4,
            "content_ref": "cnt_large"
        },
        "aux": {
            "payload": {
                "content_ref": {
                    "id": "cnt_large",
                    "bytes": 100_000,
                    "source": {"kind": "generated_artifact", "hash": "blake3:abc"}
                }
            }
        }
    });

    let encoded = encode_record_payload(RecordKind::Trace, &trace).expect("encode trace");

    assert!(
        encoded.len() < 1024,
        "encoded trace was {} bytes",
        encoded.len()
    );
}

#[test]
fn trace_rejects_full_head_payload() {
    let records = vec![ScannedRecord {
        kind: "trace".to_string(),
        payload: json!({
            "branch_id": "main",
            "transaction_id": "tx_head",
            "full_head": {"children": [{"id": "n1"}]}
        }),
        payload_bytes: 0,
        sequence: 0,
    }];

    let errors = validate_semantic_records(&records);

    assert!(errors.iter().any(|error| {
        error.contains("forbidden derived/provider payload") && error.contains("full_head")
    }));
}

#[test]
fn structured_tool_payload_not_stored_as_duplicate_text() {
    let path = temp_path("structured_tool_payload_not_stored_as_duplicate_text");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "call_1",
            "componentType": "tool_call",
            "tool_call_id": "toolu_1",
            "content": "{\"query\":\"abc\"}",
            "attrs": {
                "tool_call_id": "toolu_1",
                "tool_name": "search",
                "parameters": {"query": "abc"}
            }
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let call = file_query_json(&path, "node", Some("call_1")).expect("query call");
    let call: PactFileQueryResult = serde_json::from_str(&call).expect("call query json");

    assert!(call.matches[0].get("content").is_none());
    assert_eq!(call.matches[0]["parameters"]["query"], "abc");

    let _ = fs::remove_file(path);
}

#[test]
fn canonical_tool_fields_do_not_store_in_attrs() {
    let path = temp_path("canonical_tool_fields_do_not_store_in_attrs");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "call_1",
            "componentType": "tool_call",
            "tool_call_id": "toolu_1",
            "tool_name": "search",
            "parameters": {"query": "abc"},
            "attrs": {
                "tool_call_id": "toolu_1",
                "tool_name": "search",
                "parameters": {"query": "abc"},
                "host_note": "kept"
            }
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let call = file_query_json(&path, "node", Some("call_1")).expect("query call");
    let call: PactFileQueryResult = serde_json::from_str(&call).expect("call query json");
    let node = &call.matches[0];

    assert_eq!(node["tool_call_id"], "toolu_1");
    assert_eq!(node["tool_name"], "search");
    assert_eq!(node["parameters"]["query"], "abc");
    assert!(node["aux"].get("tool_call_id").is_none());
    assert!(node["aux"].get("tool_name").is_none());
    assert!(node["aux"].get("parameters").is_none());
    assert_eq!(node["aux"]["host_note"], "kept");

    let _ = fs::remove_file(path);
}

#[test]
fn canonical_lifecycle_fields_do_not_store_in_attrs() {
    let path = temp_path("canonical_lifecycle_fields_do_not_store_in_attrs");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "note_1",
            "componentType": "text_content",
            "ttl": 2,
            "cadence": 3,
            "key": "notice",
            "tags": ["sticky", "important"],
            "attrs": {
                "ttl": 2,
                "cad": 3,
                "key": "notice",
                "tags": ["sticky", "important"],
                "host_note": "kept"
            }
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let note = file_query_json(&path, "node", Some("note_1")).expect("query note");
    let note: PactFileQueryResult = serde_json::from_str(&note).expect("note query json");
    let node = &note.matches[0];

    assert_eq!(node["ttl"], 2);
    assert_eq!(node["cad"], 3);
    assert_eq!(node["key"], "notice");
    assert_eq!(node["tags"][0], "sticky");
    assert!(node["aux"].get("ttl").is_none());
    assert!(node["aux"].get("cad").is_none());
    assert!(node["aux"].get("key").is_none());
    assert!(node["aux"].get("tags").is_none());
    assert_eq!(node["aux"]["host_note"], "kept");

    let by_key = file_query_json(&path, "attr", Some("key=notice")).expect("query key");
    let by_key: PactFileQueryResult = serde_json::from_str(&by_key).expect("key query json");
    assert_eq!(by_key.matches[0][0], "note_1");

    let by_tag = file_query_json(&path, "attr", Some("tag=important")).expect("query tag");
    let by_tag: PactFileQueryResult = serde_json::from_str(&by_tag).expect("tag query json");
    assert_eq!(by_tag.matches[0][0], "note_1");

    let _ = fs::remove_file(path);
}

#[test]
fn root_store_rejects_cadence_without_ttl() {
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "bad_lifecycle",
            "componentType": "text_content",
            "cad": 2,
            "content": "invalid"
        }]
    });
    let err = new_ledger_json(&tree.to_string(), Some("main"), 16)
        .expect_err("cad without ttl must be invalid");

    assert!(err.to_string().contains("cad without ttl"), "{err}");
}

#[test]
fn extension_attrs_still_round_trip() {
    let path = temp_path("extension_attrs_still_round_trip");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "custom_1",
            "componentType": "org:custom",
            "attrs": {
                "host_note": "small extension",
                "local_flags": ["a", "b"]
            }
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let custom = file_query_json(&path, "node", Some("custom_1")).expect("query custom");
    let custom: PactFileQueryResult = serde_json::from_str(&custom).expect("custom query json");
    let node = &custom.matches[0];

    assert_eq!(node["attrs"]["host_note"], "small extension");
    assert_eq!(node["attrs"]["local_flags"][1], "b");

    let _ = fs::remove_file(path);
}

#[test]
fn host_vocabulary_not_in_canonical_pact_fields() {
    assert_no_host_vocabulary_in_substrate_sources();
}

#[test]
fn pact_vocabulary_scream_tests() {
    assert_no_host_vocabulary_in_substrate_sources();
}

fn assert_no_host_vocabulary_in_substrate_sources() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = [
        "src/components.rs",
        "src/file_format.rs",
        "src/provider_thread.rs",
        "src/resolver.rs",
        "src/store.rs",
    ];
    let forbidden = [
        format!("{}{}", "sca", "ffold"),
        format!("{}{}", "egre", "gore"),
        format!("{}{}", "le", "ns"),
        format!("{}{}{}", "file", "_man", "ager"),
        format!("{}{}{}", "internal", "_no", "tes"),
        format!("{}{}", "rendered", "_xml"),
    ];

    let mut violations = Vec::new();
    for relative in files {
        let path = manifest_dir.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for term in &forbidden {
            if source.contains(term) {
                violations.push(format!("{relative}: {term}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "host vocabulary leaked into canonical Pact substrate fields: {violations:?}"
    );
}

#[test]
fn test_file_reanchor_writes_explicit_frozen_anchors() {
    let path = temp_path("test_file_reanchor_writes_explicit_frozen_anchors");
    let anchored_path = temp_path("test_file_reanchor_writes_explicit_frozen_anchors_out");
    let stripped_path = temp_path("test_file_reanchor_writes_explicit_frozen_anchors_stripped");
    write_commit_fixture(&path);
    for index in 1..=10 {
        let (commit, nodes) = insert_text_commit(
            index,
            index,
            &format!("reanchor_{index}"),
            &format!("reanchor content {index}"),
        );
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    file_reanchor_json(
        &path,
        &anchored_path,
        ReanchorOptions {
            freeze_cycle_interval: Some(1),
            freeze_commit_bytes: None,
        },
    )
    .expect("reanchor");
    let anchored = scan_file(&fs::read(&anchored_path).expect("read anchored"), true);
    let source_current: Value =
        serde_json::from_str(&file_project_head_json(&path).expect("source current"))
            .expect("source json");
    let anchored_current: Value =
        serde_json::from_str(&file_project_head_json(&anchored_path).expect("anchored current"))
            .expect("anchored json");

    assert!(anchored.manifest.frozen_cycle_count > 0);
    assert_eq!(anchored_current, source_current);

    file_reanchor_json(
        &anchored_path,
        &stripped_path,
        ReanchorOptions::no_anchors(),
    )
    .expect("strip anchors");
    let stripped = scan_file(&fs::read(&stripped_path).expect("read stripped"), true);
    assert_eq!(stripped.manifest.frozen_cycle_count, 0);
    assert!(
        fs::read(&stripped_path).expect("stripped bytes").len()
            < fs::read(&anchored_path).expect("anchored bytes").len()
    );
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(anchored_path);
    let _ = fs::remove_file(stripped_path);
}

#[test]
fn reindex_head_rebuilds_missing_head_records() {
    let path = temp_path("reindex_head_rebuilds_missing_head_records");
    let no_head_path = temp_path("reindex_head_rebuilds_missing_head_records_no_head");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(1, 1, "needs_head", "headless");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let bytes = fs::read(&path).expect("read");
    let scanned = scan_file(&bytes, true);
    let mut builder = PactFileBuilder {
        records: Vec::new(),
    };
    for record in scanned
        .records
        .iter()
        .filter(|record| record.kind != "head")
    {
        let kind = RecordKind::from_str(&record.kind).expect("known kind");
        builder.push(kind, &record.payload).expect("push record");
    }
    fs::write(
        &no_head_path,
        builder.encode().expect("encode no-head file"),
    )
    .expect("write no-head file");
    let before = scan_file(&fs::read(&no_head_path).expect("read no-head"), true);
    assert_eq!(before.manifest.head_count, 0);
    assert_eq!(before.manifest.commit_count, 1);

    file_reindex_head_json(&no_head_path, None).expect("reindex head");
    let after = scan_file(&fs::read(&no_head_path).expect("read reindexed"), true);
    assert_eq!(after.manifest.commit_count, 1);
    assert_eq!(after.manifest.head_count, 1);
    let fast_head: Value =
        serde_json::from_str(&file_open_head_fast_json(&no_head_path).expect("fast head"))
            .expect("fast head json");
    assert_eq!(fast_head["head"]["head_revision"].as_u64(), Some(1));

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(no_head_path);
}

#[test]
fn head_selector_queries_preserve_pact_selector_semantics_without_stored_index() {
    let path =
        temp_path("head_selector_queries_preserve_pact_selector_semantics_without_stored_index");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(5, 5, "head_index_leaf", "indexed leaf");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    file_reindex_head_json(&path, None).expect("reindex head");

    let index_json = file_query_json(&path, "index", Some("head_selector")).expect("index");
    let index_result: PactFileQueryResult =
        serde_json::from_str(&index_json).expect("index result");
    assert!(index_result.matches.is_empty());

    let id_json = file_query_json(&path, "head-id", Some("head_index_leaf")).expect("id");
    let id_result: PactFileQueryResult = serde_json::from_str(&id_json).expect("id result");
    assert_eq!(
        id_result.matches[0]["component_ref"].as_str(),
        Some("head_index_leaf")
    );

    let type_json = file_query_json(&path, "type", Some("text_content")).expect("type");
    let type_result: PactFileQueryResult = serde_json::from_str(&type_json).expect("type result");
    assert!(
        type_result.matches[0]
            .as_array()
            .expect("text content refs")
            .iter()
            .any(|value| value.as_str() == Some("head_index_leaf"))
    );

    let node_query = file_query_json(&path, "node", Some("head_index_leaf")).expect("node");
    let node_result: PactFileQueryResult = serde_json::from_str(&node_query).expect("node result");
    assert_eq!(
        node_result.matches[0].get("id").and_then(Value::as_str),
        Some("head_index_leaf")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn current_head_queries_project_selectors_without_storing_head_index() {
    let path = temp_path("current_head_queries_project_selectors_without_storing_head_index");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(5, 5, "indexed_child", "indexed by head");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    file_reindex_head_json(&path, None).expect("reindex head");

    let depth_json = file_query_json(&path, "depth", Some("d2")).expect("depth query");
    let depth_result: PactFileQueryResult =
        serde_json::from_str(&depth_json).expect("depth result");
    assert!(
        depth_result.matches[0]
            .as_array()
            .expect("depth refs")
            .iter()
            .any(|value| value.as_str() == Some("indexed_child"))
    );

    let type_json = file_query_json(&path, "type", Some("text_content")).expect("type query");
    let type_result: PactFileQueryResult = serde_json::from_str(&type_json).expect("type result");
    assert!(
        type_result.matches[0]
            .as_array()
            .expect("type refs")
            .iter()
            .any(|value| value.as_str() == Some("indexed_child"))
    );

    let parent_json = file_query_json(&path, "parent", Some("turn_1")).expect("parent query");
    let parent_result: PactFileQueryResult =
        serde_json::from_str(&parent_json).expect("parent result");
    assert!(
        parent_result.matches[0]
            .as_array()
            .expect("parent refs")
            .iter()
            .any(|value| value.as_str() == Some("indexed_child"))
    );

    let id_json = file_query_json(&path, "head-id", Some("indexed_child")).expect("id query");
    let id_result: PactFileQueryResult = serde_json::from_str(&id_json).expect("id result");
    assert_eq!(
        id_result.matches[0]["component_ref"].as_str(),
        Some("indexed_child")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn reanchor_requires_explicit_policy() {
    let default_policy = ReanchorOptions::default();
    assert_eq!(
        default_policy.freeze_cycle_interval,
        Some(DEFAULT_FREEZE_CYCLE_INTERVAL)
    );
    assert_eq!(default_policy.freeze_commit_bytes, None);

    let no_anchor_policy = ReanchorOptions::no_anchors();
    assert_eq!(no_anchor_policy.freeze_cycle_interval, Some(0));
    assert_eq!(no_anchor_policy.freeze_commit_bytes, Some(0));
}

#[test]
fn test_frozen_cycle_rebuild_byte_identical() {
    let path = temp_path("test_frozen_cycle_rebuild_byte_identical");
    let anchored_path = temp_path("test_frozen_cycle_rebuild_byte_identical_anchored");
    write_commit_fixture(&path);

    for index in 1..=137 {
        let (commit, nodes) = insert_text_commit(
            index,
            index,
            &format!("cycle_{index}"),
            &format!("cycle content {index}"),
        );
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    file_reanchor_json(
        &path,
        &anchored_path,
        ReanchorOptions {
            freeze_cycle_interval: Some(1),
            freeze_commit_bytes: None,
        },
    )
    .expect("reanchor");
    let from_frozen_json = file_project_revision_json(&anchored_path, 137).expect("project");
    let bytes = fs::read(&anchored_path).expect("read");
    let mut records = scan_file(&bytes, true).records;
    assert!(records.iter().any(|record| record.kind == "frozen_cycle"));
    records.retain(|record| record.kind != "frozen_cycle");
    let full_replay = project_records_revision_file_wire_value(&records, 137).expect("full replay");
    let from_frozen: Value = serde_json::from_str(&from_frozen_json).expect("frozen json");

    assert_eq!(from_frozen, full_replay);
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(anchored_path);
}

#[test]
fn test_anchor_policy_triggers_are_explicit() {
    assert!(!should_freeze_commit_bytes(255, Some(256)));
    assert!(should_freeze_commit_bytes(256, Some(256)));
    assert!(!should_freeze_commit_bytes(
        DEFAULT_FREEZE_COMMIT_BYTES_THRESHOLD,
        None
    ));
    assert!(!should_freeze_cycle_interval(
        43,
        None,
        Some(DEFAULT_FREEZE_CYCLE_INTERVAL)
    ));
    assert!(should_freeze_cycle_interval(
        DEFAULT_FREEZE_CYCLE_INTERVAL,
        None,
        Some(DEFAULT_FREEZE_CYCLE_INTERVAL)
    ));
}

#[test]
fn test_footer_fast_open_bounded_bytes() {
    let path = temp_path("test_footer_fast_open_bounded_bytes");
    write_commit_fixture(&path);
    for index in 0..1000 {
        let (commit, nodes) =
            insert_text_commit(index + 1, index + 1, &format!("fast_{index}"), "x");
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    let summary_json = file_fast_open_summary_json(&path).expect("fast open");
    let summary: Value = serde_json::from_str(&summary_json).expect("summary json");

    assert_eq!(summary["fallback_used"], false);
    assert!(summary["bytes_read"].as_u64().unwrap() < 64 * 1024);
    assert!(summary["footer"]["manifest_offset"].as_u64().unwrap() >= FILE_HEADER_LEN as u64);
    let _ = fs::remove_file(path);
}

#[test]
fn fast_append_no_content_preserves_valid_footer_chain() {
    let path = temp_path("fast_append_no_content_preserves_valid_footer_chain");
    write_commit_fixture(&path);
    for index in 1..=3 {
        let (commit, nodes) =
            insert_text_commit(index, index, &format!("append_{index}"), "bounded");
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    let validation_json = file_validate_json(&path).expect("validate");
    let validation: PactFileValidation = serde_json::from_str(&validation_json).expect("validate");
    assert!(validation.ok);

    let head_json = file_open_head_fast_json(&path).expect("fast head");
    let head: Value = serde_json::from_str(&head_json).expect("head json");
    assert_eq!(head["fallback_used"], false);
    assert_eq!(head["head"]["head_revision"], 3);

    let _ = fs::remove_file(path);
}

#[test]
fn test_open_path_reads_footer_manifest_and_branch_head_directly() {
    let path = temp_path("test_open_path_reads_footer_manifest_and_branch_head_directly");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(3, 3, "fast_head", "head");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let head_json = file_open_head_fast_json(&path).expect("fast head");
    let head: Value = serde_json::from_str(&head_json).expect("head json");

    assert_eq!(head["fallback_used"], false);
    assert!(head["bytes_read"].as_u64().unwrap() < 64 * 1024);
    assert_eq!(head["manifest"]["schema"], PACT_FILE_SCHEMA);
    assert_eq!(head["head"]["branch_id"], "main");
    assert_eq!(head["head"]["head_revision"], 3);
    assert_eq!(head["latest_commit"]["branch_id"], "main");
    assert_eq!(head["latest_commit"]["revision"], 3);
    let _ = fs::remove_file(path);
}

#[test]
fn batch_open_head_fast_preserves_order_and_inlines_errors() {
    let valid_a = temp_path("batch_open_head_fast_valid_a");
    let valid_b = temp_path("batch_open_head_fast_valid_b");
    let valid_c = temp_path("batch_open_head_fast_valid_c");
    let missing = temp_path("batch_open_head_fast_missing");
    let corrupt = temp_path("batch_open_head_fast_corrupt");
    for (path, revision) in [(&valid_a, 2), (&valid_b, 3), (&valid_c, 4)] {
        write_commit_fixture(path);
        let (commit, nodes) =
            insert_text_commit(revision, revision, &format!("batch_{revision}"), "head");
        file_append_commit_json(path, &commit, &nodes).expect("append commit");
    }
    fs::write(&corrupt, b"PACTPKV1truncated").expect("write corrupt file");
    let paths = [
        valid_a.as_path(),
        missing.as_path(),
        valid_b.as_path(),
        corrupt.as_path(),
        valid_c.as_path(),
    ];

    let batch_json = file_batch_open_head_fast_json(&paths).expect("batch fast head");
    let batch: Vec<Value> = serde_json::from_str(&batch_json).expect("batch json");

    assert_eq!(batch.len(), paths.len());
    for (entry, path) in batch.iter().zip(paths) {
        assert_eq!(entry["path"], path.display().to_string());
    }
    for (entry, path) in [
        (&batch[0], &valid_a),
        (&batch[2], &valid_b),
        (&batch[4], &valid_c),
    ] {
        let single: Value =
            serde_json::from_str(&file_open_head_fast_json(path).expect("single fast head"))
                .expect("single json");
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["head"], single);
    }
    for entry in [&batch[1], &batch[3]] {
        assert_eq!(entry["ok"], false);
        assert!(
            entry["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
    }

    let _ = fs::remove_file(valid_a);
    let _ = fs::remove_file(valid_b);
    let _ = fs::remove_file(valid_c);
    let _ = fs::remove_file(corrupt);
}

#[test]
fn batch_open_head_fast_handles_empty_and_duplicate_inputs() {
    let empty_json = file_batch_open_head_fast_json::<&Path>(&[]).expect("empty batch");
    let empty: Vec<Value> = serde_json::from_str(&empty_json).expect("empty json");
    assert!(empty.is_empty());

    let path = temp_path("batch_open_head_fast_handles_duplicate_inputs");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(6, 6, "batch_duplicate", "head");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let paths = [path.as_path(), path.as_path()];
    let batch_json = file_batch_open_head_fast_json(&paths).expect("duplicate batch");
    let batch: Vec<Value> = serde_json::from_str(&batch_json).expect("batch json");
    let single: Value =
        serde_json::from_str(&file_open_head_fast_json(&path).expect("single fast head"))
            .expect("single json");

    assert_eq!(batch.len(), 2);
    for entry in &batch {
        assert_eq!(entry["ok"], true);
        assert_eq!(entry["path"], path.display().to_string());
        assert_eq!(entry["head"], single);
    }

    let _ = fs::remove_file(path);
}

#[test]
fn head_record_round_trips_current_tree() {
    let path = temp_path("head_record_round_trips_current_tree");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(5, 5, "head_indexed", "indexed");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let head_json = file_open_head_fast_json(&path).expect("fast head");
    let head: Value = serde_json::from_str(&head_json).expect("head json");
    let head_record = &head["head"];

    assert_eq!(head_record["root_node_ref"], "root");
    assert_eq!(head_record["head_revision"], 5);
    assert!(
        head_record["changed_node_refs"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == "head_indexed"))
    );
    assert!(
        head_record["record_sequence_ids"]
            .as_object()
            .is_some_and(|ids| ids.contains_key("head_indexed"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn ledger_load_assembles_head_from_persisted_node_records_for_fast_projection() {
    let path =
        temp_path("ledger_load_assembles_head_from_persisted_node_records_for_fast_projection");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(3, 3, "fast_head_3", "head");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert_eq!(scanned.manifest.frozen_cycle_count, 0);

    let ledger = ledger_from_records(&scanned.records).expect("ledger");
    let branch = ledger.branches.get("main").expect("main branch");
    assert_eq!(branch.head_revision, 3);
    assert_eq!(branch.current.head_revision, 3);
    assert!(branch.current.component("fast_head_3").is_some());

    crate::tests::instrumentation::reset_branch_trace_component_tree_replay_count();
    let projected =
        crate::store::project_branch_component_tree(&ledger, branch, Some(branch.head_revision))
            .expect("head projection");
    assert!(projected.contains("fast_head_3"));
    assert_eq!(
        crate::tests::instrumentation::branch_trace_component_tree_replay_count(),
        0,
        "head projection should use the assembled current tree"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn pact_head_uses_head_record_without_replaying_all_commits() {
    let path = temp_path("pact_head_uses_head_record_without_replaying_all_commits");
    write_commit_fixture(&path);
    for index in 1..=50 {
        let (commit, nodes) = insert_text_commit(index, index, &format!("fast_{index}"), "head");
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    let file_bytes = fs::metadata(&path).expect("metadata").len();
    let head_json = file_open_head_fast_json(&path).expect("fast head");
    let head: Value = serde_json::from_str(&head_json).expect("head json");

    assert_eq!(head["fallback_used"], false);
    assert_eq!(head["head"]["head_revision"], 50);
    assert!(head["bytes_read"].as_u64().unwrap() < file_bytes);

    let _ = fs::remove_file(path);
}

#[test]
fn pact_head_latency_gates() {
    let path = temp_path("pact_head_latency_gates");
    write_commit_fixture(&path);
    for index in 1..=100 {
        let (commit, nodes) = insert_text_commit(index, index, &format!("latency_{index}"), "head");
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }

    let head_json = file_open_head_fast_json(&path).expect("fast head");
    let head: Value = serde_json::from_str(&head_json).expect("head json");

    assert_eq!(head["fallback_used"], false);
    assert!(head["bytes_read"].as_u64().unwrap() < 64 * 1024);

    let _ = fs::remove_file(path);
}

#[test]
fn pact_selector_syntax_survives_commit_head_storage() {
    let path = temp_path("pact_selector_syntax_survives_commit_head_storage");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(7, 7, "selector_leaf", "selected");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let head_json = file_project_head_json(&path).expect("projected current");
    let selected = crate::selector::select_json(&head_json, "#selector_leaf").expect("select");

    assert!(selected.contains("selected"));

    let _ = fs::remove_file(path);
}

#[test]
fn torn_tail_recovers_previous_valid_head() {
    let path = temp_path("torn_tail_recovers_previous_valid_head");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(1, 1, "stable_head", "stable");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let mut bytes = fs::read(&path).expect("read");
    bytes.extend_from_slice(b"PGpartial torn record");
    fs::write(&path, bytes).expect("write torn tail");

    let head_json = file_open_head_fast_json(&path).expect("fast head fallback");
    let head: Value = serde_json::from_str(&head_json).expect("head json");

    assert_eq!(head["fallback_used"], true);
    assert_eq!(head["head"]["head_revision"], 1);

    let _ = fs::remove_file(path);
}

#[test]
fn test_torn_footer_falls_back_to_scan() {
    let path = temp_path("test_torn_footer_falls_back_to_scan");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(1, 1, "torn", "still visible");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let mut bytes = fs::read(&path).expect("read");
    bytes.truncate(bytes.len() - 8);
    fs::write(&path, bytes).expect("truncate footer");

    let summary_json = file_fast_open_summary_json(&path).expect("fast open fallback");
    let summary: Value = serde_json::from_str(&summary_json).expect("summary json");
    let projected_json = file_project_revision_json(&path, 1).expect("project");

    assert_eq!(summary["fallback_used"], true);
    assert!(projected_json.contains("still visible"));
    let _ = fs::remove_file(path);
}

#[test]
fn file_projection_family_preserves_head_revision_and_archive_scope_equivalence() {
    let path = temp_path("file_projection_scope_equivalence");
    write_commit_fixture(&path);

    let (commit, nodes) = insert_text_commit(2, 2, "archive_me", "visible before delete");
    file_append_commit_json(&path, &commit, &nodes).expect("append insert");
    let delete = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 3,
        revision: 3,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::DeleteComponent {
            component_id: "archive_me".to_string(),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&delete).expect("delete json"),
        "[]",
    )
    .expect("append delete");

    let head_tree: Value = serde_json::from_str(
        &file_project_tree_json(&path, Some("main"), None).expect("head tree"),
    )
    .expect("head tree json");
    let head_revision: Value =
        serde_json::from_str(&file_project_head_json(&path).expect("head revision"))
            .expect("head revision json");
    assert_eq!(
        head_tree, head_revision,
        "tree head and head projection APIs must agree"
    );
    assert!(
        find_node_by_id(&head_tree, "archive_me").is_none(),
        "head projection must omit deleted components"
    );

    let revision_tree: Value = serde_json::from_str(
        &file_project_tree_json(&path, Some("main"), Some(2)).expect("revision tree"),
    )
    .expect("revision tree json");
    let revision_direct: Value =
        serde_json::from_str(&file_project_revision_json(&path, 2).expect("revision direct"))
            .expect("revision direct json");
    assert_eq!(
        revision_tree, revision_direct,
        "tree revision and revision projection APIs must agree"
    );
    assert!(
        find_node_by_id(&revision_tree, "archive_me").is_some(),
        "explicit revision projection must include the component before deletion"
    );

    let archive_tree: Value = serde_json::from_str(
        &file_project_tree_json_with_scope(
            &path,
            Some("main"),
            None,
            crate::store::ProjectionTreeScope::Archive,
        )
        .expect("archive tree"),
    )
    .expect("archive tree json");
    let scanned = scan_file(&fs::read(&path).expect("read file"), true);
    let root_store = ledger_from_records(&scanned.records).expect("ledger from records");
    let root_store_json = serde_json::to_string(&root_store).expect("root store json");
    let store_archive: Value = serde_json::from_str(
        &crate::store::ledger_project_branch_tree_json_with_scope(
            &root_store_json,
            "main",
            None,
            crate::store::ProjectionTreeScope::Archive,
        )
        .expect("store archive"),
    )
    .expect("store archive json");
    assert_eq!(
        archive_tree, store_archive,
        "file archive tree projection must match store archive projection"
    );
    let archived = find_node_by_id(&archive_tree, "archive_me").expect("archived component");
    assert_eq!(archived["pact_lifecycle"], json!("deleted"));
    assert_eq!(archived["visible_in_head"], json!(false));
    assert_eq!(archived["deleted_at"], json!(3));

    let _ = fs::remove_file(path);
}

#[test]
fn test_no_full_tree_on_append() {
    let path = temp_path("test_no_full_tree_on_append");
    write_commit_fixture(&path);
    let before = fs::read(&path).expect("before bytes");
    let (commit, nodes) = insert_text_commit(1, 1, "prefix_safe", "append only");

    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let after = fs::read(&path).expect("after bytes");

    assert_eq!(&after[..before.len()], before.as_slice());
    let _ = fs::remove_file(path);
}

#[test]
fn test_view_records_not_canonical() {
    let path = temp_path("test_view_records_not_canonical");
    let root = sample_root_store();
    let views: PactViewRecords = serde_json::from_str(&sample_view_records()).expect("views");
    let root_store: ContextTreeLedger = serde_json::from_str(&root).expect("root store");
    let bytes = PactFileBuilder::from_ledger_with_views(&root_store, &views).expect("file bytes");
    fs::write(&path, bytes).expect("write");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.manifest.view_count > 0);
    let with_views =
        project_records_revision_file_wire_value(&scanned.records, 0).expect("with views");
    let without_view_records = scanned
        .records
        .into_iter()
        .filter(|record| record.kind != "view")
        .collect::<Vec<_>>();
    let without_views =
        project_records_revision_file_wire_value(&without_view_records, 0).expect("without views");

    assert_eq!(with_views, without_views);
    let _ = fs::remove_file(path);
}

#[test]
fn test_project_current_uses_latest_commit() {
    let path = temp_path("test_project_current_uses_latest_commit");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(9, 9, "current_latest", "current text");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let current = file_project_head_json(&path).expect("project current");

    assert!(current.contains("current_latest"));
    assert!(current.contains("current text"));
    let _ = fs::remove_file(path);
}

#[test]
fn stream_current_surface_returns_cursor_geometry() {
    let path = temp_path("stream_current_surface_returns_cursor_geometry");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(4, 4, "stream_leaf", "stream text");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let stream_json = file_stream_json(
        &path,
        "current_surface",
        Some("main"),
        None,
        None,
        None,
        "before",
        1,
        8,
    )
    .expect("stream current surface");
    let stream: Value = serde_json::from_str(&stream_json).expect("stream json");

    assert_eq!(stream["schema"], "pact.stream.v1");
    assert_eq!(stream["mode"], "current_surface");
    assert_ne!(stream["source"], "projection_fallback");
    assert_eq!(stream["position"]["total_rows"], 1);
    assert_eq!(stream["position"]["start_index"], 0);
    assert_eq!(stream["position"]["end_index"], 0);
    assert_eq!(stream["rows"][0]["row_id"], "node:turn_1");
    assert_eq!(stream["rows"][0]["seq"], 0);
    assert_eq!(stream["cursors"]["after"]["seq"], 0);
    assert_eq!(stream["has_before"], false);
    assert_eq!(stream["has_after"], false);
    let _ = fs::remove_file(path);
}

#[test]
fn stream_activity_surface_includes_trace_activity_rows() {
    let path = temp_path("stream_activity_surface_includes_trace_activity_rows");
    let root = sample_root_store_with_trace();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let stream_json = file_stream_json(
        &path,
        "activity_surface",
        Some("main"),
        None,
        None,
        None,
        "after",
        20,
        8,
    )
    .expect("stream activity surface");
    let stream: Value = serde_json::from_str(&stream_json).expect("stream json");
    let rows = stream["rows"].as_array().expect("rows");

    assert_eq!(stream["schema"], "pact.stream.v1");
    assert_eq!(stream["mode"], "activity_surface");
    assert_eq!(stream["position"]["total_rows"], rows.len());
    assert!(rows.iter().any(|row| {
        row.get("kind").and_then(Value::as_str) == Some("activity")
            && row.get("transaction_id").and_then(Value::as_str) == Some("tx_info")
    }));
    assert_eq!(stream["position"]["estimated"], false);
    let _ = fs::remove_file(path);
}

#[test]
fn stream_cursor_after_anchor_returns_following_rows() {
    let path = temp_path("stream_cursor_after_anchor_returns_following_rows");
    write_commit_fixture(&path);
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 2,
        revision: 2,
        lifecycle_tick: None,
        ops: vec![
            CommitRecordOp::InsertComponent {
                parent_id: "root".to_string(),
                node_ref: "turn_2".to_string(),
                position: Some(1),
            },
            CommitRecordOp::InsertComponent {
                parent_id: "root".to_string(),
                node_ref: "turn_3".to_string(),
                position: Some(2),
            },
        ],
    };
    let nodes = json!([
        {
            "id": "turn_2",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [],
        },
        {
            "id": "turn_3",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "children": [],
        }
    ]);
    file_append_commit_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &serde_json::to_string(&nodes).expect("nodes json"),
    )
    .expect("append message turns");

    let stream_json = file_stream_json(
        &path,
        "current_surface",
        Some("main"),
        None,
        None,
        Some(0),
        "after",
        2,
        8,
    )
    .expect("stream after anchor");
    let stream: Value = serde_json::from_str(&stream_json).expect("stream json");

    assert_eq!(stream["source"], "hot_query_index");
    assert_eq!(stream["fallback_used"], false);
    assert_eq!(stream["rows"][0]["seq"], 1);
    assert_eq!(stream["rows"][1]["seq"], 2);
    assert_eq!(stream["rows"][0]["row_id"], "node:turn_2");
    assert_eq!(stream["rows"][1]["row_id"], "node:turn_3");
    assert_eq!(stream["position"]["start_index"], 1);
    assert_eq!(stream["position"]["end_index"], 2);
    assert_eq!(stream["position"]["total_rows"], 3);
    assert_eq!(stream["has_before"], true);
    assert_eq!(stream["has_after"], false);
    let _ = fs::remove_file(path);
}

#[test]
fn stream_current_surface_keeps_hot_index_across_repeated_appends() {
    let path = temp_path("stream_current_surface_keeps_hot_index_across_repeated_appends");
    write_commit_fixture(&path);

    for revision in 2..=3 {
        let component_id = format!("turn_{revision}");
        let commit = CommitRecord {
            schema: COMMIT_RECORD_SCHEMA.to_string(),
            branch_id: "main".to_string(),
            cycle: revision,
            revision,
            lifecycle_tick: None,
            ops: vec![CommitRecordOp::InsertComponent {
                parent_id: "root".to_string(),
                node_ref: component_id.clone(),
                position: Some((revision - 1) as i64),
            }],
        };
        let nodes = json!([{
            "id": component_id,
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [],
        }]);
        file_append_commit_json(
            &path,
            &serde_json::to_string(&commit).expect("commit json"),
            &serde_json::to_string(&nodes).expect("nodes json"),
        )
        .expect("append message turn");
    }

    let stream_json = file_stream_json(
        &path,
        "current_surface",
        Some("main"),
        None,
        None,
        None,
        "after",
        10,
        8,
    )
    .expect("stream current surface");
    let stream: Value = serde_json::from_str(&stream_json).expect("stream json");
    let row_ids = stream["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["row_id"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();

    assert_eq!(stream["source"], "hot_query_index");
    assert_eq!(stream["fallback_used"], false);
    assert_eq!(stream["position"]["total_rows"], 3);
    assert_eq!(row_ids, vec!["node:turn_1", "node:turn_2", "node:turn_3"]);
    let _ = fs::remove_file(path);
}

#[test]
fn node_birth_query_reports_current_surface_membership() {
    let path = temp_path("node_birth_query_reports_current_surface_membership");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(6, 6, "birth_leaf", "born");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let birth_json = file_query_json(&path, "node-birth", Some("birth_leaf")).expect("birth query");
    let birth: PactFileQueryResult = serde_json::from_str(&birth_json).expect("birth json");

    assert_eq!(birth.matches[0]["component_id"], "birth_leaf");
    assert_eq!(birth.matches[0]["first_seen"], 6);
    assert_eq!(birth.matches[0]["current_surface"], true);
    let _ = fs::remove_file(path);
}

#[test]
fn file_create_branch_at_records_same_file_lineage_without_copying_content() {



    let path = temp_path("file_create_branch_at_records_same_file_lineage");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(3, 3, "fork_leaf", "fork text");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");

    let before = file_info_json(&path).expect("before info");
    let before_info: Value = serde_json::from_str(&before).expect("before json");
    let fork_json = file_create_branch_at_json(&path, Some("main"), Some(3), Some("forked"), 8)
        .expect("create branch at");
    let forked: Value = serde_json::from_str(&fork_json).expect("fork json");
    let root = ledger_from_records(&scan_file(&fs::read(&path).expect("read"), true).records)
        .expect("root store");

    assert_eq!(forked["branch_id"], "forked");
    assert!(forked.get("source_patch_count_at_fork").is_none());
    assert_eq!(root.current_branch_id, "forked");
    assert_eq!(
        root.branches["forked"]
            .fork_ref
            .as_ref()
            .and_then(|fork_ref| fork_ref.revision),
        Some(3)
    );
    assert_eq!(
        root.branches["forked"].parent_branch_id.as_deref(),
        Some("main")
    );
    assert!(root.branches["forked"].base_ref.content_ref.is_none());
    let forked_head: Value =
        serde_json::from_str(&file_project_tree_json(&path, Some("forked"), None).unwrap())
            .expect("forked projected json");
    assert!(find_node_by_id(&forked_head, "fork_leaf").is_some());
    let after = file_info_json(&path).expect("after info");
    let after_info: Value = serde_json::from_str(&after).expect("after json");
    assert_eq!(
        before_info["content"]["record_count"],
        after_info["content"]["record_count"]
    );
    let _ = fs::remove_file(path);
}

#[test]
fn file_rewind_to_moves_branch_head_without_python_replay() {
    let path = temp_path("file_rewind_to_moves_branch_head_without_python_replay");
    write_commit_fixture(&path);
    let (commit_2, nodes_2) = insert_text_commit(2, 2, "kept_leaf", "kept");
    file_append_commit_json(&path, &commit_2, &nodes_2).expect("append kept");
    let (commit_3, nodes_3) = insert_text_commit(3, 3, "dropped_leaf", "dropped");
    file_append_commit_json(&path, &commit_3, &nodes_3).expect("append dropped");

    file_rewind_to_json(&path, Some("main"), 2, 8).expect("rewind");
    let head: Value = serde_json::from_str(&file_head_json(&path, Some("main"), None).unwrap())
        .expect("head json");
    let projected: Value =
        serde_json::from_str(&file_project_tree_json(&path, Some("main"), None).unwrap())
            .expect("projected json");

    assert_eq!(head["revision"], 2);
    assert!(find_node_by_id(&projected, "kept_leaf").is_some());
    assert!(find_node_by_id(&projected, "dropped_leaf").is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn test_torn_commit_at_eof_is_excluded_from_replay() {
    let path = temp_path("test_torn_commit_at_eof_is_excluded_from_replay");
    write_commit_fixture(&path);
    let before = fs::read(&path).expect("before");
    let (commit, nodes) = insert_text_commit(2, 2, "torn_commit", "must not appear");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let after = fs::read(&path).expect("after");
    let appended_without_footer = after.len() - before.len() - FILE_FOOTER_LEN;
    let torn_len = before.len() + appended_without_footer / 2;
    fs::write(&path, &after[..torn_len]).expect("write torn commit");

    let scanned = scan_file(&fs::read(&path).expect("read torn"), true);
    let projected = file_project_revision_json(&path, 2).expect("project torn");

    assert!(scanned.manifest.last_valid_offset <= before.len());
    assert!(!projected.contains("torn_commit"));
    assert!(!projected.contains("must not appear"));
    let _ = fs::remove_file(path);
}

#[test]
fn test_dropped_latest_frozen_cycle_falls_back_to_previous() {
    let path = temp_path("test_dropped_latest_frozen_cycle_falls_back_to_previous");
    write_commit_fixture(&path);
    for index in 1..=60 {
        let (commit, nodes) = insert_text_commit(
            index,
            index,
            &format!("frozen_fallback_{index}"),
            &format!("fallback content {index}"),
        );
        file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    }
    let mut records = scan_file(&fs::read(&path).expect("read"), true).records;
    let root_50 = project_records_revision_file_wire_value(&records, 50).expect("cycle 50");
    let root_50_json = serde_json::to_string(&root_50).expect("root json");
    let root_ref = "test_frozen_50";
    records.push(ScannedRecord {
        kind: "content".to_string(),
        payload: serde_json::to_value(ContentPayload {
            id: root_ref.to_string(),
            kind: "frozen.root".to_string(),
            hash: stable_hash("frozen.root", root_50_json.as_bytes()),
            bytes: root_50_json.len(),
            chars: Some(root_50_json.chars().count()),
            preview_inline: Some(root_50_json.chars().take(160).collect()),
            chunk_count: 1,
            codec: "none".to_string(),
            encoding: None,
            text: Some(root_50_json.clone()),
        })
        .expect("content payload"),
        payload_bytes: 0,
        sequence: 0,
    });
    records.push(ScannedRecord {
        kind: "content_chunk".to_string(),
        payload: json!({
            "content_id": root_ref,
            "ordinal": 0,
            "hash": stable_hash("content.chunk", root_50_json.as_bytes()),
            "bytes": root_50_json.len(),
            "codec": "none",
            "text": root_50_json,
        }),
        payload_bytes: 0,
        sequence: 0,
    });
    records.push(ScannedRecord {
        kind: "frozen_cycle".to_string(),
        payload: serde_json::to_value(FrozenCycleRecord {
            schema: FROZEN_CYCLE_SCHEMA.to_string(),
            cycle: 50,
            revision: 50,
            root_ref: root_ref.to_string(),
            index_refs: Vec::new(),
            hash: stable_hash("frozen.root", root_50_json.as_bytes()),
        })
        .expect("frozen payload"),
        payload_bytes: 0,
        sequence: 0,
    });

    let with_latest =
        project_records_revision_file_wire_value(&records, 60).expect("with latest frozen");
    let without_latest = records
        .into_iter()
        .filter(|record| {
            !(record.kind == "frozen_cycle"
                && record.payload.get("cycle").and_then(Value::as_u64) == Some(50))
        })
        .collect::<Vec<_>>();
    let fallback =
        project_records_revision_file_wire_value(&without_latest, 60).expect("fallback frozen");

    assert_eq!(with_latest, fallback);
    let _ = fs::remove_file(path);
}

#[test]
fn test_concurrent_reader_uses_previous_footer_until_new_footer_is_published() {
    let path =
        temp_path("test_concurrent_reader_uses_previous_footer_until_new_footer_is_published");
    write_commit_fixture(&path);
    let before = fs::read(&path).expect("before");
    let old_footer = latest_valid_footer(&before).expect("old footer");
    let (commit, nodes) = insert_text_commit(1, 1, "pending_without_footer", "pending");
    file_append_commit_json(&path, &commit, &nodes).expect("append commit");
    let after = fs::read(&path).expect("after");
    let partial_without_new_footer = &after[..after.len() - FILE_FOOTER_LEN];

    let visible_footer = latest_valid_footer(partial_without_new_footer).expect("previous footer");

    assert_eq!(visible_footer.offset, old_footer.offset);
    assert_eq!(visible_footer.checksum, old_footer.checksum);
    let _ = fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn test_zstd_put_text_round_trips() {
    let mut collector = ContentCollector::with_compression().with_inline_previews();
    let text = "compress me ".repeat(160);
    let content_id = collector
        .put_text("test.content", "zstd_roundtrip", &text)
        .expect("put compressed text");
    let records = collector
        .by_hash
        .values()
        .map(|payload| ScannedRecord {
            kind: "content".to_string(),
            payload: serde_json::to_value(payload).expect("content payload"),
            payload_bytes: 0,
            sequence: 0,
        })
        .collect::<Vec<_>>();

    let decoded = content_texts_from_records(&records)
        .remove(&content_id)
        .expect("decoded content");

    assert_eq!(decoded, text);
    assert!(
        collector
            .by_hash
            .values()
            .all(|payload| payload.codec == "zstd")
    );
}

#[cfg(feature = "compression-zstd")]
#[test]
fn content_body_compression_is_profit_aware() {
    let tiny = encode_content_body(b"short", true).expect("tiny content");
    assert_eq!(tiny.codec, "none");

    let repeated = encode_content_body("compress me ".repeat(4096).as_bytes(), true)
        .expect("large repeated content");
    assert_eq!(repeated.codec, "zstd");
    assert_eq!(repeated.encoding.as_deref(), Some("base64"));
}

#[cfg(feature = "compression-zstd")]
#[test]
fn content_preview_does_not_decompress_payload_page() {
    let mut collector = ContentCollector::with_compression().with_inline_previews();
    let text = format!("{}{}", "a".repeat(DEFAULT_CHUNK_BYTES), "b".repeat(128));
    let content_id = collector
        .put_text("test.content", "zstd_single_chunk", &text)
        .expect("put preview text");
    let content_by_id = content_payloads_by_id(
        &collector
            .by_hash
            .values()
            .map(|payload| ScannedRecord {
                kind: "content".to_string(),
                payload: serde_json::to_value(payload).expect("content payload"),
                payload_bytes: 0,
                sequence: 0,
            })
            .collect::<Vec<_>>(),
    );

    let preview = content_preview_inline_by_id(&content_by_id, &content_id)
        .expect("preview without body decode");

    assert_eq!(preview, &text.chars().take(160).collect::<String>());
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_content_page_round_trips_utf8_and_binary() {
    let mut collector = ContentCollector::with_compression();
    let text = format!(
        "{}{}{}",
        "a".repeat(DEFAULT_CHUNK_BYTES - 1),
        "🙂",
        "b".repeat(128)
    );
    let content_id = collector
        .put_text("test.content", "zstd_utf8_boundary", &text)
        .expect("put chunked text");
    let records = collector
        .by_hash
        .values()
        .map(|payload| ScannedRecord {
            kind: "content".to_string(),
            payload: serde_json::to_value(payload).expect("content payload"),
            payload_bytes: 0,
            sequence: 0,
        })
        .chain(collector.chunks.iter().map(|chunk| ScannedRecord {
            kind: "content_chunk".to_string(),
            payload: serde_json::to_value(chunk).expect("chunk payload"),
            payload_bytes: 0,
            sequence: 0,
        }))
        .collect::<Vec<_>>();

    let decoded = content_texts_from_records(&records)
        .remove(&content_id)
        .expect("decoded content");
    let binary = vec![0, 1, 2, 255, 128, 64];
    let encoded = encode_content_body(&binary, true).expect("binary payload");
    let payload = ContentPayload {
        id: "cnt_binary".to_string(),
        kind: "binary".to_string(),
        hash: stable_hash("binary", &binary),
        bytes: binary.len(),
        chars: None,
        preview_inline: None,
        chunk_count: 1,
        codec: encoded.codec,
        encoding: encoded.encoding,
        text: Some(encoded.data),
    };

    assert_eq!(decoded, text);
    assert_eq!(
        decode_content_payload_bytes(&payload).expect("decode binary"),
        binary
    );
}

#[cfg(feature = "compression-zstd")]
#[test]
fn test_mixed_codec_store_reads_none_and_zstd_chunks() {
    let compressed = zstd::stream::encode_all("world".as_bytes(), 0).expect("compress");
    let records = vec![
        ScannedRecord {
            kind: "content_chunk".to_string(),
            payload: json!({
                "content_id": "mixed",
                "ordinal": 0,
                "hash": stable_hash("content.chunk", b"hello "),
                "bytes": 6,
                "codec": "none",
                "text": "hello ",
            }),
            payload_bytes: 0,
            sequence: 0,
        },
        ScannedRecord {
            kind: "content_chunk".to_string(),
            payload: json!({
                "content_id": "mixed",
                "ordinal": 1,
                "hash": stable_hash("content.chunk", b"world"),
                "bytes": compressed.len(),
                "codec": "zstd",
                "encoding": "base64",
                "text": base64_encode(&compressed),
            }),
            payload_bytes: 0,
            sequence: 0,
        },
    ];

    let mixed = content_texts_from_records(&records)
        .remove("mixed")
        .expect("mixed content");

    assert_eq!(mixed, "hello world");
}

pub(super) fn sample_view_records() -> String {
    r#"{
          "views": [{
            "view_id": "view_alpha",
            "root_id": "root",
            "source_branch_id": "main",
            "source_revision": 0,
            "mode": "mutable_live",
            "view_branch_id": "branch_view",
            "mount_boundary": "pre_request",
            "follow_cursor": "head",
            "local_transaction_start": "ctx_view_1",
            "local_transaction_end": "ctx_view_1",
            "changed_component_ids": ["cmp_file_index"],
            "touched_node_refs": [{"kind":"node","id":"turn_1"}],
            "external_refs": [{"kind":"workspace_file","uri":"file:///workspace/VIEW.md"}],
            "conflict_policy_hints": {"component":"manual"}
          }]
        }"#
    .to_string()
}

#[test]
fn creates_and_inspects_binary_pact_file() {
    let path = temp_path("creates_and_inspects_binary_pact_file");
    let root = sample_root_store();
    let manifest_json = file_create_from_ledger_json(&path, &root, 8).expect("create");
    let created: Value = serde_json::from_str(&manifest_json).expect("create report");
    let manifest = scan_file(&fs::read(&path).expect("read pact"), true).manifest;

    assert_eq!(created["schema"], PACT_FILE_SCHEMA);
    assert_eq!(manifest.version, FILE_VERSION);
    assert!(manifest.file_bytes > FILE_HEADER_LEN);
    assert!(manifest.page_count >= 4);
    assert!(manifest.symbol_count <= manifest.record_count);
    assert!(manifest.content_record_count > 0);
    assert_eq!(manifest.content_chunk_count, 0);
    assert_eq!(manifest.branch_count, 1);
    assert!(manifest.node_count >= 4);
    assert_eq!(manifest.file_kind, "branch");
    assert_eq!(manifest.cycle_count, 1);
    assert!(manifest.capabilities.contains(&"cycles".to_string()));
    assert!(manifest.capabilities.contains(&"project_head".to_string()));
    assert!(
        manifest
            .capabilities
            .contains(&"project_revision".to_string())
    );
    assert_eq!(manifest.cycle_ranges[0].cycle, 1);
    assert_eq!(manifest.cycle_ranges[0].tool_call_count, 1);
    assert_eq!(manifest.cycle_ranges[0].tool_result_count, 1);

    let _ = fs::remove_file(path);
}

#[test]
fn branch_current_stores_node_refs_not_embedded_nodes() {
    let path = temp_path("branch_current_stores_node_refs_not_embedded_nodes");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let bytes = fs::read(&path).expect("read");
    let scanned = scan_file(&bytes, true);
    assert!(scanned.errors.is_empty());
    let branch_record = scanned
        .records
        .iter()
        .find(|record| record.kind == "branch")
        .expect("branch record");
    let current = branch_record
        .payload
        .get("current")
        .and_then(Value::as_object)
        .expect("branch current object");
    let embedded_nodes = current
        .get("components")
        .and_then(Value::as_object)
        .expect("current nodes object");
    let node_refs = current
        .get("node_refs")
        .and_then(Value::as_object)
        .expect("current node refs");
    assert!(embedded_nodes.is_empty());
    assert!(node_refs.contains_key("root"));
    assert!(node_refs.contains_key("turn_1"));

    let root_store_json = file_ledger_json(&path).expect("root store");
    let root_store: crate::store::ContextTreeLedger =
        serde_json::from_str(&root_store_json).expect("root store json");
    let branch = root_store.branches.get("main").expect("main branch");
    let records = branch.current.component_records();
    assert!(records.contains_key("root"));
    assert!(records.contains_key("turn_1"));
    let turn = records.get("turn_1").expect("turn node");
    assert!(!turn.aux.contains_key("componentType"));
    assert!(!turn.aux.contains_key("role"));

    let projected =
        file_project_tree_json(&path, Some("main"), None).expect("project compact branch");
    assert!(projected.contains("hello from a compact pact file"));

    let _ = fs::remove_file(path);
}

#[test]
#[cfg(feature = "compression-zstd")]
fn symbol_and_record_pages_are_compressed_when_profitable() {
    let path = temp_path("symbol_and_record_pages_are_compressed_when_profitable");
    let children = (0..200)
            .map(|index| {
                json!({
                    "id": format!("msg_{index:04}"),
                    "componentType": "message_turn",
                    "type": "message_turn",
                    "role": if index % 2 == 0 { "user" } else { "assistant" },
                    "offset": index,
                    "pact_depth": 0,
                    "attrs": {
                        "cycle": index,
                        "repeated_key": "same repeated value",
                        "another_repeated_key": "another repeated value"
                    },
                    "children": [{
                        "id": format!("txt_{index:04}"),
                        "componentType": "text_content",
                        "type": "text_content",
                        "offset": 0,
                        "content": format!("message body {index} with enough repeated structure to compress")
                    }]
                })
            })
            .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 64).expect("root store");
    file_create_from_ledger_json(&path, &root, 64).expect("create");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.errors.is_empty());
    assert!(scanned.manifest.pages.iter().any(|page| {
        page.page_type == "record"
            && (page.codec == "zstd" || page.codec == PACT_RECORD_ZSTD_DICT_CODEC)
            && page.stored_payload_bytes < page.decoded_payload_bytes
    }));
    assert!(scanned.manifest.pages.iter().any(|page| {
        page.page_type == "symbol"
            && page.codec == "zstd"
            && page.stored_payload_bytes < page.decoded_payload_bytes
    }));

    let projected =
        file_project_tree_json(&path, Some("main"), None).expect("project compressed pages");
    assert!(projected.contains("message body 199"));

    let _ = fs::remove_file(path);
}

#[test]
#[cfg(feature = "compression-zstd")]
fn record_pages_use_versioned_zstd_dictionary_when_profitable() {
    let path = temp_path("record_pages_use_versioned_zstd_dictionary_when_profitable");
    let children = (0..220)
        .map(|index| {
            json!({
                "id": format!("turn_{index:04}"),
                "componentType": "message_turn",
                "type": "message_turn",
                "role": "assistant",
                "offset": index,
                "attrs": {
                    "branch_id": "main",
                    "revision": index,
                    "transaction_id": format!("tx_{index:04}")
                },
                "children": [{
                    "id": format!("call_{index:04}"),
                    "componentType": "tool_call",
                    "type": "tool_call",
                    "tool_call_id": format!("call_{index:04}"),
                    "tool_name": "sample_tool",
                    "parameters_ref": format!("cnt_params_{index:04}")
                }]
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 64).expect("root store");
    file_create_from_ledger_json(&path, &root, 64).expect("create");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.manifest.pages.iter().any(|page| {
        page.page_type == "record"
            && page.codec == PACT_RECORD_ZSTD_DICT_CODEC
            && page.stored_payload_bytes < page.decoded_payload_bytes
    }));

    let projected = file_project_tree_json(&path, Some("main"), None).expect("project dict pages");
    assert!(projected.contains("sample_tool"));

    let _ = fs::remove_file(path);
}

#[test]
#[cfg(feature = "compression-zstd")]
fn content_pages_compress_when_profitable() {
    let path = temp_path("content_pages_compress_when_profitable");
    let repeated = "same content page body\n".repeat(512);
    let children = (0..96)
        .map(|index| {
            json!({
                "id": format!("content_node_{index:04}"),
                "componentType": "text_content",
                "type": "text_content",
                "content": format!("{repeated}{index}")
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 64).expect("root store");
    file_create_from_ledger_json(&path, &root, 64).expect("create");

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.manifest.pages.iter().any(|page| {
        page.page_type == "content"
            && page.codec == "zstd"
            && page.stored_payload_bytes < page.decoded_payload_bytes
    }));

    let _ = fs::remove_file(path);
}

#[test]
fn symbol_records_round_trip_and_expand_to_plain_json() {
    let path = temp_path("symbol_records_round_trip_and_expand_to_plain_json");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let bytes = fs::read(&path).expect("read");
    assert!(bytes.contains(&VALUE_TAG_SYMBOL_REF));
    let scanned = scan_file(&bytes, true);
    assert!(scanned.errors.is_empty());
    assert!(scanned.manifest.symbol_count > 0);
    assert!(
        scanned
            .records
            .iter()
            .filter(|record| record.kind != "symbol")
            .all(|record| !serde_json::to_string(&record.payload)
                .expect("payload json")
                .contains("VALUE_TAG_SYMBOL_REF"))
    );
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");
    assert!(info["symbols"]["symbol_count"].as_u64().unwrap_or_default() > 0);
    assert!(
        info["symbols"]["substituted_occurrence_count"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );

    let _ = fs::remove_file(path);
}

#[test]
fn symbol_substitution_preserves_hash_and_query_results() {
    let path = temp_path("symbol_substitution_preserves_hash_and_query_results");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let validation_json = file_validate_json(&path).expect("validate");
    let validation: PactFileValidation =
        serde_json::from_str(&validation_json).expect("validation json");
    assert!(validation.ok);
    let node_json = file_query_json(&path, "node", Some("turn_1")).expect("node query");
    let node: PactFileQueryResult = serde_json::from_str(&node_json).expect("node json");
    assert_eq!(node.matches.len(), 1);
    assert_eq!(node.matches[0]["id"], "turn_1");

    let _ = fs::remove_file(path);
}

#[test]
fn built_in_symbols_emit_no_symbol_records() {
    assert!(SymbolTable::default().records().is_empty());
    let payload = json!({
        "schema": COMMIT_RECORD_SCHEMA,
        "branch_id": "main",
        "cycle": 1,
        "revision": 1,
        "ops": [{"kind": "state_changed", "target": "component.delta_state"}],
    });
    let encoded = encode_record_payload(RecordKind::Commit, &payload).expect("encode");
    assert!(encoded.contains(&VALUE_TAG_SYMBOL_REF));
    let decoded = decode_record_payload("commit", &encoded).expect("decode");
    assert_eq!(decoded["schema"], COMMIT_RECORD_SCHEMA);
    assert_eq!(decoded["branch_id"], "main");
    assert_eq!(decoded["ops"][0]["kind"], "state_changed");
    assert_eq!(decoded["ops"][0]["target"], "component.delta_state");
}

#[test]
fn projects_cycle_range_with_unified_range_syntax() {
    let path = temp_path("projects_cycle_range_with_unified_range_syntax");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let projection_json = file_project_range_json(&path, "1..1", Some("main"), None, 8, None, None)
        .expect("range projection");
    let projection: Value = serde_json::from_str(&projection_json).expect("range projection json");
    assert_eq!(projection["range"]["syntax"], "1..1");
    assert_eq!(projection["range"]["start"], 1);
    assert_eq!(projection["range"]["end"], 1);
    assert_eq!(projection["branch"]["message_order"][0], "turn_1");
    assert!(projection["branch"]["components"]["tool_call"].is_object());

    let _ = fs::remove_file(path);
}

#[test]
fn validates_checksums_and_reports_corruption() {
    let path = temp_path("validates_checksums_and_reports_corruption");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let valid_json = file_validate_json(&path).expect("validate");
    let valid: PactFileValidation = serde_json::from_str(&valid_json).expect("valid json");
    assert!(valid.ok);

    let mut bytes = fs::read(&path).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x55;
    fs::write(&path, bytes).expect("write corrupt");
    let invalid_json = file_validate_json(&path).expect("validate corrupt");
    let invalid: PactFileValidation = serde_json::from_str(&invalid_json).expect("invalid json");
    assert!(!invalid.ok);
    assert!(invalid.errors.iter().any(|err| err.contains("checksum")));

    let _ = fs::remove_file(path);
}

#[test]
fn queries_structural_indexes() {
    let path = temp_path("queries_structural_indexes");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let node_json = file_query_json(&path, "node", Some("turn_1")).expect("node query");
    let node: PactFileQueryResult = serde_json::from_str(&node_json).expect("node json");
    assert_eq!(node.matches.len(), 1);
    assert_eq!(node.matches[0]["role"], "user");

    let tool_json = file_query_json(&path, "tool-pair", Some("toolu_1")).expect("tool query");
    let tool: PactFileQueryResult = serde_json::from_str(&tool_json).expect("tool json");
    assert_eq!(tool.matches.len(), 1);
    assert_eq!(tool.matches[0]["call_ids"][0], "tool_call");
    assert_eq!(tool.matches[0]["result_ids"][0], "tool_result");

    let aux_json = file_query_json(&path, "aux", Some("app.activity")).expect("aux query");
    let aux: PactFileQueryResult = serde_json::from_str(&aux_json).expect("aux json");
    assert_eq!(aux.matches.len(), 1);
    assert_eq!(aux.matches[0][0], "tool_call");

    let cycles_json = file_query_json(&path, "cycles", None).expect("cycles query");
    let cycles: PactFileQueryResult = serde_json::from_str(&cycles_json).expect("cycles json");
    assert_eq!(cycles.matches.len(), 1);
    assert_eq!(cycles.matches[0]["cycle"], 1);
    assert_eq!(cycles.matches[0]["message_count"], 1);

    let cycle_json = file_query_json(&path, "cycle", Some("1")).expect("cycle query");
    let cycle: PactFileQueryResult = serde_json::from_str(&cycle_json).expect("cycle json");
    assert!(
        cycle
            .matches
            .iter()
            .any(|node| node.get("id").and_then(Value::as_str) == Some("tool_call"))
    );

    let depth_json = file_query_json(&path, "depth", Some("d0")).expect("depth query");
    let depth: PactFileQueryResult = serde_json::from_str(&depth_json).expect("depth json");
    assert_eq!(depth.matches.len(), 1);
    assert_eq!(depth.matches[0][0], "turn_1");

    let branch_json = file_query_json(&path, "branch-head", Some("main")).expect("branch query");
    let branch: PactFileQueryResult = serde_json::from_str(&branch_json).expect("branch json");
    assert_eq!(branch.matches.len(), 1);
    assert_eq!(branch.matches[0]["head_revision"], 1);

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_reports_resident_content_ref_hits() {
    let path = temp_path("text_search_reports_resident_content_ref_hits");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "hello from a compact pact file"
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let mut query = PactTextSearchQuery::new("compact pact");
    query.scope = "head".to_string();
    let search_json = file_text_search_json(&path, query).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].component_id, "text_1");
    assert_eq!(search.hits[0].field, "content_ref");
    assert_eq!(search.hits[0].match_range.byte_start, 13);
    assert_eq!(search.hits[0].match_range.byte_end, 25);
    assert!(search.hits[0].visibility.resident_in_current_surface);
    assert!(search.hits[0].exact_text_preview.contains("compact pact"));

    let all_search_json =
        file_text_search_json(&path, PactTextSearchQuery::new("compact pact")).expect("all search");
    let all_search: PactTextSearchResult =
        serde_json::from_str(&all_search_json).expect("all search json");
    assert_eq!(all_search.hits.len(), 1);
    assert_eq!(all_search.hits[0].component_id, "text_1");
    assert!(
        all_search.hits[0].visibility.resident_in_current_surface,
        "all search must include the committed head, including nodes stored via the head snapshot"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_finds_nonresident_archive_hits() {
    let path = temp_path("text_search_finds_nonresident_archive_hits");
    write_commit_fixture(&path);
    let (commit, nodes) = insert_text_commit(7, 7, "gone_text", "recoverable archive phrase");
    file_append_commit_json(&path, &commit, &nodes).expect("append insert");
    let delete = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 8,
        revision: 8,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::DeleteComponent {
            component_id: "gone_text".to_string(),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&delete).expect("delete json"),
        "[]",
    )
    .expect("append delete");

    let mut current_query = PactTextSearchQuery::new("recoverable archive phrase");
    current_query.scope = "head".to_string();
    let current_json = file_text_search_json(&path, current_query).expect("current search");
    let current: PactTextSearchResult =
        serde_json::from_str(&current_json).expect("current search json");
    assert!(current.hits.is_empty());

    let all_query = PactTextSearchQuery::new("recoverable archive phrase");
    let all_json = file_text_search_json(&path, all_query).expect("all search");
    let all: PactTextSearchResult = serde_json::from_str(&all_json).expect("all search json");
    assert_eq!(all.hits.len(), 1);
    assert_eq!(all.hits[0].component_id, "gone_text");
    assert!(!all.hits[0].visibility.resident_in_current_surface);
    assert!(all.hits[0].visibility.archived_in_pact);

    let mut selector_query = PactTextSearchQuery::new("recoverable archive phrase");
    selector_query.selector = Some("@history #gone_text".to_string());
    let selector_json = file_text_search_json(&path, selector_query).expect("selector search");
    let selector: PactTextSearchResult =
        serde_json::from_str(&selector_json).expect("selector search json");
    assert_eq!(selector.hits.len(), 1);
    assert_eq!(selector.hits[0].component_id, "gone_text");

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_searches_tool_result_fields() {
    let path = temp_path("text_search_searches_tool_result_fields");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [{
                "id": "result_1",
                "componentType": "tool_result",
                "type": "tool_result",
                "tool_call_id": "toolu_1",
                "result": {"stdout": "No item at offset 12", "exit_code": 1}
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 1024).expect("create");

    let mut query = PactTextSearchQuery::new("No item at offset");
    query.selector = Some(".tool_result".to_string());
    let search_json = file_text_search_json(&path, query).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].component_id, "result_1");
    assert_eq!(search.hits[0].element_kind, "tool_result");
    assert!(matches!(
        search.hits[0].field.as_str(),
        "result" | "result_ref"
    ));
    assert_eq!(
        search.hits[0]
            .message_container
            .as_ref()
            .map(|container| container.component_id.as_str()),
        Some("turn_1")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_supports_regex_case_insensitive_and_ranges() {
    let path = temp_path("text_search_supports_regex_case_insensitive_and_ranges");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text_1",
            "componentType": "text_content",
            "type": "text_content",
            "content": "hello from a compact pact file"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let mut query = PactTextSearchQuery::new("HELLO\\s+FROM");
    query.regex = true;
    query.case_insensitive = true;
    query.preview_chars = 16;
    let search_json = file_text_search_json(&path, query).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].match_range.byte_start, 0);
    assert_eq!(search.hits[0].match_range.char_start, 0);
    assert!(search.hits[0].exact_text_preview.starts_with("hello from"));

    let mut literal_query = PactTextSearchQuery::new("HELLO FROM");
    literal_query.case_insensitive = true;
    let literal_json = file_text_search_json(&path, literal_query).expect("literal search");
    let literal: PactTextSearchResult =
        serde_json::from_str(&literal_json).expect("literal search json");
    assert_eq!(literal.hits.len(), 1);
    assert_eq!(literal.hits[0].match_range.byte_start, 0);

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_selector_scope_returns_node_hits() {
    let path = temp_path("text_search_selector_scope_returns_node_hits");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_a",
            "componentType": "message_turn",
            "type": "message_turn",
            "children": [{
                "id": "text_a",
                "componentType": "text_content",
                "type": "text_content",
                "content": "later scoped"
            }]
        }, {
            "id": "turn_b",
            "componentType": "message_turn",
            "type": "message_turn",
            "children": [{
                "id": "text_b",
                "componentType": "text_content",
                "type": "text_content",
                "content": "later outside"
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let mut query = PactTextSearchQuery::new("later");
    query.selector = Some("#text_a".to_string());
    let search_json = file_text_search_json(&path, query).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert_eq!(search.query.selector.as_deref(), Some("#text_a"));
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].component_id, "text_a");

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_supports_chained_selectors_via_snapshot_window() {
    let path = temp_path("text_search_supports_chained_selectors_via_snapshot_window");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let mut query = PactTextSearchQuery::new("later");
    query.selector = Some("#turn_1 .text_content".to_string());
    let search_json = file_text_search_json(&path, query).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].component_id, "text_2");
    assert_eq!(search.source, "selector_stream:snapshot_window");
    let stats = search
        .selector_stats
        .as_ref()
        .expect("selector_stats present");
    assert_eq!(stats.source, "snapshot_window");
    assert!(!stats.fallback_used);

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_does_not_treat_ids_as_lookup_text() {
    let path = temp_path("text_search_does_not_treat_ids_as_lookup_text");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let search_json =
        file_text_search_json(&path, PactTextSearchQuery::new("text_1")).expect("text search");
    let search: PactTextSearchResult = serde_json::from_str(&search_json).expect("search json");

    assert!(search.hits.is_empty());

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_json_output_snapshot() {
    let path = temp_path("text_search_json_output_snapshot");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "children": [{
                "id": "text_1",
                "componentType": "text_content",
                "type": "text_content",
                "content": "snapshot needle"
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 8).expect("root store");
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let output =
        file_text_search_json(&path, PactTextSearchQuery::new("needle")).expect("text search");
    let snapshot: Value = serde_json::from_str(&output).expect("search json");

    assert_eq!(
        snapshot,
        json!({
            "schema": "pact.file.v0.text_search.v1",
            "query": {
                "text": "needle",
                "scope": "all",
                "case_insensitive": false,
                "regex": false,
                "limit": 20,
                "preview_chars": 240
            },
            "hits": [{
                "component_id": "text_1",
                "branch_id": "main",
                "element_kind": "text_content",
                "field": "content_ref",
                "match_range": {
                    "byte_start": 9,
                    "byte_end": 15,
                    "char_start": 9,
                    "char_end": 15
                },
                "exact_text_preview": "snapshot needle",
                "node_text_len_bytes": 15,
                "node_text_len_chars": 15,
                "message_container": {
                    "component_id": "turn_1",
                    "role": "user",
                    "token_count_estimated": false
                },
                "lifecycle": {
                    "born": {
                        "revision": 0
                    }
                },
                "visibility": {
                    "resident_in_current_surface": true,
                    "archived_in_pact": true
                }
            }],
            "source": "record_scan",
            "fallback_used": false
        })
    );

    let _ = fs::remove_file(path);
}

#[test]
fn hot_query_index_records_page_addresses_for_runtime_queries() {
    let path = temp_path("hot_query_index_records_page_addresses_for_runtime_queries");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let directory_json =
        file_query_json(&path, "index", Some("pact.hot_query_index.v1")).expect("hot index");
    let directory: PactFileQueryResult =
        serde_json::from_str(&directory_json).expect("hot index directory json");
    assert_eq!(directory.matches.len(), 1);
    assert!(directory.matches[0]["entries"]["indexes"]["node_by_id"]["turn"].is_object());

    let index_json = file_query_json(
        &path,
        "index",
        Some("pact.hot_query_index.v1.node_by_id.turn"),
    )
    .expect("node index");
    let index: PactFileQueryResult = serde_json::from_str(&index_json).expect("node index json");
    assert_eq!(index.matches.len(), 1);
    let entries = index.matches[0]["entries"].as_object().expect("entries");
    let turn_address = &entries["turn_1"];
    assert_eq!(turn_address["record_kind"], RecordKind::Node as u16);
    assert!(turn_address["page_offset"].as_u64().unwrap_or_default() > 0);

    let tool_index_json =
        file_query_json(&path, "index", Some("pact.hot_query_index.v1.tool_pair"))
            .expect("tool index");
    let tool_index: PactFileQueryResult =
        serde_json::from_str(&tool_index_json).expect("tool index json");
    assert!(
        tool_index.matches[0]["entries"]["toolu_1"]["call_ids"]
            .as_array()
            .expect("call ids")
            .iter()
            .any(|value| value.as_str() == Some("tool_call"))
    );

    let node_json = file_query_json(&path, "node", Some("turn_1")).expect("node query");
    let node: PactFileQueryResult = serde_json::from_str(&node_json).expect("node json");
    assert_eq!(node.matches[0]["id"], "turn_1");

    let order_json = file_query_json(&path, "message-order", Some("main")).expect("order query");
    let order: PactFileQueryResult = serde_json::from_str(&order_json).expect("order json");
    assert!(
        order.matches[0]
            .as_array()
            .expect("message order")
            .iter()
            .any(|value| value.as_str() == Some("turn_1"))
    );

    let _ = fs::remove_file(path);
}

#[test]
#[ignore = "component snapshot storage removed from Pact v0"]
fn component_records_use_compact_static_field_layout() {
    let component = json!({
        "component_id": "cmp_file_index",
        "root_id": "root",
        "branch_id": "main",
        "kind": "component:file_index",
        "owner": "branch:main",
        "schema_version": 1,
        "current_state_ref": {"content_ref":"state_1"},
        "revision": 2,
        "lifecycle": "active"
    });
    let json_bytes = serde_json::to_vec(&component).expect("component json");
    let compact = compact_canonical_record_payload(RecordKind::Component, component.clone());
    let compact_bytes = serde_json::to_vec(&compact).expect("compact json");
    let expanded = expand_canonical_record_payload("component", compact).expect("expand component");

    assert!(compact_bytes.len() < json_bytes.len());
    assert_eq!(expanded["component_id"], component["component_id"]);
    assert_eq!(
        expanded["current_state_ref"],
        component["current_state_ref"]
    );
    assert_eq!(expanded["lifecycle"], component["lifecycle"]);
    assert!(expanded.get("current_projection_ref").is_none());
}

#[test]
fn trace_records_use_compact_static_field_layout() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_1",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":2},
        "post_request_alteration_ref": {"root_id":"root","branch_id":"main","revision":3},
        "request_ref": {"root_id":"root","branch_id":"main","revision":4},
        "alteration_ids": ["a1", "a2"],
        "aux": {"trigger": "test"}
    });
    let json_bytes = serde_json::to_vec(&trace).expect("trace json");
    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());
    let compact_bytes = serde_json::to_vec(&compact).expect("compact json");
    let expanded = expand_canonical_record_payload("trace", compact).expect("expand trace");

    assert!(compact_bytes.len() < json_bytes.len());
    assert!(expanded["authored_ref"].is_object());
    assert_eq!(expanded["transaction_id"], trace["transaction_id"]);
    assert_eq!(expanded["request_ref"], trace["request_ref"]);
    assert!(expanded.get("response_ref").is_none());
}

#[test]
fn trace_refs_encode_relative_to_trace_context() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_relative",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {
            "root_id":"root",
            "branch_id":"main",
            "revision":2,
            "component_id": "n2"
        },
        "request_ref": {
            "root_id":"root",
            "branch_id":"main",
            "revision":3,
            "content_ref": "cnt_req"
        },
        "commit": {
            "commit_id": "commit_1",
            "phase": "request",
            "kind": "commit",
            "pact_ref": {"root_id":"root","branch_id":"main","revision":3}
        }
    });
    let compact = compact_canonical_record_payload(RecordKind::Trace, trace);
    let fields = compact.as_array().expect("compact trace vector");

    assert_eq!(fields[4], json!([1]));
    assert_eq!(fields[5], json!([2, null, "n2"]));
    assert_eq!(fields[7], json!([3, null, null, "cnt_req"]));
    assert_eq!(fields[12]["pact_ref"], json!([3]));
}

#[test]
fn trace_local_transaction_ids_use_relative_slots() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_trace",
        "authored_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 1,
            "transaction_id": "tx_trace",
            "component_id": "node_1"
        },
        "response_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 2,
            "transaction_id": "tx_followup",
            "content_ref": "cnt_followup"
        }
    });
    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());
    let fields = compact.as_array().expect("compact trace vector");

    assert_eq!(fields[4], json!([1, 0, "node_1"]));
    assert_eq!(fields[8], json!([2, 1, null, "cnt_followup"]));
    assert_eq!(fields[14], json!(["tx_trace", "tx_followup"]));

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand trace");
    assert_eq!(expanded, trace);
}

#[test]
fn trace_local_transaction_table_fixture_is_byte_stable() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_root",
        "authored_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 1,
            "transaction_id": "tx_root",
            "component_id": "node_a"
        },
        "pre_request_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 2,
            "transaction_id": "tx_pre",
            "component_id": "node_b"
        },
        "post_request_alteration_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 3,
            "transaction_id": "tx_pre",
            "content_ref": "cnt_pre"
        },
        "request_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 4,
            "transaction_id": "tx_req"
        },
        "response_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 5,
            "transaction_id": "tx_pre",
            "content_ref": "cnt_resp"
        },
        "post_response_alteration_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 6,
            "transaction_id": "tx_tail",
            "component_id": "node_tail"
        },
        "post_response_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 7,
            "transaction_id": "tx_tail",
            "component_id": "node_tail"
        },
        "alteration_ids": ["a1"],
        "commit": {
            "commit_id": "commit_1",
            "phase": "provider",
            "kind": "surface",
            "pact_ref": {
                "root_id": "root",
                "branch_id": "main",
                "revision": 8,
                "transaction_id": "tx_req",
                "content_ref": "cnt_commit"
            }
        }
    });
    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());

    assert_eq!(
        compact,
        json!([
            "pact.transaction_trace.v1",
            "root",
            "main",
            "tx_root",
            [1, 0, "node_a"],
            [2, 1, "node_b"],
            [3, 1, null, "cnt_pre"],
            [4, 2],
            [5, 1, null, "cnt_resp"],
            [6, 3, "node_tail"],
            [7, 3, "node_tail"],
            ["a1"],
            {
                "commit_id": "commit_1",
                "phase": "provider",
                "kind": "surface",
                "pact_ref": [8, 2, null, "cnt_commit"]
            },
            null,
            ["tx_root", "tx_pre", "tx_req", "tx_tail"]
        ])
    );
    assert_eq!(
        expand_canonical_record_payload("trace", compact).expect("expand trace"),
        trace
    );

    let empty_trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_empty",
        "replay_trace": {"ops": []}
    });
    let compact_empty = compact_canonical_record_payload(RecordKind::Trace, empty_trace.clone());
    assert_eq!(
        compact_empty,
        json!([
            "pact.transaction_trace.v1",
            "root",
            "main",
            "tx_empty",
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            {"ops": []}
        ])
    );
    assert_eq!(
        expand_canonical_record_payload("trace", compact_empty).expect("expand empty trace"),
        empty_trace
    );
}

#[test]
fn corrupt_trace_numeric_op_code_errors_at_codec_boundary() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_bad_trace_op",
        "replay_trace": {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": 99
            }]
        }
    });
    let payload = encode_record_payload(RecordKind::Trace, &trace).expect("encode corrupt trace");

    let err = decode_record_payload("trace", &payload).expect_err("numeric trace op code rejects");
    assert_eq!(err.kind(), crate::error::PactErrorKind::Codec);
    let message = err.to_string();
    assert!(
        message.contains("trace") && message.contains("99"),
        "codec error should name trace record and offending code: {message}"
    );
}

#[test]
fn trace_replay_op_tags_use_static_code_table() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_trace_ops",
        "replay_trace": {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [
                {
                    "component_id": "n_insert",
                    "op": "insert_component",
                    "parent_id": "root",
                    "index": 0,
                    "component": {
                        "id": "n_insert",
                        "componentType": "text_content",
                        "type": "text_content",
                        "content": "hello"
                    }
                },
                {"component_id": "n_delete", "op": "delete_component"},
                {
                    "component_id": "n_move",
                    "op": "move_component",
                    "parent_id": "root",
                    "index": 1
                },
                {
                    "component_id": "n_field",
                    "op": "set_field",
                    "key": "role",
                    "value": "assistant"
                },
                {"component_id": "n_unset_field", "op": "unset_field", "key": "role"},
                {
                    "component_id": "n_attr",
                    "op": "set_attr",
                    "key": "mood",
                    "value": "green"
                },
                {"component_id": "n_unset_attr", "op": "unset_attr", "key": "mood"},
                {
                    "component_id": "n_content",
                    "op": "set_content",
                    "content": "updated"
                },
                {
                    "component_id": "n_replace",
                    "op": "replace_content_range",
                    "start": 1,
                    "end": 2,
                    "replacement": "x"
                },
                {
                    "component_id": "n_lifecycle",
                    "op": "set_lifecycle",
                    "value": "archived"
                }
            ]
        }
    });

    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());
    let fields = compact.as_array().expect("compact trace fields");
    let ops = fields[15]["ops"].as_array().expect("compact replay ops");
    let codes = ops
        .iter()
        .map(|op| op["op"].as_u64().expect("coded op tag"))
        .collect::<Vec<_>>();
    assert_eq!(codes, (1..=10).collect::<Vec<_>>());

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand trace");
    assert_eq!(expanded, trace);
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    assert_eq!(replay.ops.len(), 10);
}

#[test]
fn trace_replay_string_op_tags_remain_v1_tolerant() {
    let compact = json!([
        "pact.transaction_trace.v1",
        "root",
        "main",
        "tx_v1_trace_ops",
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": "set_field",
                "key": "role",
                "value": "assistant"
            }]
        }
    ]);

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand v1 trace");
    assert_eq!(expanded["replay_trace"]["ops"][0]["op"], "set_field");
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    assert_eq!(replay.ops.len(), 1);
}

#[test]
fn trace_replay_numeric_and_string_op_tags_can_mix() {
    let compact = json!([
        "pact.transaction_trace.v1",
        "root",
        "main",
        "tx_mixed_trace_ops",
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        {
            "from_rev": 1,
            "to_rev": 3,
            "ops": [{
                "component_id": "node_1",
                "op": 4,
                "key": "role",
                "value": "assistant"
            }, {
                "component_id": "node_2",
                "op": "set_content",
                "content": "mixed"
            }]
        }
    ]);

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand mixed trace");
    assert_eq!(expanded["replay_trace"]["ops"][0]["op"], "set_field");
    assert_eq!(expanded["replay_trace"]["ops"][1]["op"], "set_content");
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    assert_eq!(replay.ops.len(), 2);
}

#[test]
fn trace_insert_component_payload_id_is_dropped_and_restored() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_insert_id_drop",
        "replay_trace": {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": "insert_component",
                "parent_id": "root",
                "index": 0,
                "component": {
                    "id": "node_1",
                    "componentType": "text_content",
                    "type": "text_content",
                    "content": "hello"
                }
            }]
        }
    });

    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());
    assert_eq!(compact[15]["ops"][0]["op"], 1);
    assert!(compact[15]["ops"][0]["component"].get("id").is_none());

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand trace");
    assert_eq!(
        expanded["replay_trace"]["ops"][0]["component"]["id"],
        "node_1"
    );
    assert_eq!(expanded, trace);
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    replay.validate().expect("restored insert id validates");
}

#[test]
fn trace_insert_component_payload_id_remains_v1_tolerant() {
    let compact = json!([
        "pact.transaction_trace.v1",
        "root",
        "main",
        "tx_insert_id_v1",
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": "insert_component",
                "parent_id": "root",
                "index": 0,
                "component": {
                    "id": "node_1",
                    "componentType": "text_content",
                    "type": "text_content"
                }
            }]
        }
    ]);

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand v1 trace");
    assert_eq!(
        expanded["replay_trace"]["ops"][0]["component"]["id"],
        "node_1"
    );
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    replay.validate().expect("v1 insert id validates");
}

#[test]
fn trace_insert_component_missing_payload_id_restores_from_component_id() {
    let compact = json!([
        "pact.transaction_trace.v1",
        "root",
        "main",
        "tx_insert_id_v2",
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        null,
        {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": 1,
                "parent_id": "root",
                "index": 0,
                "component": {
                    "componentType": "text_content",
                    "type": "text_content"
                }
            }]
        }
    ]);

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand v2 trace");
    assert_eq!(
        expanded["replay_trace"]["ops"][0]["component"]["id"],
        "node_1"
    );
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    replay.validate().expect("restored v2 insert id validates");
}

#[test]
fn trace_insert_component_mismatched_payload_id_is_preserved() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_insert_id_mismatch",
        "replay_trace": {
            "from_rev": 1,
            "to_rev": 2,
            "ops": [{
                "component_id": "node_1",
                "op": "insert_component",
                "parent_id": "root",
                "index": 0,
                "component": {
                    "id": "wrong_node",
                    "componentType": "text_content",
                    "type": "text_content"
                }
            }]
        }
    });

    let compact = compact_canonical_record_payload(RecordKind::Trace, trace);
    assert_eq!(compact[15]["ops"][0]["component"]["id"], "wrong_node");
    let expanded = expand_canonical_record_payload("trace", compact).expect("expand mismatch");
    assert_eq!(
        expanded["replay_trace"]["ops"][0]["component"]["id"],
        "wrong_node"
    );
    let replay: crate::transaction_trace::TransactionTrace =
        serde_json::from_value(expanded["replay_trace"].clone()).expect("typed replay trace");
    let err = replay
        .validate()
        .expect_err("mismatched insert payload id should remain invalid");
    assert!(
        err.to_string()
            .contains("insert_component payload id wrong_node does not match component_id node_1"),
        "mismatch should stay attributed: {err}"
    );
}

#[test]
fn trace_query_expands_relative_refs_to_full_refs() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_expand_relative",
        "authored_ref": {"root_id":"root","branch_id":"main","revision":1},
        "pre_request_ref": {"root_id":"root","branch_id":"main","revision":2},
        "request_ref": {
            "root_id":"root",
            "branch_id":"main",
            "revision":3,
            "content_ref": "cnt_req"
        }
    });
    let payload = encode_record_payload(RecordKind::Trace, &trace).expect("encode trace");
    let decoded = decode_record_payload("trace", &payload).expect("decode trace");

    assert_eq!(decoded["authored_ref"], trace["authored_ref"]);
    assert_eq!(decoded["pre_request_ref"], trace["pre_request_ref"]);
    assert_eq!(decoded["request_ref"], trace["request_ref"]);
}

#[test]
fn record_payloads_use_pact_binary_value_encoding() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_binary",
        "request_ref": {"root_id":"root","branch_id":"main","revision":4},
        "alteration_ids": ["a1", "a2"],
        "aux": {"ok": true, "latency_ms": 12.5}
    });
    let payload = encode_record_payload(RecordKind::Trace, &trace).expect("encode");

    assert!(payload.starts_with(RECORD_PAYLOAD_MAGIC));
    assert!(serde_json::from_slice::<Value>(&payload).is_err());

    let decoded = decode_record_payload("trace", &payload).expect("decode");
    assert_eq!(decoded["transaction_id"], trace["transaction_id"]);
    assert_eq!(decoded["request_ref"], trace["request_ref"]);
    assert_eq!(decoded["aux"], trace["aux"]);
}

#[test]
fn canonical_enums_encode_as_compact_ids() {
    assert_eq!(PageKind::Record as u16, 4);
    assert_eq!(RecordKind::Trace as u16, 13);
    assert_eq!(RecordKind::Head as u16, 21);
}

#[test]
fn optional_fields_encode_with_presence_bitmap() {
    let payload = json!({
        "id": "n1",
        "componentType": "text_content",
        "type": "text_content",
        "role": null,
        "content": "hello",
        "content_preview": null
    });
    let compact = compact_canonical_record_payload(RecordKind::Node, payload);
    let items = compact.as_array().expect("compact node array");

    assert!(items.len() < 15);
    assert_eq!(items[0], "n1");
    assert_eq!(items[1], "text_content");
    assert_eq!(items[11], Value::Null);
    assert_eq!(items[12], Value::Null);
    assert_eq!(items[13], "hello");
}

#[test]
fn binary_payload_roundtrips_to_expanded_json() {
    let node = json!({
        "id": "node_roundtrip",
        "componentType": "text_content",
        "content": "hello"
    });
    let encoded = encode_record_payload(RecordKind::Node, &node).expect("encode");
    let decoded = decode_record_payload("node", &encoded).expect("decode");

    assert_eq!(decoded["id"], node["id"]);
    assert_eq!(decoded["componentType"], node["componentType"]);
    assert_eq!(decoded["content"], node["content"]);
}

#[test]
fn varint_lengths_use_one_byte_for_small_values() {
    let mut out = Vec::new();
    write_len(&mut out, 12).expect("write len");
    assert_eq!(out.len(), 1);

    let mut offset = 0usize;
    assert_eq!(read_len(&out, &mut offset).expect("read len"), 12);
    assert_eq!(offset, 1);
}

#[test]
fn small_numeric_values_encode_as_varints() {
    let encoded = encode_pact_value_payload(&json!({
        "small": 12,
        "negative": -3,
        "large_unsigned": u64::MAX,
    }))
    .expect("encode numeric payload");

    assert!(encoded.contains(&VALUE_TAG_ZIGZAG_I64));
    assert!(encoded.contains(&VALUE_TAG_VAR_U64));
    let decoded = decode_record_payload("meta", &encoded).expect("decode numeric payload");
    assert_eq!(decoded["small"], 12);
    assert_eq!(decoded["negative"], -3);
    assert_eq!(decoded["large_unsigned"], Value::Number(u64::MAX.into()));
}

#[test]
fn object_keys_can_use_symbol_refs() {
    let symbols = SymbolTable::default();
    let value = json!({
        "content_ref": "cnt_alpha",
        "node": "node_alpha",
    });
    let encoded = encode_pact_value_payload_with_symbols(&value, &symbols).expect("encode symbols");

    assert!(encoded.contains(&VALUE_TAG_OBJECT_SYMBOL_KEYS));
    let decoded = decode_record_payload("meta", &encoded).expect("decode symbols");
    assert_eq!(decoded, value);
}

#[test]
fn token_span_string_value_tag_round_trips_text() {
    let value = json!({
        "text": "Hello Unicode café\npoetry run pytest tests/core -v and the tokenizer should decode the text",
    });
    let encoded = encode_pact_value_payload(&value).expect("encode token-span payload");

    assert!(
        encoded.contains(&VALUE_TAG_TOKEN_SPAN_STRING),
        "plain string leaves should use the token-span string value tag by default"
    );
    assert!(
        encoded.contains(&TOKEN_SPAN_PART_LEX_REF),
        "default English lexpack should emit dense lex refs where they beat literal token ids"
    );
    assert!(
        encoded.contains(&TOKEN_SPAN_PART_LEX_REF_RUN),
        "English lex refs should be packed into runs instead of paying a tag per word"
    );
    let decoded = decode_record_payload("meta", &encoded).expect("decode token-span payload");
    assert_eq!(decoded, value);
}

#[test]
fn token_span_repeated_raw_tokens_use_local_token_table() {
    let repeated = "QZX_987654321 ".repeat(48);
    let value = json!({ "text": repeated });
    let encoded = encode_pact_value_payload(&value).expect("encode token table payload");

    assert!(
        encoded.contains(&TOKEN_SPAN_PART_TOKEN_TABLE_RUN),
        "repeated raw-token runs should use a local token table instead of wide token ids"
    );
    let decoded = decode_record_payload("meta", &encoded).expect("decode token table payload");
    assert_eq!(decoded, value);
}

#[test]
fn plain_string_payload_can_be_written_for_baseline_debug() {
    let value = json!({
        "text": "baseline plain UTF-8 escape hatch",
    });
    let encoded = encode_pact_value_payload_with_options(
        &value,
        PactValueCodecOptions {
            token_span_strings: false,
        },
    )
    .expect("encode baseline payload");

    assert!(encoded.contains(&VALUE_TAG_STRING));
    assert!(!encoded.contains(&VALUE_TAG_TOKEN_SPAN_STRING));
    let decoded = decode_record_payload("meta", &encoded).expect("decode baseline payload");
    assert_eq!(decoded, value);
}

#[test]
fn file_create_records_default_token_span_codec_substrate() {
    let path = temp_path("file_create_records_default_token_span_codec_substrate");
    let tree = json!({
        "id": "root_token_span",
        "componentType": "root",
        "children": [{
            "id": "text_token_span",
            "componentType": "text_content",
            "content": "assistant prose should be tokenized by default"
        }]
    });
    let root =
        new_ledger_json(&tree.to_string(), Some("branch_token_span"), 16).expect("root store");
    let report_json = file_create_from_ledger_json(&path, &root, 16).expect("create");
    let report: Value = serde_json::from_str(&report_json).expect("report");
    assert_eq!(report["slimming"]["token_span_strings"], true);

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.errors.is_empty(), "{:?}", scanned.errors);
    let meta = scanned
        .records
        .iter()
        .find(|record| record.kind == "meta")
        .expect("meta record");
    assert!(
        meta.payload["capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .any(|value| value.as_str() == Some(TRACE_COMPACT_V2))
    );
    assert!(
        meta.payload["capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .any(|value| value.as_str() == Some(TOKEN_SPAN_STRINGS_V1))
    );
    assert_eq!(
        meta.payload["codec_substrate"]["tokenizers"][0]["name"],
        "o200k_base"
    );
    assert_eq!(
        meta.payload["codec_substrate"]["lexical_packs"][0]["pack_id"],
        ENGLISH_LEXPACK_ID
    );
    assert_eq!(
        meta.payload["codec_substrate"]["lexical_packs"][1]["pack_id"],
        SHELL_LEXPACK_ID
    );
    assert_eq!(
        meta.payload["codec_substrate"]["lexical_packs"]
            .as_array()
            .expect("lexical packs")
            .len(),
        2
    );
    assert!(
        meta.payload["codec_substrate"]["lexical_packs"][0]["source"].is_null(),
        "default lexical pack is a decoder/runtime dependency, not embedded file baggage"
    );

    let projected =
        file_project_tree_json(&path, Some("branch_token_span"), None).expect("project");
    assert!(projected.contains("assistant prose should be tokenized by default"));
    let _ = fs::remove_file(path);
}

#[test]
fn file_create_emits_file_local_token_span_dictionary_refs() {
    let path = temp_path("file_create_emits_file_local_token_span_dictionary_refs");
    let repeated =
        "the tokenizer should reuse this repeated provider-visible phrase across records ";
    let repeated_block = repeated.repeat(8);
    let long_a = format!(
        "alpha {repeated_block}{}{repeated_block}{}",
        "a".repeat(220),
        "b".repeat(80)
    );
    let long_b = format!(
        "beta {repeated_block}{}{repeated_block}{}",
        "c".repeat(220),
        "d".repeat(80)
    );
    let tree = json!({
        "id": "root_token_span_dict",
        "componentType": "root",
        "children": [{
            "id": "text_token_span_dict_a",
            "componentType": "text_content",
            "content": long_a
        }, {
            "id": "text_token_span_dict_b",
            "componentType": "text_content",
            "content": long_b
        }]
    });
    let root =
        new_ledger_json(&tree.to_string(), Some("branch_token_span_dict"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");
    let bytes = fs::read(&path).expect("read");
    let scanned = scan_file(&bytes, true);
    assert!(scanned.errors.is_empty(), "{:?}", scanned.errors);
    let symbol = scanned
        .records
        .iter()
        .find(|record| record.kind == "symbol")
        .expect("symbol record");
    assert!(
        symbol.payload["token_span_tokens"]
            .as_array()
            .is_some_and(|spans| !spans.is_empty())
    );
    assert!(
        symbol.payload["token_span_hashes"].is_null()
            && symbol.payload["token_span_sources"].is_null(),
        "hot token-span dictionaries should carry decode material only"
    );
    assert!(
        bytes.contains(&TOKEN_SPAN_PART_SPAN_REF),
        "encoded payloads should reference file-local token-span dictionary entries"
    );
    let projected =
        file_project_tree_json(&path, Some("branch_token_span_dict"), None).expect("project");
    assert!(projected.contains(repeated.trim()));
    let _ = fs::remove_file(path);
}

#[test]
fn file_create_can_disable_token_span_for_baseline_output() {
    let path = temp_path("file_create_can_disable_token_span_for_baseline_output");
    let tree = json!({
        "id": "root_no_token_span",
        "componentType": "root",
        "children": [{
            "id": "text_no_token_span",
            "componentType": "text_content",
            "content": "baseline text"
        }]
    });
    let root =
        new_ledger_json(&tree.to_string(), Some("branch_no_token_span"), 16).expect("root store");
    let report_json =
        file_create_from_ledger_with_token_span_json(&path, &root, false).expect("create baseline");
    let report: Value = serde_json::from_str(&report_json).expect("report");
    assert_eq!(report["slimming"]["token_span_strings"], false);

    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    assert!(scanned.errors.is_empty(), "{:?}", scanned.errors);
    let meta = scanned
        .records
        .iter()
        .find(|record| record.kind == "meta")
        .expect("meta record");
    assert_eq!(meta.payload["capabilities"], json!([TRACE_COMPACT_V2]));
    assert!(meta.payload["codec_substrate"].is_null());
    let _ = fs::remove_file(path);
}

#[test]
fn token_span_stats_reports_part_counters() {
    let path = temp_path("token_span_stats_reports_part_counters");
    let tree = json!({
        "id": "root_token_span_stats",
        "componentType": "root",
        "children": [{
            "id": "text_token_span_stats",
            "componentType": "text_content",
            "content": "token span stats should count literal tokens and lexical refs"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("branch_token_span_stats"), 16)
        .expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let report_json = file_token_span_stats_json(&path).expect("token span stats");
    let report: Value = serde_json::from_str(&report_json).expect("report");
    assert!(report["token_span_strings"].as_u64().unwrap_or_default() > 0);
    assert!(report["page_token_stats"].as_array().is_some_and(|pages| {
        pages
            .iter()
            .any(|page| page["token_span_strings"].as_u64().unwrap_or_default() > 0)
    }));
    assert!(report["errors"].as_array().is_some_and(Vec::is_empty));
    let _ = fs::remove_file(path);
}

#[test]
fn page_compare_reports_candidate_deltas() {
    let plain_path = temp_path("page_compare_reports_candidate_deltas_plain");
    let token_path = temp_path("page_compare_reports_candidate_deltas_token");
    let tree = json!({
        "id": "root_page_compare",
        "componentType": "root",
        "children": [{
            "id": "text_page_compare",
            "componentType": "text_content",
            "content": "page compare should expose stored and decoded page deltas"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("branch_page_compare"), 16).expect("root");
    file_create_from_ledger_with_token_span_json(&plain_path, &root, false).expect("plain create");
    file_create_from_ledger_with_token_span_json(&token_path, &root, true).expect("token create");

    let report_json = file_page_compare_json(&plain_path, &token_path).expect("page compare");
    let report: Value = serde_json::from_str(&report_json).expect("report");
    assert_eq!(report["baseline_path"], plain_path.display().to_string());
    assert_eq!(report["candidate_path"], token_path.display().to_string());
    assert!(
        report["pages"]
            .as_array()
            .is_some_and(|pages| !pages.is_empty())
    );
    let _ = fs::remove_file(plain_path);
    let _ = fs::remove_file(token_path);
}

#[test]
fn file_repack_can_expand_token_span_to_baseline_strings() {
    let source = temp_path("file_repack_can_expand_token_span_to_baseline_strings_source");
    let output = temp_path("file_repack_can_expand_token_span_to_baseline_strings_output");
    let tree = json!({
        "id": "root_repack_token_span",
        "componentType": "root",
        "children": [{
            "id": "text_repack_token_span",
            "componentType": "text_content",
            "content": "tokenized text survives baseline repack"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("branch_repack_token_span"), 16)
        .expect("root store");
    file_create_from_ledger_json(&source, &root, 16).expect("create tokenized");

    let report_json = file_repack_with_options_json(
        &source,
        &output,
        PactRepackOptions {
            token_span_strings: false,
        },
    )
    .expect("baseline repack");
    let report: Value = serde_json::from_str(&report_json).expect("report");
    assert_eq!(report["token_span_strings"], false);
    assert_eq!(report["head_preserved"], true);

    let source_head = file_project_head_json(&source).expect("source head");
    let output_head = file_project_head_json(&output).expect("output head");
    assert_eq!(source_head, output_head);

    let scanned = scan_file(&fs::read(&output).expect("read"), true);
    assert!(scanned.errors.is_empty(), "{:?}", scanned.errors);
    let meta = scanned
        .records
        .iter()
        .find(|record| record.kind == "meta")
        .expect("meta record");
    assert!(meta.payload["codec_substrate"].is_null());

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}

#[test]
fn file_repack_writes_codec_metadata_only_on_file_meta() {
    let source = temp_path("file_repack_writes_codec_metadata_only_on_file_meta_source");
    let output = temp_path("file_repack_writes_codec_metadata_only_on_file_meta_output");
    let tree = json!({
        "id": "root_repack_codec_meta",
        "componentType": "root",
        "children": [{
            "id": "text_repack_codec_meta",
            "componentType": "text_content",
            "content": "token-span metadata should stay file-level only"
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("branch_repack_codec_meta"), 16)
        .expect("root store");
    file_create_from_ledger_json(&source, &root, 16).expect("create tokenized");

    file_repack_json(&source, &output).expect("default repack");
    let scanned = scan_file(&fs::read(&output).expect("read"), true);
    assert!(scanned.errors.is_empty(), "{:?}", scanned.errors);
    let codec_meta_count = scanned
        .records
        .iter()
        .filter(|record| record.kind == "meta")
        .filter(|record| record.payload["codec_substrate"].is_object())
        .count();
    assert_eq!(codec_meta_count, 1);

    let meta = scanned
        .records
        .iter()
        .find(|record| record.payload["codec_substrate"].is_object())
        .expect("file codec meta");
    assert!(
        meta.payload["codec_substrate"]["lexical_packs"][0]["source"].is_null(),
        "default lexical pack source data must live with the installed decoder"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}

#[test]
fn file_local_ids_expand_at_api_boundary() {
    let path = temp_path("file_local_ids_expand_at_api_boundary");
    let tree = json!({
        "id": "root_local",
        "componentType": "root",
        "attrs": {
            "session_id": "session_local"
        },
        "children": [{
            "id": "turn_local",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "attrs": {
                "session_id": "session_local",
                "transaction_id": "tx_local"
            },
            "children": [{
                "id": "text_local",
                "componentType": "text_content",
                "type": "text_content",
                "content": "file local id surface"
            }, {
                "id": "call_local",
                "componentType": "tool_call",
                "type": "tool_call",
                "tool_call_id": "call_local",
                "tool_name": "generic",
                "parameters": {"target": "root_local"}
            }, {
                "id": "result_local",
                "componentType": "tool_result",
                "type": "tool_result",
                "tool_call_id": "call_local",
                "success": true,
                "result": {"status": "ok"}
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("branch_local"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let bytes = fs::read(&path).expect("read");
    assert!(
        bytes.contains(&VALUE_TAG_FILE_LOCAL_ID_REF),
        "record payloads should use the file-local id value tag"
    );

    let node_json = file_query_json(&path, "node", Some("text_local")).expect("node query");
    let node_result: PactFileQueryResult = serde_json::from_str(&node_json).expect("node result");
    assert_eq!(
        node_result.matches[0].get("id").and_then(Value::as_str),
        Some("text_local")
    );

    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");
    let kinds = &info["symbols"]["file_local_id_kinds"];
    assert!(kinds["root"].as_u64().unwrap_or_default() > 0);
    assert!(kinds["branch"].as_u64().unwrap_or_default() > 0);
    assert!(kinds["session"].as_u64().unwrap_or_default() > 0);
    assert!(kinds["transaction"].as_u64().unwrap_or_default() > 0);
    assert!(kinds["node"].as_u64().unwrap_or_default() > 0);
    assert!(kinds["content"].as_u64().unwrap_or_default() > 0);

    let projected = file_project_tree_json(&path, Some("branch_local"), None).expect("project");
    assert!(projected.contains("file local id surface"));

    let _ = fs::remove_file(path);
}

#[test]
fn pact_refs_use_static_field_vectors_inside_records() {
    let trace = json!({
        "schema": "pact.transaction_trace.v1",
        "root_id": "root",
        "branch_id": "main",
        "transaction_id": "tx_ref_slots",
        "authored_ref": {
            "root_id": "root",
            "branch_id": "main",
            "revision": 1,
            "transaction_id": "tx_ref_slots",
            "component_id": "node_1"
        },
        "commit": {
            "commit_id": "commit_1",
            "phase": "provider",
            "kind": "surface",
            "pact_ref": {
                "root_id": "root",
                "branch_id": "main",
                "revision": 2,
                "content_ref": "cnt_1"
            }
        }
    });
    let compact = compact_canonical_record_payload(RecordKind::Trace, trace.clone());

    let compact_fields = compact.as_array().expect("compact trace vector");
    assert!(compact_fields[4].is_array());
    assert!(compact_fields[12]["pact_ref"].is_array());

    let expanded = expand_canonical_record_payload("trace", compact).expect("expand trace");
    assert_eq!(expanded["authored_ref"], trace["authored_ref"]);
    assert_eq!(expanded["commit"]["pact_ref"], trace["commit"]["pact_ref"]);
}

#[test]
fn common_records_use_static_field_vectors_before_binary_encoding() {
    let node = json!({
        "id": "node_1",
        "componentType": "message",
        "type": "text",
        "role": "user",
        "parent_id": "root",
        "offset": 0,
        "pact_depth": 1,
        "created_at_ns": 99,
        "creation_index": 7,
        "child_ids": ["node_2"],
        "aux": {"cycle": 3},
        "content_ref": "cnt_abc",
        "content_preview": "hello"
    });
    let compact = compact_canonical_record_payload(RecordKind::Node, node.clone());
    let expanded = expand_canonical_record_payload("node", compact.clone()).expect("expand node");
    let json_bytes = serde_json::to_vec(&node).expect("node json");
    let compact_binary = encode_pact_value_payload(&compact).expect("compact binary");

    assert!(compact.is_array());
    assert!(compact_binary.len() < json_bytes.len());
    assert_eq!(expanded["componentType"], node["componentType"]);
    assert_eq!(expanded["content_ref"], node["content_ref"]);
    assert_eq!(expanded["aux"], node["aux"]);
}

#[test]
fn base64_content_chunks_store_raw_bytes_in_binary_payload() {
    let raw = b"compressed-ish bytes \x00\x01\x02".to_vec();
    let encoded = base64_encode(&raw);
    let chunk = json!({
        "content_id": "cnt_1",
        "ordinal": 0,
        "hash": "hash",
        "bytes": raw.len(),
        "codec": "zstd",
        "encoding": "base64",
        "text": encoded,
    });
    let payload = encode_record_payload(RecordKind::ContentChunk, &chunk).expect("encode");
    let decoded = decode_record_payload("content_chunk", &payload).expect("decode");

    assert!(
        payload
            .windows(raw.len())
            .any(|window| window == raw.as_slice())
    );
    assert_eq!(decoded["encoding"], chunk["encoding"]);
    assert_eq!(decoded["text"], chunk["text"]);
}

#[test]
fn pact_info_reports_page_record_content_and_ref_breakdowns() {
    let path = temp_path("pact_info_reports_page_record_content_and_ref_breakdowns");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");

    assert!(
        info["pages"]["record"]["payload_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(info["records"]["node"]["count"].as_u64().unwrap_or(0) > 0);
    assert!(info["content"]["content_count"].as_u64().unwrap_or(0) > 0);
    assert_eq!(info["content"]["chunk_count"].as_u64().unwrap_or(0), 0);
    assert!(info["content"]["duplicate_chunk_hash_count"].is_number());
    assert!(info["content"]["estimated_reusable_chunk_bytes"].is_number());
    assert!(info["content"]["max_chunks_per_content"].is_number());
    assert!(info["refs"]["content_ref_count"].as_u64().unwrap_or(0) > 0);
    assert!(info["refs"]["estimated_repeated_string_bytes"].is_number());

    let _ = fs::remove_file(path);
}

#[test]
fn pact_info_reports_largest_payload_paths() {
    let path = temp_path("pact_info_reports_largest_payload_paths");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");
    let largest = info["largest_records"].as_array().expect("largest records");

    assert!(!largest.is_empty());
    assert!(largest[0]["payload_bytes"].as_u64().unwrap_or(0) > 0);
    assert!(
        largest[0]["largest_path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
    );
    assert!(largest[0]["largest_path_bytes"].as_u64().unwrap_or(0) > 0);

    let _ = fs::remove_file(path);
}

#[test]
fn pact_info_reports_top_hot_fields() {
    let path = temp_path("pact_info_reports_top_hot_fields");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");

    assert!(
        info["fields"]["top"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(info["hot_fields"]["aux"]["bytes"].as_u64().unwrap_or(0) > 0);
    assert!(info["payload_slots"]["inline_payload_count"].is_number());
    assert!(info["payload_slots"]["content_ref_count"].is_number());

    let _ = fs::remove_file(path);
}

#[test]
fn pact_info_reports_aux_and_trace_hot_fields() {
    let path = temp_path("pact_info_reports_aux_and_trace_hot_fields");
    let root = sample_root_store_with_trace();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");

    assert!(info["hot_fields"]["aux"]["bytes"].as_u64().unwrap_or(0) > 0);
    assert!(info["traces"]["count"].as_u64().unwrap_or(0) > 0);
    assert!(info["traces"]["decoded_bytes"].as_u64().unwrap_or(0) > 0);
    assert!(
        info["traces"]["relative_ref_savings_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(info["traces"]["top"].as_array().is_some());

    let _ = fs::remove_file(path);
}

#[test]
fn pact_info_storage_audit_reports_full_component_replacements() {
    let path = temp_path("pact_info_storage_audit_reports_full_component_replacements");


    write_corpus_selector_revision_fixture(&path);

    let info: Value =
        serde_json::from_str(&file_info_json(&path).expect("info")).expect("info json");
    let replacements = &info["smells"]["ledger_remediation"]["full_component_replacements"];

    assert_eq!(replacements["count"].as_u64(), Some(2));
    assert_eq!(replacements["missing_node_ref_count"].as_u64(), Some(0));
    assert!(replacements["encoded_payload_bytes"].as_u64().unwrap_or(0) > 0);
    assert!(replacements["expanded_json_bytes"].as_u64().unwrap_or(0) > 0);
    assert_eq!(
        replacements["top"]
            .as_array()
            .map(|top| top.len())
            .unwrap_or(0),
        2,
        "each replacement should appear in the top list: {replacements:?}"
    );
    assert!(
        replacements["top"]
            .as_array()
            .and_then(|top| top.first())
            .and_then(|entry| entry["component_id"].as_str())
            == Some("stable_text"),
        "replacement should reference the overwritten node: {replacements:?}"
    );
    assert!(
        info["smells"]["warnings"]
            .as_array()
            .map(|warnings| warnings
                .iter()
                .any(|warning| warning["code"].as_str() == Some("full_component_replacements")))
            .unwrap_or(false),
        "storage audit should surface a full_component_replacements warning: {info:?}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn storage_audit_reports_verbose_trace_and_derived_bytes() {
    let path = temp_path("storage_audit_reports_verbose_trace_and_derived_bytes");
    let root = sample_root_store_with_trace();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let info: Value =
        serde_json::from_str(&file_info_json(&path).expect("info")).expect("info json");
    let audit = &info["smells"]["ledger_remediation"];



    assert!(
        audit["verbose_trace"]["count"].as_u64().unwrap_or(0) > 0,
        "expected verbose trace records: {audit}"
    );
    assert!(audit["verbose_trace"]["bytes"].as_u64().unwrap_or(0) > 0);

    assert!(audit["derived_records_in_canonical_path"]["bytes"].is_u64());
    assert!(audit["derived_records_in_canonical_path"]["by_kind"].is_array());

    let _ = fs::remove_file(path);
}

#[test]
fn pact_bench_storage_reports_core_timings() {
    let path = temp_path("pact_bench_storage_reports_core_timings");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let bench_json = file_bench_storage_json(&path).expect("bench");
    let bench: Value = serde_json::from_str(&bench_json).expect("bench json");

    assert_eq!(bench["valid"], Value::Bool(true));
    assert!(bench["timings_ms"]["read_file"].is_number());
    assert!(bench["timings_ms"]["scan_manifest"].is_number());
    assert!(bench["timings_ms"]["scan_records"].is_number());
    assert!(bench["timings_ms"]["project_head"].is_number());
    assert!(bench["timings_ms"]["cached_scan_records_first"].is_number());
    assert!(bench["timings_ms"]["cached_scan_records_second"].is_number());
    assert!(bench["timings_ms"]["fast_head_open"].is_number());
    assert!(bench["timings_ms"]["preview_lookup"].is_number());
    assert!(
        bench["whole_file_compression"]["zstd_bytes"].is_u64()
            || bench["whole_file_compression"]["zstd_bytes"].is_null()
    );
    assert!(bench["scan_cache"]["hits"].as_u64().unwrap_or_default() >= 1);

    let _ = fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn pact_bench_storage_reports_jsonl_gzip_comparison() {
    let path = temp_path("pact_bench_storage_reports_jsonl_gzip_comparison");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");
    let bench_json = file_bench_storage_json(&path).expect("bench");
    let bench: Value = serde_json::from_str(&bench_json).expect("bench json");
    let comparison = &bench["jsonl_comparison"];

    assert!(
        comparison["expanded_full_state_jsonl_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(
        comparison["expanded_full_state_jsonl_gzip_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(
        comparison["provider_thread_jsonl_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(
        comparison["provider_thread_jsonl_gzip_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(comparison["ratio_pact_to_full_state_jsonl_gzip"].is_number());
    assert!(comparison["ratio_pact_zstd_to_full_state_jsonl_gzip"].is_number());
    assert!(comparison["ratio_pact_archive_zstd_to_full_state_jsonl_gzip"].is_number());
    assert!(comparison["ratio_pact_to_provider_thread_jsonl_gzip"].is_number());
    assert!(
        bench["whole_file_compression"]["semantic_archive_zstd_bytes"].is_u64()
            || bench["whole_file_compression"]["semantic_archive_zstd_bytes"].is_null()
    );

    let _ = fs::remove_file(path);
}

#[test]
fn pact_bench_storage_medium_agentic_target_reports_pass() {
    let path = temp_path("pact_bench_storage_medium_agentic_target_reports_pass");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");
    let bench_json =
        file_bench_storage_with_profile_json(&path, Some("medium-agentic")).expect("bench");
    let bench: Value = serde_json::from_str(&bench_json).expect("bench json");

    assert_eq!(bench["target"]["profile"], "medium-agentic");
    assert_eq!(bench["target"]["pass"], Value::Bool(true));
    assert!(bench["file_bytes"].as_u64().unwrap_or_default() <= 100 * 1024);
    assert!(
        bench["target"]["max_file_bytes"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert!(bench["preview_lookup"]["count"].is_u64());

    let _ = fs::remove_file(path);
}

#[test]
fn storage_profiles_do_not_change_provider_visible_head() {
    let path = temp_path("storage_profiles_do_not_change_provider_visible_head");
    let hot_path = temp_path("storage_profiles_do_not_change_provider_visible_head_hot");
    let compact_path = temp_path("storage_profiles_do_not_change_provider_visible_head_compact");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");
    let before = file_project_head_json(&path).expect("before head");

    let hot_report_json =
        file_export_storage_profile_json(&path, &hot_path, "hot").expect("hot export");
    let compact_report_json =
        file_export_storage_profile_json(&path, &compact_path, "compact").expect("compact export");
    let hot_report: Value = serde_json::from_str(&hot_report_json).expect("hot report");
    let compact_report: Value = serde_json::from_str(&compact_report_json).expect("compact report");

    assert_eq!(hot_report["page_addressable"], Value::Bool(true));
    assert_eq!(hot_report["appendable"], Value::Bool(true));
    assert_eq!(compact_report["page_addressable"], Value::Bool(true));
    assert_eq!(compact_report["appendable"], Value::Bool(true));
    assert_eq!(file_project_head_json(&hot_path).expect("hot head"), before);
    assert_eq!(
        file_project_head_json(&compact_path).expect("compact head"),
        before
    );

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(hot_path);
    let _ = fs::remove_file(compact_path);
}

#[test]
#[cfg(feature = "compression-zstd")]
fn archive_profile_beats_full_state_jsonl_gzip() {
    let path = temp_path("archive_profile_beats_full_state_jsonl_gzip");
    let archive_path = temp_path("archive_profile_beats_full_state_jsonl_gzip_archive");
    let repeated = "archive profile repeated provider-visible payload\n".repeat(4096);
    let children = (0..256)
        .map(|index| {
            json!({
                "id": format!("archive_text_{index:04}"),
                "componentType": "text_content",
                "type": "text_content",
                "content": format!("{repeated}{index}")
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 96).expect("root store");
    file_create_from_ledger_json(&path, &root, 96).expect("create");
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    let full_state_gzip =
        gzip_len(&expanded_record_jsonl_bytes(&scanned.records)).expect("full state gzip");

    let report_json =
        file_export_storage_profile_json(&path, &archive_path, "archive").expect("archive");
    let report: Value = serde_json::from_str(&report_json).expect("archive report");
    let archive_bytes = report["output_bytes"].as_u64().unwrap_or_default() as usize;

    assert_eq!(report["page_addressable"], Value::Bool(false));
    assert_eq!(report["appendable"], Value::Bool(false));
    assert_eq!(report["head_preserved"], Value::Bool(true));
    assert!(report["full_state_jsonl_gzip_bytes"].is_u64());
    assert!(report["ratio_to_full_state_jsonl_gzip"].is_number());
    assert!(report["omitted_index_bytes"].is_u64());
    assert!(
        archive_bytes < full_state_gzip,
        "archive={archive_bytes}, jsonl.gz={full_state_gzip}"
    );

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(archive_path);
}

#[test]
fn bench_medium_agentic_stays_under_100kb() {
    let path = temp_path("bench_medium_agentic_stays_under_100kb");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");

    let bytes = fs::metadata(&path).expect("metadata").len();
    let bench_json =
        file_bench_storage_with_profile_json(&path, Some("medium-agentic")).expect("bench");
    let bench: Value = serde_json::from_str(&bench_json).expect("bench json");

    assert!(bytes <= 100 * 1024, "hot pact bytes: {bytes}");
    assert_eq!(bench["target"]["pass"], Value::Bool(true));

    let _ = fs::remove_file(path);
}

#[test]
fn pact_hot_file_size_gates() {
    medium_agentic_hot_pact_is_under_100kb();
}

#[test]
fn provider_thread_projection_matches_native_tree_renderer() {
    let tree_value = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "active_turn",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "children": [{
                "id": "active_text",
                "componentType": "text_content",
                "type": "text_content",
                "content": "hello native provider projection"
            }]
        }]
    });
    let tree = ComponentTree::from_value(&tree_value).expect("component tree");
    let options = crate::provider_thread::ProviderThreadOptions::default();

    let native = crate::provider_thread::build_provider_thread_tree(&tree, &options)
        .expect("native provider thread");
    let compatibility = crate::provider_thread::build_provider_thread(&tree_value, &options)
        .expect("compat provider thread");

    assert_eq!(native, compatibility);
    assert_eq!(
        native["messages"][0]["content"][0]["content"],
        "hello native provider projection"
    );
}

#[test]
fn selector_and_text_search_projection_windows_agree_on_scope() {
    let path = temp_path("selector_and_text_search_projection_windows_agree_on_scope");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let selected: PactFileSelectorResult = serde_json::from_str(
        &file_select_json(&path, PactFileSelectorQuery::new("#turn_1 .text_content"))
            .expect("select"),
    )
    .expect("selector json");
    let mut query = PactTextSearchQuery::new("later");
    query.selector = Some("#turn_1 .text_content".to_string());
    let search: PactTextSearchResult =
        serde_json::from_str(&file_text_search_json(&path, query).expect("text search"))
            .expect("search json");

    let selected_ids = selected
        .hits
        .iter()
        .map(|hit| hit.component_id.as_str())
        .collect::<Vec<_>>();
    assert!(selected_ids.contains(&"text_2"));
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].component_id, "text_2");
    assert!(search.source.starts_with("selector_stream:"));

    let _ = fs::remove_file(path);
}

#[test]
fn hard_fork_head_snapshot_matches_source_projection() {
    let root = sample_root_store();
    let forked =
        ledger_create_branch_at_revision_json(&root, "main", Some(1), Some("snapshot"), None, 8)
            .expect("fork snapshot");
    let source: crate::store::PactView = serde_json::from_str(
        &crate::store::ledger_project_branch_json(&root, "main", Some(1), 8)
            .expect("source projection"),
    )
    .expect("source view");
    let snapshot: crate::store::PactView = serde_json::from_str(
        &crate::store::ledger_project_branch_json(&forked, "snapshot", None, 8)
            .expect("snapshot projection"),
    )
    .expect("snapshot view");

    assert_eq!(snapshot.tree.root, source.tree.root);
    assert_eq!(snapshot.message_order, source.message_order);
    assert_eq!(
        snapshot
            .component_records()
            .get("text_2")
            .and_then(|record| record.content.as_deref()),
        Some("later")
    );
}

#[test]
fn file_hard_fork_writes_child_from_head_snapshot_and_leaves_parent_unchanged() {
    let parent = temp_path("file_hard_fork_parent");
    let child = temp_path("file_hard_fork_child");
    let root = sample_root_store();
    file_create_from_ledger_json(&parent, &root, 8).expect("create parent");
    let (commit, nodes) = insert_text_commit(2, 2, "after_fork", "after fork");
    file_append_commit_json(&parent, &commit, &nodes).expect("append after fork");
    let parent_before = fs::read(&parent).expect("parent before");

    let report_json = file_fork_json(
        &parent,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        Some(r#"[{"kind":"relative_path","value":"."}]"#),
        8,
    )
    .expect("hard fork");
    let report: Value = serde_json::from_str(&report_json).expect("report json");
    assert_eq!(report["mode"], "hard");
    assert_eq!(report["revision"], 1);
    assert!(child.is_file(), "hard fork must write a child file");
    assert_eq!(fs::read(&parent).expect("parent after"), parent_before);

    let child_head: Value =
        serde_json::from_str(&file_project_tree_json(&child, Some("main"), None).expect("child"))
            .expect("child head json");
    assert!(find_node_by_id(&child_head, "text_2").is_some());
    assert!(
        find_node_by_id(&child_head, "after_fork").is_none(),
        "child must open at the fork-point head, not the parent's later head"
    );

    let parent_head: Value =
        serde_json::from_str(&file_project_tree_json(&parent, Some("main"), None).expect("parent"))
            .expect("parent head json");
    assert!(find_node_by_id(&parent_head, "after_fork").is_some());

    let child_store =
        ledger_from_records(&scan_file(&fs::read(&child).expect("read child"), true).records)
            .expect("child ledger");
    let parent_store =
        ledger_from_records(&scan_file(&parent_before, true).records).expect("parent ledger");
    let provenance = child_store.parent.expect("child parent provenance");
    assert_eq!(provenance.parent_ref.root_id, parent_store.root_id);
    assert_eq!(provenance.parent_ref.branch_id, "main");
    assert_eq!(provenance.parent_ref.revision, Some(1));
    assert_eq!(provenance.locator_hints.len(), 1);
    assert_eq!(provenance.locator_hints[0].kind, "relative_path");
    assert_eq!(provenance.locator_hints[0].value, ".");
    assert_ne!(child_store.root_id, parent_store.root_id);

    let _ = fs::remove_file(parent);
    let _ = fs::remove_file(child);
}

#[test]
fn file_select_history_crosses_hard_fork_parent_boundary() {
    let parent = temp_path("file_select_history_parent");
    let child = temp_path("file_select_history_child");
    let root = sample_root_store();
    file_create_from_ledger_json(&parent, &root, 8).expect("create parent");
    let (commit, nodes) = insert_text_commit(2, 2, "parent_archived", "parent only history");
    file_append_commit_json(&parent, &commit, &nodes).expect("append parent-only component");
    let delete = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 3,
        revision: 3,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::DeleteComponent {
            component_id: "parent_archived".to_string(),
        }],
    };
    file_append_commit_json(
        &parent,
        &serde_json::to_string(&delete).expect("delete json"),
        "[]",
    )
    .expect("append delete");

    file_fork_json(
        &parent,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(3),
        None,
        8,
    )
    .expect("hard fork");
    let child_head: Value =
        serde_json::from_str(&file_project_tree_json(&child, Some("main"), None).expect("child"))
            .expect("child head");
    assert!(find_node_by_id(&child_head, "parent_archived").is_none());

    let query = PactFileSelectorQuery::new("@history #parent_archived");
    let mut explicit_parents = BTreeMap::new();
    explicit_parents.insert("root".to_string(), parent.clone());
    let parent_config = ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents,
    };
    let err = file_select_with_parents(&child, query, &parent_config)
        .expect_err("parent-crossing history stitch is not wired yet");
    assert!(
        err.to_string()
            .contains("parent-crossing history is not yet wired"),
        "unexpected error: {err}"
    );

    let _ = fs::remove_file(parent);
    let _ = fs::remove_file(child);
}

#[test]
fn file_select_history_rejects_wrong_hard_fork_parent() {
    let parent = temp_path("file_select_wrong_parent_source");
    let child = temp_path("file_select_wrong_parent_child");
    let wrong_parent = temp_path("file_select_wrong_parent_other");
    file_create_from_ledger_json(&parent, &sample_root_store(), 8).expect("create parent");
    file_fork_json(
        &parent,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("hard fork");

    let other_root = json!({
        "id": "wrong_root",
        "componentType": "root",
        "children": []
    });
    let other_ledger =
        new_ledger_json(&other_root.to_string(), Some("main"), 8).expect("other ledger");
    file_create_from_ledger_json(&wrong_parent, &other_ledger, 8).expect("create wrong parent");

    let query = PactFileSelectorQuery::new("@history .text_content");
    let mut explicit_parents = BTreeMap::new();
    explicit_parents.insert("root".to_string(), wrong_parent.clone());
    let parent_config = ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents,
    };
    let err = file_select_with_parents(&child, query, &parent_config)
        .expect_err("wrong parent must be rejected");
    assert!(
        err.to_string().contains("parent root mismatch")
            || err.to_string().contains("does not match expected"),
        "unexpected error: {err}"
    );

    let _ = fs::remove_file(parent);
    let _ = fs::remove_file(child);
    let _ = fs::remove_file(wrong_parent);
}

#[test]
fn medium_head_projection_rss_delta_stays_bounded() {
    run_rss_child_test(
        "file_format::tests::rss_child_medium_head_projection_rss_delta_stays_bounded",
    );
}

#[test]
#[ignore = "invoked by medium_head_projection_rss_delta_stays_bounded in an isolated child process"]
fn rss_child_medium_head_projection_rss_delta_stays_bounded() {
    if !should_run_rss_child() {
        return;
    }
    let path = temp_path("medium_head_projection_rss_delta_stays_bounded");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");

    let (projection_json, probe) = run_with_rss_probe(|| {
        file_projection_json(&path, Some("main"), None, 96, None, None).expect("projection")
    });
    let projection: Value = serde_json::from_str(&projection_json).expect("projection json");
    assert_eq!(projection["schema"], PACT_FILE_PROJECTION_SCHEMA);
    assert_rss_delta_under("medium head projection", probe, 64 * 1024 * 1024);

    let _ = fs::remove_file(path);
}

#[test]
fn medium_provider_thread_projection_rss_delta_stays_bounded() {
    run_rss_child_test(
        "file_format::tests::rss_child_medium_provider_thread_projection_rss_delta_stays_bounded",
    );
}

#[test]
#[ignore = "invoked by medium_provider_thread_projection_rss_delta_stays_bounded in an isolated child process"]
fn rss_child_medium_provider_thread_projection_rss_delta_stays_bounded() {
    if !should_run_rss_child() {
        return;
    }
    let path = temp_path("medium_provider_thread_projection_rss_delta_stays_bounded");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");

    let (thread, probe) = run_with_rss_probe(|| {
        let tree = crate::ContextTree::open_file(&path, None).expect("open context tree");
        match tree
            .project(crate::ProjectionRequest::ProviderThread {
                options: crate::provider_thread::ProviderThreadOptions::default(),
            })
            .expect("provider projection")
        {
            crate::ProjectionOutput::ProviderThread(thread) => thread,
            other => panic!("unexpected projection output: {other:?}"),
        }
    });
    assert!(
        thread["messages"]
            .as_array()
            .is_some_and(|messages| !messages.is_empty())
    );
    assert_rss_delta_under("medium provider-thread projection", probe, 96 * 1024 * 1024);

    let _ = fs::remove_file(path);
}

#[test]
fn medium_selector_window_rss_delta_stays_bounded() {
    run_rss_child_test(
        "file_format::tests::rss_child_medium_selector_window_rss_delta_stays_bounded",
    );
}

#[test]
#[ignore = "invoked by medium_selector_window_rss_delta_stays_bounded in an isolated child process"]
fn rss_child_medium_selector_window_rss_delta_stays_bounded() {
    if !should_run_rss_child() {
        return;
    }
    let path = temp_path("medium_selector_window_rss_delta_stays_bounded");
    write_corpus_selector_large_stream_fixture(&path, 120);

    let mut query = PactFileSelectorQuery::new("@history .text_content");
    query.limit = 3;
    let (result_json, probe) =
        run_with_rss_probe(|| file_select_json(&path, query).expect("select"));
    let result: PactFileSelectorResult = serde_json::from_str(&result_json).expect("result json");
    assert_eq!(result.hits.len(), 3);
    assert_rss_delta_under("medium selector window", probe, 96 * 1024 * 1024);

    let _ = fs::remove_file(path);
}

#[test]
fn medium_component_id_hydration_rss_delta_stays_bounded() {
    run_rss_child_test(
        "file_format::tests::rss_child_medium_component_id_hydration_rss_delta_stays_bounded",
    );
}

#[test]
#[ignore = "invoked by medium_component_id_hydration_rss_delta_stays_bounded in an isolated child process"]
fn rss_child_medium_component_id_hydration_rss_delta_stays_bounded() {
    if !should_run_rss_child() {
        return;
    }
    let path = temp_path("medium_component_id_hydration_rss_delta_stays_bounded");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");

    let requested = vec!["text_10".to_string(), "missing_component".to_string()];
    let (hydrated_json, probe) = run_with_rss_probe(|| {
        file_hydrate_components_json(&path, &requested, Some("main"), None).expect("hydrate")
    });
    let hydrated: Value = serde_json::from_str(&hydrated_json).expect("hydrated json");
    assert_eq!(hydrated["found_component_ids"], json!(["text_10"]));
    assert_eq!(
        hydrated["missing_component_ids"],
        json!(["missing_component"])
    );
    assert_rss_delta_under("medium component-id hydration", probe, 64 * 1024 * 1024);

    let _ = fs::remove_file(path);
}

#[test]
#[ignore = "manual RSS reporter for large projection/query/provider-thread reads"]
fn large_projection_rss_reporter() {
    let path = temp_path("large_projection_rss_reporter");
    write_corpus_selector_large_stream_fixture(&path, 1_000);
    let file_bytes = fs::metadata(&path).expect("metadata").len();

    let (projection_json, projection_probe) = run_with_rss_probe(|| {
        file_projection_json(&path, Some("main"), None, 96, None, None).expect("projection")
    });
    let (provider_thread, provider_probe) = run_with_rss_probe(|| {
        let tree = crate::ContextTree::open_file(&path, None).expect("open context tree");
        match tree
            .project(crate::ProjectionRequest::ProviderThread {
                options: crate::provider_thread::ProviderThreadOptions::default(),
            })
            .expect("provider projection")
        {
            crate::ProjectionOutput::ProviderThread(thread) => thread,
            other => panic!("unexpected projection output: {other:?}"),
        }
    });
    let mut query = PactFileSelectorQuery::new("@history .text_content");
    query.limit = 128;
    let (query_result, query_probe) =
        run_with_rss_probe(|| file_select_json(&path, query).expect("select"));
    let requested = vec!["flurry_node_1001".to_string()];
    let (hydration_result, hydration_probe) = run_with_rss_probe(|| {
        file_hydrate_components_json(&path, &requested, Some("main"), None).expect("hydrate")
    });
    let projection_bytes = projection_json.len();
    let provider_thread_bytes = serde_json::to_vec(&provider_thread)
        .expect("provider json")
        .len();
    let query_result_bytes = query_result.len();
    let hydration_result_bytes = hydration_result.len();
    let (projection_peak_rss, projection_delta_rss) = rss_peak_and_delta(&projection_probe);
    let (provider_peak_rss, provider_delta_rss) = rss_peak_and_delta(&provider_probe);
    let (query_peak_rss, query_delta_rss) = rss_peak_and_delta(&query_probe);
    let (hydration_peak_rss, hydration_delta_rss) = rss_peak_and_delta(&hydration_probe);

    eprintln!(
        "large projection RSS report: file_bytes={file_bytes} projection_bytes={projection_bytes} provider_thread_bytes={provider_thread_bytes} query_result_bytes={query_result_bytes} hydration_result_bytes={hydration_result_bytes} projection_peak_rss={projection_peak_rss} projection_delta_rss={projection_delta_rss} provider_peak_rss={provider_peak_rss} provider_delta_rss={provider_delta_rss} query_peak_rss={query_peak_rss} query_delta_rss={query_delta_rss} hydration_peak_rss={hydration_peak_rss} hydration_delta_rss={hydration_delta_rss}"
    );

    assert!(projection_bytes > 0);
    assert!(provider_thread_bytes > 0);
    assert!(query_result_bytes > 0);
    assert!(hydration_result_bytes > 0);
    let _ = fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn pact_jsonl_gzip_comparison_gates() {
    let path = temp_path("pact_jsonl_gzip_comparison_gates");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");
    let bench_json = file_bench_storage_json(&path).expect("bench");
    let bench: Value = serde_json::from_str(&bench_json).expect("bench json");
    let comparison = &bench["jsonl_comparison"];
    let raw_ratio = comparison["ratio_pact_to_full_state_jsonl_gzip"]
        .as_f64()
        .expect("raw ratio");
    let archive_ratio = comparison["ratio_pact_archive_zstd_to_full_state_jsonl_gzip"]
        .as_f64()
        .expect("archive ratio");

    assert!(
        raw_ratio <= 1.30,
        "raw pact/full-state jsonl.gz ratio: {raw_ratio}"
    );
    assert!(
        archive_ratio <= 0.80,
        "semantic archive/full-state jsonl.gz ratio: {archive_ratio}"
    );

    let _ = fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
fn dedup_stress_output() -> String {
    let mut state = 0x5eed_1234_89ab_cdef_u64;
    let mut out = String::with_capacity(128 * 1024);
    for index in 0..2048 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let hash_a = state;
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let hash_b = state;
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let duration = state % 100_000;
        out.push_str(&format!(
                "[{index:04}] {hash_a:016x}{hash_b:016x} /tmp/build/{:02}/{:08x}.rs status={} duration={duration}\n",
                index % 97,
                (hash_a ^ hash_b) as u32,
                index % 5
            ));
    }
    out
}

#[cfg(feature = "compression-zstd")]
#[test]
fn repeated_output_pact_beats_jsonl_gzip() {
    let repeated = dedup_stress_output();
    let children = (0..16)
        .map(|index| {
            json!({
                "id": format!("result_text_{index}"),
                "componentType": "text_content",
                "type": "text_content",
                "content": repeated
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let equivalent_jsonl_gzip = gzip_len(serde_json::to_vec(&tree).expect("tree json").as_slice())
        .expect("equivalent gzip");
    let root = new_ledger_json(&tree.to_string(), Some("main"), 64).expect("repeated root store");
    let path = temp_path("repeated_output_pact_beats_jsonl_gzip");
    file_create_from_ledger_json(&path, &root, 64).expect("create");
    let pact_bytes = fs::metadata(&path).expect("metadata").len() as usize;
    let scanned = scan_file(&fs::read(&path).expect("read"), true);
    let archive_bytes = semantic_archive_zstd_bytes(&scanned.records).expect("archive bytes");

    assert!(
        pact_bytes <= equivalent_jsonl_gzip * 70 / 100,
        "pact={pact_bytes}, equivalent jsonl.gz={equivalent_jsonl_gzip}"
    );
    assert!(
        archive_bytes <= equivalent_jsonl_gzip / 2,
        "archive={archive_bytes}, equivalent jsonl.gz={equivalent_jsonl_gzip}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn repeated_large_output_growth_is_ref_bounded() {
    let repeated = "same generated output line\n".repeat(512);
    let children = (0..16)
        .map(|index| {
            json!({
                "id": format!("result_text_{index}"),
                "componentType": "text_content",
                "type": "text_content",
                "content": repeated.clone()
            })
        })
        .collect::<Vec<_>>();
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": children,
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 64).expect("repeated root store");
    let root_value: Value = serde_json::from_str(&root).expect("root json");
    let content_refs = root_value["branches"]["main"]["current"]["content_refs"]
        .as_object()
        .expect("content refs");

    assert_eq!(
        content_refs.len(),
        1,
        "repeated large output should share one content ref"
    );

    let path = temp_path("repeated_large_output_growth_is_ref_bounded");
    file_create_from_ledger_json(&path, &root, 64).expect("create");
    let bytes = fs::metadata(&path).expect("metadata").len() as usize;
    assert!(
        bytes < repeated.len() * 4,
        "hot file should grow by ref metadata, not 16 full repeated bodies: {bytes}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn heavy_repeated_output_uses_refs_not_inline_bodies() {
    repeated_large_output_growth_is_ref_bounded();
}

#[test]
fn medium_agentic_hot_pact_is_under_100kb() {
    let path = temp_path("medium_agentic_hot_pact_is_under_100kb");
    let root = medium_agentic_root_store();
    file_create_from_ledger_json(&path, &root, 96).expect("create");

    let bytes = fs::metadata(&path).expect("metadata").len();
    assert!(bytes <= 100 * 1024, "hot pact bytes: {bytes}");

    let _ = fs::remove_file(path);
}

#[test]
fn content_store_hash_index_is_rebuilt_not_serialized() {
    let store_json =
        content_store_put_inline_json(None, "text", "same body", 20).expect("put first");
    assert!(!store_json.contains("records_by_hash"));

    let store_json = content_store_put_inline_json(Some(&store_json), "text", "same body", 20)
        .expect("put duplicate");
    let store: crate::store::ContentStore =
        serde_json::from_str(&store_json).expect("content store");

    assert_eq!(store.records.len(), 1);
    assert!(!store_json.contains("records_by_hash"));
}

#[test]
fn content_hash_scheme_round_trips_and_validates() {
    let store_json =
        content_store_put_inline_json(None, "text", "same body", 20).expect("put first");
    let store: crate::store::ContentStore =
        serde_json::from_str(&store_json).expect("content store");
    let record = store.records.values().next().expect("content record");
    assert_eq!(record.hash.len(), 32);

    let path = temp_path("content_hash_scheme_round_trips_and_validates");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let validation_json = file_validate_json(&path).expect("validate");
    let validation: PactFileValidation =
        serde_json::from_str(&validation_json).expect("validation json");
    assert!(validation.ok);

    let errors = validate_semantic_records(&[ScannedRecord {
        kind: "content".to_string(),
        payload: json!({
            "id": "cnt_old",
            "kind": "text",
            "hash": "0123456789abcdef",
            "bytes": 0,
        }),
        payload_bytes: 0,
        sequence: 1,
    }]);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("expected blake3-128"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn projects_and_repairs_pact_file() {
    let path = temp_path("projects_and_repairs_pact_file");
    let repaired_path = temp_path("projects_and_repairs_pact_file_repaired");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let projected = file_project_tree_json(&path, Some("main"), Some(1)).expect("project");
    let tree: Value = serde_json::from_str(&projected).expect("tree");
    assert!(projected.contains("later"));
    assert_eq!(tree["id"], "root");

    let repair_json = file_repair_json(&path, &repaired_path, false).expect("repair");
    let repair: PactFileRepairReport = serde_json::from_str(&repair_json).expect("repair json");
    assert!(repair.source_validation.ok);
    assert!(repair.repaired_validation.ok);
    assert!(repair.rebuilt_indexes);
    assert!(
        repair
            .repaired_validation
            .manifest
            .capabilities
            .contains(&"is_healed".to_string())
    );
    assert!(!repair.truncated_tail);
    let reprojected =
        file_project_tree_json(&repaired_path, Some("main"), None).expect("reproject");
    assert!(reprojected.contains("later"));

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(repaired_path);
}

#[test]
fn hydrate_components_includes_parent_chain_and_hydrated_content() {
    let path = temp_path("hydrate_components_includes_parent_chain_and_hydrated_content");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let component_ids = vec!["text_1".to_string()];
    let projected = file_hydrate_components_json(&path, &component_ids, Some("main"), Some(1))
        .expect("components");
    let result: Value = serde_json::from_str(&projected).expect("result");

    assert_eq!(result["schema"], "pact.file.v0.hydrate_components.v1");
    assert_eq!(result["found_component_ids"], json!(["text_1"]));
    assert_eq!(result["missing_component_ids"], json!([]));
    assert_eq!(result["tree"]["id"], "root");

    let turn = &result["tree"]["children"][0];
    assert_eq!(turn["id"], "turn_1");
    assert_eq!(turn["children"].as_array().expect("children").len(), 1);

    let text = &turn["children"][0];
    assert_eq!(text["id"], "text_1");
    assert_eq!(text["content"], "hello from a compact pact file");
    assert!(text.get("content_ref").is_none());

    let _ = fs::remove_file(path);
}

#[test]
fn hydrate_components_keeps_requested_parent_subtree_and_reports_missing_ids() {
    let path =
        temp_path("hydrate_components_keeps_requested_parent_subtree_and_reports_missing_ids");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let component_ids = vec!["turn_1".to_string(), "missing_node".to_string()];
    let projected = file_hydrate_components_json(&path, &component_ids, Some("main"), Some(1))
        .expect("components");
    let result: Value = serde_json::from_str(&projected).expect("result");

    assert_eq!(result["found_component_ids"], json!(["turn_1"]));
    assert_eq!(result["missing_component_ids"], json!(["missing_node"]));

    let turn = &result["tree"]["children"][0];
    assert_eq!(turn["id"], "turn_1");
    assert!(find_node_by_id(turn, "text_1").is_some());
    assert!(find_node_by_id(turn, "tool_call").is_some());
    assert!(find_node_by_id(turn, "tool_result").is_some());
    assert!(find_node_by_id(turn, "text_2").is_some());

    let _ = fs::remove_file(path);
}

#[test]
fn append_view_records_does_not_rewrite_full_file() {
    let path = temp_path("append_view_records_does_not_rewrite_full_file");
    let root = sample_root_store();
    let forked =
        ledger_create_branch_at_revision_json(&root, "main", Some(1), Some("branch_view"), None, 8)
            .expect("fork view branch");
    file_create_from_ledger_json(&path, &forked, 8).expect("create");
    let before = fs::read(&path).expect("read before");

    file_append_view_records_json(&path, &sample_view_records(), 8).expect("append views");
    let after = fs::read(&path).expect("read after");

    assert!(after.starts_with(&before));
    assert!(after.len() < before.len() + 4096);
    let view_json = file_query_json(&path, "view", Some("view_alpha")).expect("view query");
    let view: PactFileQueryResult = serde_json::from_str(&view_json).expect("view json");
    assert_eq!(view.matches.len(), 1);

    let _ = fs::remove_file(path);
}

#[test]
fn manifest_only_info_does_not_collect_records() {
    let path = temp_path("manifest_only_info_does_not_collect_records");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let info_json = file_manifest_info_json(&path).expect("manifest info");
    let info: Value = serde_json::from_str(&info_json).expect("manifest info json");
    assert_eq!(info["records_collected"], 0);
    assert!(info["manifest"]["record_count"].as_u64().unwrap_or(0) > 0);
    assert!(info["pages"].is_object());

    let _ = fs::remove_file(path);
}

#[test]
fn inspect_summary_skips_page_payload_decode() {
    let path = temp_path("inspect_summary_skips_page_payload_decode");
    let text = "large page payload ".repeat(8192);
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "message",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "assistant",
            "children": [{
                "id": "text",
                "componentType": "text_content",
                "type": "text_content",
                "content": text,
            }],
        }],
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let manifest_json = file_inspect_json(&path).expect("inspect");
    let manifest: PactFileManifest = serde_json::from_str(&manifest_json).expect("manifest");
    assert_eq!(
        manifest.record_count,
        manifest
            .pages
            .iter()
            .map(|page| page.record_count)
            .sum::<usize>()
    );
    assert!(
        manifest
            .pages
            .iter()
            .filter(|page| page.page_type == "content")
            .all(|page| page.decoded_payload_bytes == 0)
    );

    let _ = fs::remove_file(path);
}

#[test]
fn content_manifest_reuses_chunks_and_reconstructs_payload() {
    let path = temp_path("content_manifest_reuses_chunks_and_reconstructs_payload");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let first = json!({
        "manifest_id": "mf_first",
        "kind": "generic.payload",
        "chunks": [
            {"key": "a", "text": "alpha"},
            {"key": "b", "text": "beta"}
        ]
    });
    let first_report_json =
        file_append_content_manifest_json(&path, &first.to_string()).expect("first manifest");
    let first_report: Value = serde_json::from_str(&first_report_json).expect("first report json");
    assert_eq!(first_report["new_content_count"], 2);
    assert_eq!(first_report["reused_chunk_count"], 0);

    let second = json!({
        "manifest_id": "mf_second",
        "kind": "generic.payload",
        "chunks": [
            {"key": "a", "text": "alpha"},
            {"key": "b", "text": "beta"},
            {"key": "c", "text": "gamma"}
        ]
    });
    let second_report_json =
        file_append_content_manifest_json(&path, &second.to_string()).expect("second manifest");
    let second_report: Value =
        serde_json::from_str(&second_report_json).expect("second report json");
    assert_eq!(second_report["new_content_count"], 1);
    assert_eq!(second_report["reused_chunk_count"], 2);

    let payload_json =
        file_content_manifest_payload_json(&path, "mf_second").expect("manifest payload");
    let payload: Value = serde_json::from_str(&payload_json).expect("payload json");
    assert_eq!(payload["text"], "alphabetagamma");

    let info_json = file_info_json(&path).expect("info");
    let info: Value = serde_json::from_str(&info_json).expect("info json");
    assert_eq!(info["content"]["manifest_count"], 2);
    assert_eq!(info["content"]["manifest_chunk_refs"], 5);

    let _ = fs::remove_file(path);
}

#[test]
fn near_duplicate_content_reuses_chunk_refs() {
    content_manifest_reuses_chunks_and_reconstructs_payload();
}

#[test]
fn export_full_preserves_content_manifests() {
    let path = temp_path("export_full_preserves_content_manifests");
    let full_path = temp_path("export_full_preserves_content_manifests_full");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let manifest = json!({
        "manifest_id": "mf_export",
        "kind": "generic.payload",
        "chunks": [
            {"key": "left", "text": "left-"},
            {"key": "right", "text": "right"}
        ]
    });
    file_append_content_manifest_json(&path, &manifest.to_string()).expect("append manifest");
    let export_json = file_bundle_export_full_json(&path, &full_path).expect("export full");
    let export: Value = serde_json::from_str(&export_json).expect("export json");
    assert!(
        export["preserved_file_level_content_records"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );

    let payload_json =
        file_content_manifest_payload_json(&full_path, "mf_export").expect("full payload");
    let payload: Value = serde_json::from_str(&payload_json).expect("payload json");
    assert_eq!(payload["text"], "left-right");

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(full_path);
}

#[test]
fn hot_save_does_not_create_portable_bundle_sidecar() {
    let path = temp_path("hot_save_does_not_create_portable_bundle_sidecar");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");
    let mut obsolete_sidecar = path.clone();
    obsolete_sidecar.set_extension("pact.bundle");

    assert!(!obsolete_sidecar.exists());

    let _ = fs::remove_file(path);
}

#[test]
fn export_full_inlines_reachable_content_refs() {
    let path = temp_path("export_full_inlines_reachable_content_refs");
    let full_path = temp_path("export_full_inlines_reachable_content_refs_full");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let export_json = file_bundle_export_full_json(&path, &full_path).expect("export full");
    let export: Value = serde_json::from_str(&export_json).expect("export json");
    assert!(export["output_file_bytes"].as_u64().unwrap_or_default() > 0);

    let current = file_project_head_json(&full_path).expect("project full");
    assert!(current.contains("hello from a compact pact file"));

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(full_path);
}

#[test]
fn slices_and_import_checks_pact_file() {
    let path = temp_path("slices_and_import_checks_pact_file");
    let slice_path = temp_path("slices_and_import_checks_pact_file_slice");
    let root = sample_root_store();
    file_create_from_ledger_json(&path, &root, 8).expect("create");

    let spec = r#"{"mode":"branch-with-lineage","branch_id":"main"}"#;
    let slice_json = file_slice_json(&path, &slice_path, spec).expect("slice");
    let validation: PactFileValidation = serde_json::from_str(&slice_json).expect("validation");
    assert!(validation.ok);
    assert_eq!(validation.latest_valid_branch_heads["main"], 1);

    let import_json = file_import_check_json(&slice_path).expect("import check");
    let import: PactFileValidation = serde_json::from_str(&import_json).expect("import json");
    assert!(import.ok);
    assert_eq!(import.manifest.file_kind, "slice");
    assert!(import.manifest.capabilities.contains(&"slice".to_string()));
    assert!(
        import
            .manifest
            .capabilities
            .contains(&"slice_open".to_string())
    );
    let projected = file_project_tree_json(&slice_path, Some("main"), None).expect("slice project");
    assert!(projected.contains("later"));

    let projection = file_open_json(&slice_path, Some("main"), None, 8).expect("slice open");
    assert!(projection.contains(PACT_FILE_PROJECTION_SCHEMA));

    let import_report_json = file_import_json(&path, &slice_path).expect("import slice");
    let import_report: PactFileImportReport =
        serde_json::from_str(&import_report_json).expect("import report");
    assert!(import_report.applied);
    assert!(
        import_report
            .imported_branch_ids
            .iter()
            .any(|branch_id| branch_id.starts_with("imported_main"))
    );

    let merge_report_json = file_merge_json(&path, &slice_path).expect("merge slice");
    let merge_report: PactFileImportReport =
        serde_json::from_str(&merge_report_json).expect("merge report");
    assert!(merge_report.applied);
    assert!(!merge_report.imported_branch_ids.is_empty());

    let _ = fs::remove_file(path);
    let _ = fs::remove_file(slice_path);
}





fn insert_text_commit_cycle_attr(
    cycle: u64,
    revision: u64,
    component_id: &str,
    content: &str,
) -> (String, String) {
    let node = json!({
        "id": component_id,
        "componentType": "text_content",
        "type": "text_content",
        "offset": revision as i64,
        "content": content,
        "aux": {"cycle": cycle},
    });
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle,
        revision,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: component_id.to_string(),
            position: Some(revision as i64),
        }],
    };
    (
        serde_json::to_string(&commit).expect("commit json"),
        serde_json::to_string(&vec![node]).expect("nodes json"),
    )
}



fn write_corpus_selector_fixture(path: &Path) {
    write_commit_fixture(path);
    let entries = [
        (
            2u64,
            2u64,
            "corpus_text_2",
            "cycle two: shared needle present",
        ),
        (3, 3, "corpus_text_3", "cycle three: only here"),
        (4, 4, "corpus_text_4", "cycle four: shared needle again"),
        (5, 5, "corpus_text_5", "cycle five: final line"),
    ];
    for (cycle, revision, component_id, content) in entries {
        let (commit, nodes) = insert_text_commit_cycle_attr(cycle, revision, component_id, content);
        file_append_commit_json(path, &commit, &nodes).expect("append commit");
    }
}

#[test]
fn temporal_selector_parses_current_previous_absolute_and_ranges() {
    let path = temp_path("temporal_selector_parses_basics");
    write_corpus_selector_fixture(&path);

    for selector in [
        "@t0 .text_content",
        "@t-1 .text_content",
        "@t-5..@t-1 .text_content",
        "@c3 .text_content",
        "@c3..@c5 .text_content",
        "@history .text_content",
        "@* .text_content",
    ] {
        let result_json = file_select_json(&path, PactFileSelectorQuery::new(selector))
            .unwrap_or_else(|err| panic!("selector {selector:?} should parse: {err}"));
        let result: PactFileSelectorResult =
            serde_json::from_str(&result_json).expect("selector json");
        assert!(!result.temporal.raw.is_empty(), "raw empty for {selector}");
    }

    let _ = fs::remove_file(path);
}

#[test]
fn temporal_selector_rejects_ambiguous_or_descending_ranges() {
    let path = temp_path("temporal_selector_rejects_ambiguous");
    write_corpus_selector_fixture(&path);

    let err = file_select_json(&path, PactFileSelectorQuery::new("@c1..@t-1 .text_content"))
        .expect_err("mixed axes range must error");
    assert!(
        err.to_string().contains("mixes axes"),
        "expected mixed-axes error, got {err:?}"
    );

    let err = file_select_json(&path, PactFileSelectorQuery::new("@t1 .text_content"))
        .expect_err("positive @t alias must error");
    assert!(
        err.to_string()
            .contains("positive relative temporal aliases")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_streams_history_newest_first() {
    let path = temp_path("corpus_selector_streams_history_newest_first");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(&path, PactFileSelectorQuery::new("@history .text_content"))
        .expect("history selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");

    assert_eq!(result.source, "stream");
    assert!(!result.fallback_used);
    let stats = result.selector_stats.expect("selector_stats");
    assert!(!stats.fallback_used);
    let cycles: Vec<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert!(
        cycles.len() >= 4,
        "expected multi-cycle hits, got {cycles:?}"
    );

    for window in cycles.windows(2) {
        assert!(
            window[0] >= window[1],
            "expected newest-first, got {cycles:?}"
        );
    }
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_filters_absolute_cycle_range() {
    let path = temp_path("corpus_selector_filters_absolute_cycle_range");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(&path, PactFileSelectorQuery::new("@c2..@c3 .text_content"))
        .expect("cycle range");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");

    let cycles: BTreeSet<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert_eq!(cycles, BTreeSet::from([2, 3]));
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_filters_by_relative_snapshot_range() {
    let path = temp_path("corpus_selector_filters_by_relative_snapshot_range");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(
        &path,
        PactFileSelectorQuery::new("@t-3..@t-1 .text_content"),
    )
    .expect("relative range");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");


    let cycles: BTreeSet<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert!(
        cycles.iter().all(|c| (2u64..=4u64).contains(c)),
        "expected sealed cycles 2..=4, got {cycles:?}"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_supports_node_local_structural_filters() {
    let path = temp_path("corpus_selector_supports_node_local_structural_filters");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(&path, PactFileSelectorQuery::new("@history #corpus_text_3"))
        .expect("id selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].component_id, "corpus_text_3");
    assert_eq!(result.source, "stream");
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_supports_descendant_and_has_selectors() {
    let path = temp_path("corpus_selector_supports_descendant_and_has_selectors");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(
        &path,
        PactFileSelectorQuery::new("@history .message_turn > .text_content"),
    )
    .expect("descendant selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");
    assert_eq!(result.source, "snapshot_window");
    assert!(!result.hits.is_empty(), "expected matches under turn_1");
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.element_kind == "text_content"),
        "all hits should be text_content"
    );

    let json = file_select_json(
        &path,
        PactFileSelectorQuery::new("@history .message_turn:has(.text_content)"),
    )
    .expect(":has selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect(":has json");
    assert_eq!(result.source, "snapshot_window");
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.component_id == "turn_1" && hit.element_kind == "message_turn"),
        "expected turn_1 message_turn hit"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_supports_coordinate_and_depth_selectors() {
    let path = temp_path("corpus_selector_supports_coordinate_and_depth_selectors");
    write_corpus_selector_fixture(&path);

    let json =
        file_select_json(&path, PactFileSelectorQuery::new("@history d0")).expect("depth selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");
    assert!(
        result.hits.iter().any(|hit| hit.component_id == "turn_1"),
        "expected turn_1 at depth 0, got {:?}",
        result.hits
    );
    let _ = fs::remove_file(path);
}

#[test]
fn text_search_uses_temporal_corpus_selector_filter() {
    let path = temp_path("text_search_uses_temporal_corpus_selector_filter");
    write_corpus_selector_fixture(&path);

    let mut cycle_2_query = PactTextSearchQuery::new("shared needle");
    cycle_2_query.selector = Some("@c2 .text_content".to_string());
    let json = file_text_search_json(&path, cycle_2_query).expect("cycle-2 search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("result json");
    let cycles: BTreeSet<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert_eq!(cycles, BTreeSet::from([2]));
    assert!(result.source.starts_with("selector_stream"));

    let mut history_query = PactTextSearchQuery::new("shared needle");
    history_query.selector = Some("@history .text_content".to_string());
    let json = file_text_search_json(&path, history_query).expect("history search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("history result");
    let cycles: Vec<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert!(cycles.contains(&4) && cycles.contains(&2));

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_selector_finds_nonresident_archive_hits() {
    let path = temp_path("text_search_selector_finds_nonresident_archive_hits");
    write_corpus_selector_fixture(&path);



    let delete = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 6,
        revision: 6,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::DeleteComponent {
            component_id: "corpus_text_3".to_string(),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&delete).expect("delete json"),
        "[]",
    )
    .expect("append delete");

    let mut query = PactTextSearchQuery::new("only here");
    query.selector = Some("@history #corpus_text_3".to_string());
    let json = file_text_search_json(&path, query).expect("history archive search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("result json");

    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].component_id, "corpus_text_3");
    assert!(!result.hits[0].visibility.resident_in_current_surface);
    assert!(result.hits[0].visibility.archived_in_pact);

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_stream_short_circuits_after_limit() {
    let path = temp_path("text_search_stream_short_circuits_after_limit");
    write_corpus_selector_fixture(&path);

    for cycle in 6u64..=20u64 {
        let (commit, nodes) = insert_text_commit_cycle_attr(
            cycle,
            cycle,
            &format!("flurry_{cycle}"),
            "limit phrase appears many times",
        );
        file_append_commit_json(&path, &commit, &nodes).expect("append flurry");
    }

    let mut query = PactTextSearchQuery::new("limit phrase");
    query.selector = Some("@history .text_content".to_string());
    query.limit = 2;
    let json = file_text_search_json(&path, query).expect("limit search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("result json");
    assert_eq!(result.hits.len(), 2);
    let stats = result.selector_stats.expect("selector_stats");

    assert!(
        stats.payloads_touched <= 10,
        "short-circuit failed: payloads_touched={}, hits={}",
        stats.payloads_touched,
        result.hits.len()
    );

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_applies_temporal_gate_before_payload_lookup() {
    let path = temp_path("text_search_applies_temporal_gate_before_payload_lookup");
    write_corpus_selector_fixture(&path);

    let mut query = PactTextSearchQuery::new("only here");
    query.selector = Some("@c3 .text_content".to_string());
    let json = file_text_search_json(&path, query).expect("temporal gate search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("result json");
    let stats = result.selector_stats.expect("selector_stats");
    assert!(
        stats.records_temporal_rejected > 0,
        "expected temporal gate to reject out-of-range records, stats={stats:?}"
    );
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].component_id, "corpus_text_3");

    let _ = fs::remove_file(path);
}

#[test]
fn text_search_reports_selector_stream_source_metadata() {
    let path = temp_path("text_search_reports_selector_stream_source_metadata");
    write_corpus_selector_fixture(&path);

    let mut node_local = PactTextSearchQuery::new("shared needle");
    node_local.selector = Some("@history .text_content".to_string());
    let node_local_json = file_text_search_json(&path, node_local).expect("node-local search");
    let node_local_result: PactTextSearchResult =
        serde_json::from_str(&node_local_json).expect("node-local result");
    assert_eq!(node_local_result.source, "selector_stream:stream");
    assert!(!node_local_result.fallback_used);

    let mut structural = PactTextSearchQuery::new("shared needle");
    structural.selector = Some("@history .message_turn > .text_content".to_string());
    let structural_json = file_text_search_json(&path, structural).expect("structural search");
    let structural_result: PactTextSearchResult =
        serde_json::from_str(&structural_json).expect("structural result");
    assert_eq!(structural_result.source, "selector_stream:snapshot_window");
    assert!(!structural_result.fallback_used);

    let _ = fs::remove_file(path);
}

#[test]
fn selector_hits_hydrate_components_with_parent_chain() {
    let path = temp_path("selector_hits_hydrate_components_with_parent_chain");
    write_corpus_selector_fixture(&path);

    let json = file_select_json(&path, PactFileSelectorQuery::new("@history #corpus_text_3"))
        .expect("history selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("result json");
    assert_eq!(result.hits.len(), 1);
    let hit_component_id = result.hits[0].component_id.clone();

    let projected =
        file_hydrate_components_json(&path, std::slice::from_ref(&hit_component_id), None, None)
            .expect("hydrate hit");
    let value: Value = serde_json::from_str(&projected).expect("hydrate json");
    let found_ids = value
        .get("found_component_ids")
        .and_then(Value::as_array)
        .expect("found_component_ids array");
    assert!(
        found_ids
            .iter()
            .any(|id| id.as_str() == Some(hit_component_id.as_str())),
        "expected hydrate_components to find {hit_component_id}, got {found_ids:?}"
    );
    let tree = value.get("tree").expect("tree");
    assert!(
        find_node_by_id(tree, &hit_component_id).is_some(),
        "hydrated tree did not contain selected component {hit_component_id}"
    );
    let _ = fs::remove_file(path);
}





fn write_corpus_selector_revision_fixture(path: &Path) {
    write_commit_fixture(path);
    let mut current_offset = 1i64;
    let entries = [
        (1u64, 1u64, "old literal"),
        (2, 2, "middle literal"),
        (3, 3, "new literal"),
    ];
    for (cycle, revision, content) in entries {
        current_offset += 1;
        let node = json!({
            "id": "stable_text",
            "componentType": "text_content",
            "type": "text_content",
            "offset": current_offset,
            "content": content,
            "aux": {"cycle": cycle},
        });
        let op = if cycle == 1 {
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "stable_text".to_string(),
                position: Some(current_offset),
            }
        } else {
            CommitRecordOp::ReplaceComponent {
                component_id: "stable_text".to_string(),
                node_ref: "stable_text".to_string(),
            }
        };
        let commit = CommitRecord {
            schema: COMMIT_RECORD_SCHEMA.to_string(),
            branch_id: "main".to_string(),
            cycle,
            revision,
            lifecycle_tick: None,
            ops: vec![op],
        };
        file_append_commit_json(
            path,
            &serde_json::to_string(&commit).expect("commit json"),
            &serde_json::to_string(&vec![node]).expect("nodes json"),
        )
        .expect("append commit");
    }
}

#[test]
fn corpus_selector_history_preserves_versions_of_current_node() {
    let path = temp_path("corpus_selector_history_preserves_versions_of_current_node");
    write_corpus_selector_revision_fixture(&path);

    let json = file_select_json(&path, PactFileSelectorQuery::new("@history #stable_text"))
        .expect("history selector");
    let result: PactFileSelectorResult = serde_json::from_str(&json).expect("json");

    let cycles: BTreeSet<u64> = result.hits.iter().filter_map(|hit| hit.cycle).collect();
    assert_eq!(cycles, BTreeSet::from([1, 2, 3]));
    assert_eq!(result.hits.len(), 3);
    let _ = fs::remove_file(path);
}

#[test]
fn text_search_history_finds_old_content_for_current_node_id() {
    let path = temp_path("text_search_history_finds_old_content_for_current_node_id");
    write_corpus_selector_revision_fixture(&path);

    let mut query = PactTextSearchQuery::new("old literal");
    query.selector = Some("@history #stable_text".to_string());
    let json = file_text_search_json(&path, query).expect("search");
    let result: PactTextSearchResult = serde_json::from_str(&json).expect("json");

    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].cycle, Some(1));
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_node_local_applies_selector_controls() {
    let path = temp_path("corpus_selector_node_local_applies_selector_controls");
    write_corpus_selector_fixture(&path);

    let result: PactFileSelectorResult = serde_json::from_str(
        &file_select_json(
            &path,
            PactFileSelectorQuery::new("@history .text_content @limit=1"),
        )
        .expect("select"),
    )
    .expect("json");
    assert_eq!(result.hits.len(), 1);

    let offset_result: PactFileSelectorResult = serde_json::from_str(
        &file_select_json(
            &path,
            PactFileSelectorQuery::new("@history .text_content @offset=1 @limit=1"),
        )
        .expect("select"),
    )
    .expect("json");
    assert_eq!(offset_result.hits.len(), 1);
    assert_ne!(
        result.hits[0].component_id,
        offset_result.hits[0].component_id
    );
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_snapshot_window_applies_selector_controls_once() {
    let path = temp_path("corpus_selector_snapshot_window_applies_selector_controls_once");
    write_corpus_selector_fixture(&path);

    let first: PactFileSelectorResult = serde_json::from_str(
        &file_select_json(
            &path,
            PactFileSelectorQuery::new("@history .message_turn > .text_content @limit=1"),
        )
        .expect("select first"),
    )
    .expect("first json");
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.source, "snapshot_window");

    let offset: PactFileSelectorResult = serde_json::from_str(
        &file_select_json(
            &path,
            PactFileSelectorQuery::new("@history .message_turn > .text_content @offset=1 @limit=1"),
        )
        .expect("select offset"),
    )
    .expect("offset json");
    assert_eq!(offset.hits.len(), 1);
    assert_eq!(offset.source, "snapshot_window");
    assert_ne!(first.hits[0].component_id, offset.hits[0].component_id);
    let _ = fs::remove_file(path);
}

fn write_corpus_selector_large_stream_fixture(path: &Path, count: u64) {
    write_commit_fixture(path);
    for index in 0..count {
        let cycle = index + 2;
        let (commit, nodes) = insert_text_commit_cycle_attr(
            cycle,
            cycle,
            &format!("flurry_node_{cycle}"),
            &format!("flurry content {cycle}"),
        );
        file_append_commit_json(path, &commit, &nodes).expect("append flurry");
    }
}

#[test]
fn corpus_selector_node_local_limit_does_not_scan_entire_corpus() {
    let path = temp_path("corpus_selector_node_local_limit_does_not_scan_entire_corpus");
    write_corpus_selector_large_stream_fixture(&path, 200);

    let mut query = PactFileSelectorQuery::new("@history .text_content");
    query.limit = 3;
    let result: PactFileSelectorResult =
        serde_json::from_str(&file_select_json(&path, query).expect("select")).expect("json");
    let stats = result.selector_stats.expect("stats");

    assert_eq!(result.hits.len(), 3);
    assert!(
        stats.records_seen < 50,
        "node-local stream scanned too much before satisfying limit: {stats:?}"
    );
    let _ = fs::remove_file(path);
}






fn write_corpus_selector_branch_fixture(path: &Path) {
    write_commit_fixture(path);
    for (cycle, revision, component_id, content) in [
        (2u64, 2u64, "main_text_2", "main cycle 2"),
        (3, 3, "main_text_3", "main cycle 3"),
    ] {
        let (commit, nodes) = insert_text_commit_cycle_attr(cycle, revision, component_id, content);
        file_append_commit_json(path, &commit, &nodes).expect("append main");
    }



    for (cycle, revision, component_id, content) in [
        (10u64, 10u64, "side_text_10", "side cycle 10"),
        (11, 11, "side_text_11", "side cycle 11"),
    ] {
        let node = json!({
            "id": component_id,
            "componentType": "text_content",
            "type": "text_content",
            "offset": revision as i64,
            "content": content,
            "aux": {"cycle": cycle},
        });
        let commit = CommitRecord {
            schema: COMMIT_RECORD_SCHEMA.to_string(),
            branch_id: "side".to_string(),
            cycle,
            revision,
            lifecycle_tick: None,
            ops: vec![CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: component_id.to_string(),
                position: Some(revision as i64),
            }],
        };
        file_append_commit_json(
            path,
            &serde_json::to_string(&commit).expect("commit json"),
            &serde_json::to_string(&vec![node]).expect("nodes json"),
        )
        .expect("append side");
    }
}

#[test]
fn corpus_selector_branch_option_drives_t0_and_relative_history() {
    let path = temp_path("corpus_selector_branch_option_drives_t0");
    write_corpus_selector_branch_fixture(&path);

    let mut t0_query = PactFileSelectorQuery::new("@t0 .text_content");
    t0_query.branch_id = Some("side".to_string());
    let result: PactFileSelectorResult =
        serde_json::from_str(&file_select_json(&path, t0_query).expect("select")).expect("json");
    assert_eq!(result.temporal.cycles, vec![11]);

    let mut main_query = PactFileSelectorQuery::new("@t0 .text_content");
    main_query.branch_id = Some("main".to_string());
    let result: PactFileSelectorResult =
        serde_json::from_str(&file_select_json(&path, main_query).expect("select")).expect("json");
    assert_eq!(result.temporal.cycles, vec![3]);

    let mut prev = PactFileSelectorQuery::new("@t-1 .text_content");
    prev.branch_id = Some("side".to_string());
    let result: PactFileSelectorResult =
        serde_json::from_str(&file_select_json(&path, prev).expect("select")).expect("json");
    assert_eq!(result.temporal.cycles, vec![10]);
    let _ = fs::remove_file(path);
}

#[test]
fn corpus_selector_structural_snapshot_returns_single_commit_nodes() {
    let path = temp_path("corpus_selector_structural_snapshot");
    write_commit_fixture(&path);




    let nodes = vec![
        json!({
            "id": "snap_text_a", "componentType": "text_content",
            "type": "text_content", "offset": 1, "content": "snap node a",
        }),
        json!({
            "id": "snap_text_b", "componentType": "text_content",
            "type": "text_content", "offset": 2, "content": "snap node b",
        }),
    ];
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 4,
        revision: 4,
        lifecycle_tick: None,
        ops: vec![
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "snap_text_a".to_string(),
                position: Some(1),
            },
            CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: "snap_text_b".to_string(),
                position: Some(2),
            },
        ],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &serde_json::to_string(&nodes).expect("nodes json"),
    )
    .expect("append snapshot commit");

    let query = PactFileSelectorQuery::new("@t0 .message_turn > .text_content");
    let result: PactFileSelectorResult =
        serde_json::from_str(&file_select_json(&path, query).expect("select")).expect("json");
    let component_ids: BTreeSet<String> = result
        .hits
        .iter()
        .map(|hit| hit.component_id.clone())
        .collect();

    assert_eq!(result.source, "snapshot_window");
    assert!(component_ids.contains("snap_text_a"));
    assert!(component_ids.contains("snap_text_b"));
    let _ = fs::remove_file(path);
}

#[test]
fn text_search_selector_controls_apply_before_text_matching() {
    let path = temp_path("text_search_selector_controls_apply_before_text_matching");
    write_corpus_selector_fixture(&path);





    let mut unconstrained = PactTextSearchQuery::new("shared needle");
    unconstrained.selector = Some("@history .text_content".to_string());
    unconstrained.limit = 10;
    let unconstrained_result: PactTextSearchResult = serde_json::from_str(
        &file_text_search_json(&path, unconstrained).expect("unconstrained search"),
    )
    .expect("json");

    let mut constrained = PactTextSearchQuery::new("shared needle");
    constrained.selector = Some("@history .text_content @limit=3".to_string());
    constrained.limit = 10;
    let constrained_result: PactTextSearchResult = serde_json::from_str(
        &file_text_search_json(&path, constrained).expect("constrained search"),
    )
    .expect("json");

    assert!(
        unconstrained_result.hits.len() >= 2,
        "expected baseline run to find both shared-needle cycles, got {:?}",
        unconstrained_result.hits
    );
    assert!(
        constrained_result.hits.len() < unconstrained_result.hits.len(),
        "selector @limit must shrink the candidate window seen by text search; \
         unconstrained={}, constrained={}",
        unconstrained_result.hits.len(),
        constrained_result.hits.len()
    );
    let _ = fs::remove_file(path);
}



fn capture_policy_root_store_tree() -> String {
    json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "type": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "attrs": {"cycle": 1},
            "children": [
                {
                    "id": "stored_node",
                    "componentType": "text_content",
                    "type": "text_content",
                    "attrs": {"cycle": 1},
                    "content": "STORED-PAYLOAD-VISIBLE"
                },
                {
                    "id": "ref_only_node",
                    "componentType": "text_content",
                    "type": "text_content",
                    "attrs": {"cycle": 1},
                    "content": "REFERENCE-ONLY-PAYLOAD",
                    "content_capture": "reference_only",
                    "source_ref": {
                        "kind": "file",
                        "path": "/project/app.py",
                        "byte_range": [0, 4096]
                    }
                },
                {
                    "id": "redacted_node",
                    "componentType": "text_content",
                    "type": "text_content",
                    "attrs": {"cycle": 1},
                    "content": "REDACTED-PAYLOAD",
                    "content_capture": "redacted"
                }
            ]
        }]
    })
    .to_string()
}

fn open_root_store_for_capture_test(path: &Path) -> crate::store::ContextTreeLedger {
    let scanned = scan_file(&fs::read(path).expect("read pact"), true);
    ledger_from_records(&scanned.records).expect("root store")
}

fn find_persisted_node(
    root_store: &crate::store::ContextTreeLedger,
    component_id: &str,
) -> crate::store::ComponentRecord {
    root_store
        .branches
        .values()
        .find_map(|branch| branch.current.component_record(component_id))
        .unwrap_or_else(|| panic!("node {component_id} not in any branch"))
}

#[test]
fn reference_only_node_does_not_emit_content_records() {
    let path = temp_path("reference_only_does_not_emit_content_records");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let bytes = fs::read(&path).expect("read");
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        !body.contains("REFERENCE-ONLY-PAYLOAD"),
        "reference_only content leaked into .pact"
    );
    assert!(
        !body.contains("REDACTED-PAYLOAD"),
        "redacted content leaked into .pact"
    );

    let scanned = scan_file(&bytes, true);
    for record in scanned
        .records
        .iter()
        .filter(|record| record.kind == "content")
    {
        let payload_text = record
            .payload
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| record.payload.get("preview_inline").and_then(Value::as_str))
            .unwrap_or_default();
        assert!(
            !payload_text.contains("REFERENCE-ONLY-PAYLOAD"),
            "content record leaked reference_only payload"
        );
        assert!(
            !payload_text.contains("REDACTED-PAYLOAD"),
            "content record leaked redacted payload"
        );
    }
    let _ = fs::remove_file(path);
}

#[test]
fn stored_node_still_emits_content_records() {
    let path = temp_path("stored_node_still_emits_content_records");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let projected_json = file_project_head_json(&path).expect("project");
    assert!(
        projected_json.contains("STORED-PAYLOAD-VISIBLE"),
        "stored content vanished from projection"
    );
    let _ = fs::remove_file(path);
}

#[test]
fn reference_only_node_projects_with_source_ref_but_without_content() {
    let path = temp_path("reference_only_projects_with_source_ref");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let projected_json = file_project_head_json(&path).expect("project");
    let value: Value = serde_json::from_str(&projected_json).expect("project json");
    let node = find_node_by_id(&value, "ref_only_node").expect("ref node present");
    assert!(
        node.get("content").is_none(),
        "reference_only node should have no content in projection, got {node:?}"
    );
    assert_eq!(node["content_capture"], "reference_only");
    assert!(node.get("content_status").is_none());
    assert_eq!(node["source_ref"]["kind"], "file");
    assert_eq!(node["source_ref"]["path"], "/project/app.py");
    let _ = fs::remove_file(path);
}

#[test]
fn redacted_node_projects_without_content_and_marks_status() {
    let path = temp_path("redacted_node_projects_without_content");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let projected_json = file_project_head_json(&path).expect("project");
    let value: Value = serde_json::from_str(&projected_json).expect("project json");
    let node = find_node_by_id(&value, "redacted_node").expect("redacted node present");
    assert!(
        node.get("content").is_none(),
        "redacted node should not surface content"
    );
    assert_eq!(node["content_capture"], "redacted");
    assert!(node.get("content_status").is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn commit_append_preserves_capture_policy_and_source_ref() {
    let path = temp_path("commit_append_preserves_capture_policy");
    write_commit_fixture(&path);

    let node = json!({
        "id": "appended_ref_only",
        "componentType": "text_content",
        "type": "text_content",
        "offset": 1,
        "content": "APPENDED-REFERENCE-ONLY",
        "content_capture": "reference_only",
        "source_ref": {"kind": "file", "path": "/x/y.py"},
        "attrs": {"cycle": 2}
    });
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 2,
        revision: 2,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::InsertComponent {
            parent_id: "turn_1".to_string(),
            node_ref: "appended_ref_only".to_string(),
            position: Some(1),
        }],
    };
    file_append_commit_json(
        &path,
        &serde_json::to_string(&commit).expect("commit json"),
        &serde_json::to_string(&vec![node]).expect("nodes json"),
    )
    .expect("append");

    let bytes = fs::read(&path).expect("read");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("APPENDED-REFERENCE-ONLY"),
        "commit-appended reference_only content leaked into .pact"
    );

    let projected_json = file_project_head_json(&path).expect("project");
    let value: Value = serde_json::from_str(&projected_json).expect("project json");
    let appended = find_node_by_id(&value, "appended_ref_only").expect("appended node");
    assert!(appended.get("content").is_none());
    assert_eq!(appended["content_capture"], "reference_only");
    assert_eq!(appended["source_ref"]["path"], "/x/y.py");
    let _ = fs::remove_file(path);
}

#[test]
fn repack_preserves_reference_only_provenance_without_capturing_content() {
    let source_path = temp_path("repack_preserves_reference_only_source");
    let target_path = temp_path("repack_preserves_reference_only_target");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&source_path, &root, 16).expect("create");

    file_repack_json(&source_path, &target_path).expect("repack");
    let repacked_bytes = fs::read(&target_path).expect("read repacked");
    assert!(
        !String::from_utf8_lossy(&repacked_bytes).contains("REFERENCE-ONLY-PAYLOAD"),
        "repack leaked reference_only payload"
    );

    let projected_json = file_project_head_json(&target_path).expect("project");
    let value: Value = serde_json::from_str(&projected_json).expect("project json");
    let node = find_node_by_id(&value, "ref_only_node").expect("ref_only node");
    assert_eq!(node["content_capture"], "reference_only");
    assert_eq!(node["source_ref"]["path"], "/project/app.py");
    assert!(node.get("content").is_none());

    let _ = fs::remove_file(source_path);
    let _ = fs::remove_file(target_path);
}

#[test]
fn slice_import_preserves_reference_only_provenance() {
    let source_path = temp_path("slice_import_preserves_reference_only_source");
    let slice_path = temp_path("slice_import_preserves_reference_only_slice");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&source_path, &root, 16).expect("create");

    let spec = PactFileSliceSpec {
        mode: PactFileSliceMode::BranchWithLineage,
        branch_id: Some("main".to_string()),
        start_transaction_id: None,
        end_transaction_id: None,
        start_cycle: None,
        end_cycle: None,
    };
    file_slice_json(
        &source_path,
        &slice_path,
        &serde_json::to_string(&spec).expect("slice spec"),
    )
    .expect("slice");

    let slice_bytes = fs::read(&slice_path).expect("read slice");
    assert!(
        !String::from_utf8_lossy(&slice_bytes).contains("REFERENCE-ONLY-PAYLOAD"),
        "slice leaked reference_only payload"
    );

    let scanned = scan_file(&slice_bytes, true);
    let root_store = ledger_from_records(&scanned.records).expect("slice root store");
    let node = find_persisted_node(&root_store, "ref_only_node");
    assert_eq!(
        node.content_capture,
        Some(crate::components::ContentCapturePolicy::ReferenceOnly)
    );
    assert!(node.content.is_none());
    assert_eq!(
        node.source_ref.as_ref().map(|s| s.kind.as_str()),
        Some("file")
    );

    let _ = fs::remove_file(source_path);
    let _ = fs::remove_file(slice_path);
}

#[test]
fn root_store_round_trips_capture_policy_and_source_ref() {
    let path = temp_path("root_store_round_trips_capture_policy");
    let tree = capture_policy_root_store_tree();
    let root = new_ledger_json(&tree, Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");

    let root_store = open_root_store_for_capture_test(&path);
    let ref_node = find_persisted_node(&root_store, "ref_only_node");
    assert_eq!(
        ref_node.content_capture,
        Some(crate::components::ContentCapturePolicy::ReferenceOnly)
    );
    assert!(ref_node.content.is_none());
    assert_eq!(
        ref_node.source_ref.as_ref().map(|s| s.kind.as_str()),
        Some("file")
    );

    let redacted = find_persisted_node(&root_store, "redacted_node");
    assert_eq!(
        redacted.content_capture,
        Some(crate::components::ContentCapturePolicy::Redacted)
    );
    assert!(redacted.content_status.is_none());
    assert!(redacted.content.is_none());

    let stored = find_persisted_node(&root_store, "stored_node");
    assert!(
        stored.content_capture.is_none()
            || stored.content_capture == Some(crate::components::ContentCapturePolicy::Stored),
        "stored node should not carry a non-stored capture policy"
    );

    let _ = fs::remove_file(path);
}

fn inline_content_map(content: &crate::store::ContentStore) -> BTreeMap<String, String> {
    content
        .records
        .values()
        .filter_map(|record| {
            record
                .storage
                .inline
                .as_ref()
                .map(|text| (record.id.clone(), text.clone()))
        })
        .collect()
}

#[test]
fn file_diff_in_place_update_emits_deltas_not_replace_component() {


    let old = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "the quick brown fox",
              "aux": { "stale": 1, "keep": 2 } }
        ]
    });
    let new = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "the quick red fox",
              "aux": { "keep": 2, "fresh": 3 } }
        ]
    });

    let mut ops = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut content = crate::store::ContentStore::default();
    super::history::diff_value_trees(&old, &new, &mut ops, &mut nodes, &mut content).expect("diff");

    assert!(
        ops.iter()
            .all(|op| !matches!(op, CommitRecordOp::ReplaceComponent { .. })),
        "in-place change must not emit ReplaceComponent: {ops:?}"
    );
    assert!(
        nodes.is_empty(),
        "in-place deltas must not persist whole-node snapshots: {nodes:?}"
    );


    assert!(
        ops.iter().any(
            |op| matches!(op, CommitRecordOp::AttachContentRef { component_id, .. } if component_id == "n1")
        ),
        "content edit should be an AttachContentRef: {ops:?}"
    );
    assert!(
        ops.iter().any(
            |op| matches!(op, CommitRecordOp::SetComponentAux { key_symbol, .. } if key_symbol == "fresh")
        ),
        "new attr should be SetComponentAux: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|op| matches!(op, CommitRecordOp::UnsetComponentAux { key_symbol, .. } if key_symbol == "stale")),
        "removed attr should be UnsetComponentAux: {ops:?}"
    );
}

#[test]
fn file_diff_deltas_round_trip_through_transaction_trace_replay() {
    let old = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "old text",
              "attrs": { "stale": 1 } }
        ]
    });
    let new = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "new text",
              "attrs": { "fresh": 2 } }
        ]
    });

    let mut ops = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut content = crate::store::ContentStore::default();
    super::history::diff_value_trees(&old, &new, &mut ops, &mut nodes, &mut content).expect("diff");

    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops,
    };
    let content_map = inline_content_map(&content);


    let mut head = old.clone();
    super::projection_replay::apply_commit_record(&mut head, &commit, &nodes, &content_map)
        .expect("apply commit record");
    assert_eq!(head, new, "HEAD replay must equal after tree");


    let trace = crate::transaction_trace::TransactionTrace::diff(0, 1, &old, &head);
    let replayed = trace.replay(&old).expect("trace replay");
    assert_eq!(
        replayed, new,
        "transaction trace replay must equal after tree"
    );
    assert_eq!(head, replayed, "commit and trace replay paths must agree");
}

#[test]
fn file_diff_delete_emits_only_tombstone() {
    let old = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "gone" }
        ]
    });
    let new = json!({
        "id": "root", "componentType": "root", "children": []
    });

    let mut ops = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut content = crate::store::ContentStore::default();
    super::history::diff_value_trees(&old, &new, &mut ops, &mut nodes, &mut content).expect("diff");

    assert!(
        ops.iter()
            .any(|op| matches!(op, CommitRecordOp::DeleteComponent { component_id } if component_id == "n1")),
        "delete should emit a DeleteComponent tombstone: {ops:?}"
    );
    assert_eq!(ops.len(), 1, "delete should be a single tombstone: {ops:?}");
    assert!(
        nodes.is_empty(),
        "delete should not persist node snapshots: {nodes:?}"
    );

    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops,
    };
    let mut head = old.clone();
    super::projection_replay::apply_commit_record(&mut head, &commit, &nodes, &BTreeMap::new())
        .expect("apply commit record");
    assert_eq!(head, new);
}

#[test]
fn commit_delta_ops_round_trip_through_trace_replay() {
    let base = || {
        json!({
            "id": "root", "componentType": "root", "children": [
                { "id": "n1", "componentType": "text_content", "content": "before",
                  "aux": { "stale": 1, "keep": 2 } }
            ]
        })
    };
    let want = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "n1", "componentType": "text_content", "content": "after",
              "aux": { "keep": 2, "fresh": 3 } }
        ]
    });

    let mut content = crate::store::ContentStore::default();
    let content_blob = content.put_inline("node.content", "after", 64).id;
    let attr_blob = content.put_inline("node.attr", "3", 64).id;
    let ops = vec![
        CommitRecordOp::AttachContentRef {
            component_id: "n1".to_string(),
            blob_id: content_blob,
        },
        CommitRecordOp::SetComponentAux {
            component_id: "n1".to_string(),
            key_symbol: "fresh".to_string(),
            value_ref: attr_blob,
        },
        CommitRecordOp::UnsetComponentAux {
            component_id: "n1".to_string(),
            key_symbol: "stale".to_string(),
        },
    ];
    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops,
    };
    let content_map = inline_content_map(&content);

    let mut head = base();
    super::projection_replay::apply_commit_record(
        &mut head,
        &commit,
        &BTreeMap::new(),
        &content_map,
    )
    .expect("apply commit record");
    assert_eq!(head, want, "apply_commit_record path");

    let trace = crate::transaction_trace::TransactionTrace::diff(0, 1, &base(), &head);
    let replayed = trace.replay(&base()).expect("trace replay");
    assert_eq!(replayed, want, "transaction trace replay path");
    assert_eq!(head, replayed, "commit and trace replay paths must agree");
}

#[test]
fn file_diff_content_delta_is_idempotent_under_archive_reapply() {






    let old = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "c1", "componentType": "text_content", "content": "the quick brown fox" }
        ]
    });
    let new = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "c1", "componentType": "text_content", "content": "the quick red fox" }
        ]
    });

    let mut ops = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut content = crate::store::ContentStore::default();
    super::history::diff_value_trees(&old, &new, &mut ops, &mut nodes, &mut content).expect("diff");
    assert!(
        ops.iter()
            .all(|op| !matches!(op, CommitRecordOp::ReplaceContentRange { .. })),
        "content edit must use an idempotent op, not ReplaceContentRange: {ops:?}"
    );

    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops,
    };
    let content_map = inline_content_map(&content);

    let mut once = old.clone();
    super::projection_replay::apply_commit_record(
        &mut once,
        &commit,
        &BTreeMap::new(),
        &content_map,
    )
    .expect("apply once");
    assert_eq!(once, new, "single application must reach the after tree");

    let mut twice = old.clone();
    super::projection_replay::apply_commit_record(
        &mut twice,
        &commit,
        &BTreeMap::new(),
        &content_map,
    )
    .expect("apply 1/2");
    super::projection_replay::apply_commit_record(
        &mut twice,
        &commit,
        &BTreeMap::new(),
        &content_map,
    )
    .expect("apply 2/2");
    assert_eq!(
        twice, once,
        "content delta must be idempotent under archive re-application"
    );
}

#[test]
fn unset_component_aux_missing_node_is_noop_in_both_replay_paths() {


    let commit = CommitRecord {
        schema: COMMIT_RECORD_SCHEMA.to_string(),
        branch_id: "main".to_string(),
        cycle: 1,
        revision: 1,
        lifecycle_tick: None,
        ops: vec![CommitRecordOp::UnsetComponentAux {
            component_id: "missing".to_string(),
            key_symbol: "k".to_string(),
        }],
    };
    let base = json!({
        "id": "root", "componentType": "root",
        "children": [{ "id": "present", "attrs": { "k": 1 } }]
    });

    let mut head = base.clone();
    super::projection_replay::apply_commit_record(
        &mut head,
        &commit,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
    .expect("HEAD replay must not error on a missing node");

    let trace = crate::transaction_trace::TransactionTrace::diff(0, 1, &base, &head);
    let replayed = trace.replay(&base).expect("trace replay");

    assert_eq!(head, base, "HEAD path must be a no-op");
    assert_eq!(replayed, base, "trace path must be a no-op");
    assert_eq!(head, replayed, "both paths must agree");
}





#[cfg(feature = "compression-zstd")]
fn write_append_heavy_session(path: &Path, cycles: u64) {
    let root = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "turn_1", "componentType": "message_turn", "type": "message_turn", "role": "user", "children": [] }
        ]
    });
    let root_store = new_ledger_json(&root.to_string(), Some("main"), 96).expect("root store");
    file_create_from_ledger_json(path, &root_store, 96).expect("create");

    let template = "<shell_history>\n".to_string()
        + &"  <line>cargo build --release --workspace</line>\n".repeat(8);
    for cycle in 1..=cycles {
        let node = json!({
            "id": format!("res_{cycle}"),
            "componentType": "tool_result",
            "type": "tool_result",
            "content": format!("<shell_output>{template}<status>step {cycle} ok</status></shell_output>")
        });
        let commit = CommitRecord {
            schema: COMMIT_RECORD_SCHEMA.to_string(),
            branch_id: "main".to_string(),
            cycle,
            revision: cycle,
            lifecycle_tick: None,
            ops: vec![CommitRecordOp::InsertComponent {
                parent_id: "turn_1".to_string(),
                node_ref: format!("res_{cycle}"),
                position: Some(cycle as i64),
            }],
        };
        file_append_commit_json(
            path,
            &serde_json::to_string(&commit).expect("commit json"),
            &serde_json::to_string(&vec![node]).expect("nodes json"),
        )
        .expect("append commit");
    }
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_v1_repack_projects_from_ledger_and_shrinks() {
    let source = temp_path("packed_v1_repack_source");
    let output = temp_path("packed_v1_repack_output");
    write_append_heavy_session(&source, 40);

    let report_json = file_repack_packed_v1_json(&source, &output).expect("packed-v1 repack");
    let report: Value = serde_json::from_str(&report_json).expect("report json");
    assert_eq!(report["format"], "packed-v1");
    assert_eq!(report["head_projected_from_ledger"], Value::Bool(true));
    assert_eq!(report["head_cycle"].as_u64(), Some(40));


    let source_bytes = fs::read(&source).expect("read source");
    let scanned = scan_file(&source_bytes, true);
    let head_cycle = latest_projected_cycle(&scanned.records);
    let v0_head = crate::transaction_trace::normalize_for_transaction_trace(
        &project_records_revision_file_wire_value(&scanned.records, head_cycle).expect("v0 head"),
    );

    let packed_bytes = fs::read(&output).expect("read packed");
    let packed = crate::pact_pack::PackedV1::read(&packed_bytes).expect("read packed-v1");
    assert_eq!(
        packed.project_head().expect("packed head"),
        v0_head,
        "packed-v1 head projection must equal the v0 cycle projection"
    );



    let full_state_gzip = gzip_len(&expanded_record_jsonl_bytes(&scanned.records)).expect("gzip");
    let packed_len = report["output_file_bytes"].as_u64().unwrap_or(u64::MAX) as usize;
    assert!(
        packed_len < full_state_gzip,
        "packed-v1 ({packed_len}) should beat gzip(v0 record jsonl) ({full_state_gzip})"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}



#[cfg(feature = "compression-zstd")]
fn node_content_map(value: &Value) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    fn walk(node: &Value, out: &mut std::collections::BTreeMap<String, String>) {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            let content = node
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            out.insert(id.to_string(), content);
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                walk(child, out);
            }
        }
    }
    walk(value, &mut out);
    out
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_v1_is_readable_through_normal_apis() {
    let source = temp_path("packed_api_v0_src");
    let packed = temp_path("packed_api_v1");
    let packed_slice = temp_path("packed_api_v1_slice");
    write_append_heavy_session(&source, 12);
    file_repack_packed_v1_json(&source, &packed).expect("repack to packed-v1");



    let v0_head: Value =
        serde_json::from_str(&file_project_tree_json(&source, None, None).expect("v0 project"))
            .expect("v0 head json");
    let packed_head: Value =
        serde_json::from_str(&file_project_tree_json(&packed, None, None).expect("packed project"))
            .expect("packed head json");
    let packed_main_head: Value = serde_json::from_str(
        &file_project_tree_json(&packed, Some("main"), None).expect("packed main project"),
    )
    .expect("packed main head json");
    assert_eq!(
        node_content_map(&packed_head),
        node_content_map(&v0_head),
        "packed-v1 project must match the v0 head"
    );
    assert_eq!(
        node_content_map(&packed_main_head),
        node_content_map(&v0_head),
        "packed-v1 project with explicit main branch must match the v0 head"
    );
    assert!(
        file_project_tree_json(&packed, Some("side"), None).is_err(),
        "packed-v1 exposes only the main branch"
    );
    assert!(node_content_map(&packed_head).contains_key("res_12"));


    let packed_proj =
        file_projection_json(&packed, None, None, 96, None, None).expect("packed projection");
    let proj: Value = serde_json::from_str(&packed_proj).expect("projection json");
    assert_eq!(proj["schema"], "pact.projection.v1");
    assert!(
        proj["branch"]["components"].get("res_12").is_some(),
        "projection should include packed-v1 nodes"
    );


    let q = file_query_json(&packed, "node", Some("res_5")).expect("packed query");
    assert!(q.contains("res_5"), "query should find res_5: {q}");



    let head: Value =
        serde_json::from_str(&file_head_json(&packed, None, None).expect("packed head")).unwrap();
    assert!(head.is_object(), "head should read: {head}");
    let validation: Value =
        serde_json::from_str(&file_validate_json(&packed).expect("packed validate")).unwrap();
    assert_eq!(validation["ok"], Value::Bool(true), "packed file validates");
    let components_json = file_hydrate_components_json(&packed, &["res_3".to_string()], None, None)
        .expect("packed hydrate-components");
    assert!(
        components_json.contains("res_3"),
        "hydrate-components finds res_3"
    );



    let current: Value =
        serde_json::from_str(&file_project_head_json(&packed).expect("packed current"))
            .expect("current json");
    assert_eq!(
        node_content_map(&current),
        node_content_map(&v0_head),
        "project-head should match the v0 head"
    );
    let cycle_5 = file_project_revision_json(&packed, 5).expect("packed cycle projection");
    assert!(
        cycle_5.contains("res_5"),
        "project-revision should read packed-v1 history: {cycle_5}"
    );
    let slice_report = file_slice_json(
        &packed,
        &packed_slice,
        r#"{"mode":"branch-with-lineage","branch_id":"main"}"#,
    )
    .expect("slice packed-v1");
    let slice_validation: PactFileValidation =
        serde_json::from_str(&slice_report).expect("slice validation");
    assert!(slice_validation.ok, "packed-v1 slice validates");
    let sliced =
        file_project_tree_json(&packed_slice, Some("main"), None).expect("project packed-v1 slice");
    assert!(
        sliced.contains("res_12"),
        "slice should preserve packed-v1 branch content"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(packed);
    let _ = fs::remove_file(packed_slice);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_v1_inspect_and_diagnostic_apis_read_through() {





    let source = temp_path("packed_diag_src");
    let packed = temp_path("packed_diag_v1");
    write_append_heavy_session(&source, 12);
    file_repack_packed_v1_json(&source, &packed).expect("repack to packed-v1");

    let as_json = |s: &str| serde_json::from_str::<Value>(s).expect("json");
    let u64_at = |v: &Value, ptr: &str| v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0);
    let bool_at = |v: &Value, ptr: &str| v.pointer(ptr).and_then(Value::as_bool);
    let not_null = |v: &Value, ptr: &str| !v.pointer(ptr).unwrap_or(&Value::Null).is_null();


    let v0_inspect = as_json(&file_inspect_json(&source).expect("v0 inspect"));
    let pk_inspect = as_json(&file_inspect_json(&packed).expect("packed inspect"));
    assert!(
        u64_at(&v0_inspect, "/record_count") > 0,
        "v0 inspect has records"
    );
    assert!(
        u64_at(&pk_inspect, "/record_count") > 0,
        "packed inspect must report records, not an empty manifest: {pk_inspect}"
    );


    let pk_manifest = as_json(&file_manifest_info_json(&packed).expect("packed manifest"));
    assert_eq!(
        bool_at(&pk_manifest, "/valid"),
        Some(true),
        "packed manifest must report valid: {pk_manifest}"
    );
    assert!(
        u64_at(&pk_manifest, "/manifest/record_count") > 0,
        "packed manifest record_count must be > 0"
    );


    let v0_tss = as_json(&file_token_span_stats_json(&source).expect("v0 token-span"));
    let pk_tss = as_json(&file_token_span_stats_json(&packed).expect("packed token-span"));
    assert!(u64_at(&v0_tss, "/records_seen") > 0, "v0 sees records");
    assert!(
        u64_at(&pk_tss, "/records_seen") > 0 && u64_at(&pk_tss, "/pages_seen") > 0,
        "packed token-span must walk real pages/records: {pk_tss}"
    );


    let pk_fast_head = as_json(&file_open_head_fast_json(&packed).expect("packed fast head"));
    assert!(
        not_null(&pk_fast_head, "/head"),
        "packed fast-head must not be null: {pk_fast_head}"
    );


    let pk_fast_open = as_json(&file_fast_open_summary_json(&packed).expect("packed fast open"));
    assert!(
        not_null(&pk_fast_open, "/footer"),
        "packed fast-open footer must not be null: {pk_fast_open}"
    );


    let pk_import = as_json(&file_import_check_json(&packed).expect("packed import check"));
    assert_eq!(
        bool_at(&pk_import, "/ok"),
        Some(true),
        "packed import-check must be ok: {pk_import}"
    );


    let pk_bench = as_json(&file_bench_storage_json(&packed).expect("packed bench"));
    assert_eq!(
        bool_at(&pk_bench, "/valid"),
        Some(true),
        "packed bench must be valid: {pk_bench}"
    );
    assert!(
        u64_at(&pk_bench, "/record_count") > 0,
        "packed bench record_count must be > 0"
    );



    assert_eq!(
        file_bundle_verify_json(&packed).is_ok(),
        file_bundle_verify_json(&source).is_ok(),
        "packed bundle-verify ok-ness must match v0"
    );
    assert_eq!(
        file_bundle_prune_json(&packed).is_ok(),
        file_bundle_prune_json(&source).is_ok(),
        "packed bundle-prune ok-ness must match v0"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(packed);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_v1_revision_numbers_match_source() {



    let source = temp_path("packed_rev_src");
    let packed_path = temp_path("packed_rev_v1");
    write_append_heavy_session(&source, 5);
    file_repack_packed_v1_json(&source, &packed_path).expect("repack");

    let scanned = scan_file(&fs::read(&source).expect("read source"), true);
    let head_cycle = latest_projected_cycle(&scanned.records);
    assert_eq!(head_cycle, 5);

    let packed = crate::pact_pack::PackedV1::read(&fs::read(&packed_path).expect("read packed"))
        .expect("read packed-v1");
    assert_eq!(
        packed.head_rev, head_cycle,
        "packed head_rev must equal the source head cycle (no off-by-one)"
    );


    for rev in 0..=head_cycle {
        let src_tree =
            project_records_revision_file_wire_value(&scanned.records, rev).expect("source cycle");
        let pkd_tree = packed.project_revision(rev).expect("packed rev");
        assert_eq!(
            node_content_map(&pkd_tree),
            node_content_map(&src_tree),
            "packed revision {rev} must match source cycle {rev}"
        );
    }

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(packed_path);
}

#[cfg(feature = "compression-zstd")]
fn tree_contains_id(value: &Value, id: &str) -> bool {
    if value.get("id").and_then(Value::as_str) == Some(id) {
        return true;
    }
    value
        .get("children")
        .and_then(Value::as_array)
        .is_some_and(|children| children.iter().any(|child| tree_contains_id(child, id)))
}

#[cfg(feature = "compression-zstd")]
#[test]
fn packed_v1_repack_preserves_base_tree_with_no_commits() {


    let root = json!({
        "id": "root", "componentType": "root", "children": [
            { "id": "turn", "componentType": "message_turn", "type": "message_turn", "role": "user",
              "pact_depth": 0, "children": [
                { "id": "text", "componentType": "text_content", "content": "base content must survive" }
            ]}
        ]
    });
    let root_store = new_ledger_json(&root.to_string(), Some("main"), 96).expect("root store");
    let source = temp_path("packed_v1_base_only_src");
    let output = temp_path("packed_v1_base_only_out");
    file_create_from_ledger_json(&source, &root_store, 96).expect("create");

    let scanned = scan_file(&fs::read(&source).expect("read"), true);
    let head_cycle = latest_projected_cycle(&scanned.records);

    file_repack_packed_v1_json(&source, &output).expect("repack base-only");
    let packed = crate::pact_pack::PackedV1::read(&fs::read(&output).expect("read packed"))
        .expect("read packed-v1");
    let head = packed.project_head().expect("packed head");

    assert!(
        tree_contains_id(&head, "text") && tree_contains_id(&head, "turn"),
        "base tree (head_cycle={head_cycle}) must survive repack: {head:#?}"
    );

    assert_eq!(
        head,
        crate::transaction_trace::normalize_for_transaction_trace(
            &project_records_revision_file_wire_value(&scanned.records, head_cycle)
                .expect("v0 head")
        )
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}

#[test]
fn packed_v1_repack_rejects_multi_branch_sessions() {


    let source = temp_path("packed_v1_multibranch_src");
    let output = temp_path("packed_v1_multibranch_out");
    write_commit_fixture(&source);
    let (commit, nodes) = insert_text_commit(3, 3, "fork_leaf", "fork text");
    file_append_commit_json(&source, &commit, &nodes).expect("append commit");
    file_create_branch_at_json(&source, Some("main"), Some(3), Some("forked"), 8).expect("fork");

    let result = file_repack_packed_v1_json(&source, &output);
    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|err| err.to_string().contains("multi-branch")),
        "expected a multi-branch refusal, got {result:?}"
    );
    assert!(
        !output.exists(),
        "no packed file should be written on refusal"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(output);
}

#[test]
fn projected_and_selected_components_use_aux_not_attrs() {



    let path = temp_path("projected_and_selected_components_use_aux_not_attrs");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn_1",
            "componentType": "message_turn",
            "role": "user",
            "pact_depth": 0,
            "children": [{
                "id": "note_1",
                "componentType": "text_content",
                "content": "hello",

                "aux": {"host_note": "kept", "flag": true}
            }]
        }]
    });
    let root = new_ledger_json(&tree.to_string(), Some("main"), 16).expect("root store");
    file_create_from_ledger_json(&path, &root, 16).expect("create");


    let projected: Value = serde_json::from_str(&file_project_head_json(&path).expect("project"))
        .expect("projected json");
    let note = find_node_by_id(&projected, "note_1").expect("note in projected tree");
    let note_obj = note.as_object().expect("note is object");
    assert!(
        !note_obj.contains_key("attrs"),
        "projected component must not expose a top-level `attrs` key: {note_obj:?}"
    );
    assert_eq!(
        note["aux"]["host_note"],
        json!("kept"),
        "extension data must live under `aux`"
    );
    assert_eq!(note["aux"]["flag"], json!(true));


    let selected =
        crate::selector::select_components(&projected, "#note_1", false).expect("select note_1");
    assert_eq!(selected.len(), 1);
    let selected_value = serde_json::to_value(&selected[0]).expect("selected json");
    let selected_obj = selected_value.as_object().expect("selected is object");
    assert!(
        !selected_obj.contains_key("attrs"),
        "selected component must not expose a top-level `attrs` key: {selected_obj:?}"
    );
    assert_eq!(selected_value["aux"]["host_note"], json!("kept"));
    assert_eq!(selected_value["aux"]["flag"], json!(true));

    let _ = fs::remove_file(path);
}



fn open_ledger(path: &Path) -> crate::store::ContextTreeLedger {
    ledger_from_records(&scan_file(&fs::read(path).expect("read .pact"), true).records)
        .expect("decode ledger")
}

#[test]
fn parent_provenance_serde_round_trip() {
    let provenance = crate::store::PactParentProvenance {
        parent_ref: crate::store::PactRef {
            root_id: "root".to_string(),
            branch_id: "main".to_string(),
            revision: Some(3),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        },
        locator_hints: vec![crate::store::PactParentLocator {
            kind: "relative_path".to_string(),
            value: "parent.pact".to_string(),
        }],
        aux: BTreeMap::new(),
    };
    let json = serde_json::to_string(&provenance).expect("encode provenance");
    let decoded: crate::store::PactParentProvenance =
        serde_json::from_str(&json).expect("decode provenance");
    assert_eq!(decoded, provenance);


    let old_ledger_json = sample_root_store();
    let mut old_value: Value = serde_json::from_str(&old_ledger_json).expect("ledger json");
    assert!(
        old_value
            .as_object_mut()
            .unwrap()
            .remove("parent")
            .is_none(),
        "fixture should not already carry a parent field"
    );
    let old_ledger: crate::store::ContextTreeLedger =
        serde_json::from_str(&old_value.to_string()).expect("decode old ledger");
    assert!(old_ledger.parent.is_none());
}

#[test]
fn hard_fork_writes_child_and_leaves_source_byte_unchanged() {
    let source = temp_path("hard_fork_writes_child_source");
    let child = temp_path("hard_fork_writes_child_child");
    let root = sample_root_store();
    file_create_from_ledger_json(&source, &root, 8).expect("create source");

    let source_bytes_before = fs::read(&source).expect("read source before");

    let report_json = file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("hard fork");
    let report: Value = serde_json::from_str(&report_json).expect("report json");


    assert!(child.is_file(), "child .pact must exist");
    assert_eq!(report["mode"], "hard");
    assert_eq!(report["parent_ref"]["root_id"], "root");
    assert_eq!(report["parent_ref"]["branch_id"], "main");
    assert_eq!(report["parent_ref"]["revision"], 1);


    let child_root_id = report["child_root_id"].as_str().expect("child root id");
    assert_ne!(child_root_id, "root");


    let source_bytes_after = fs::read(&source).expect("read source after");
    assert_eq!(
        source_bytes_before, source_bytes_after,
        "hard fork must not mutate the source file"
    );


    let child_ledger = open_ledger(&child);
    let provenance = child_ledger
        .parent
        .as_ref()
        .expect("child has parent provenance");
    assert_eq!(provenance.parent_ref.root_id, "root");
    assert_eq!(provenance.parent_ref.branch_id, "main");
    assert_eq!(provenance.parent_ref.revision, Some(1));
    assert_eq!(child_ledger.root_id, child_root_id);

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(child);
}

#[test]
fn hard_fork_child_opens_to_fork_point_head_without_parent() {
    let dir = std::env::temp_dir().join(format!(
        "hard_fork_child_open_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let source = dir.join("source.pact");
    let child = dir.join("child.pact");

    let root = sample_root_store();
    file_create_from_ledger_json(&source, &root, 8).expect("create source");


    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("hard fork");



    fs::remove_file(&source).expect("remove parent");
    assert!(!source.is_file());

    let child_head: Value =
        serde_json::from_str(&file_project_tree_json(&child, Some("main"), None).expect("project"))
            .expect("child head json");


    assert!(find_node_by_id(&child_head, "turn_1").is_some());
    assert!(find_node_by_id(&child_head, "text_1").is_some());
    assert!(
        find_node_by_id(&child_head, "text_2").is_some(),
        "fork at revision 1 must include the revision-1 node text_2"
    );



    let child_ledger = open_ledger(&child);
    assert_eq!(child_ledger.branches["main"].head_revision, 0);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn hard_fork_at_earlier_revision_drops_later_nodes() {
    let source = temp_path("hard_fork_earlier_rev_source");
    let child = temp_path("hard_fork_earlier_rev_child");
    let root = sample_root_store();
    file_create_from_ledger_json(&source, &root, 8).expect("create source");


    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("hard fork at rev 0");

    let child_head: Value =
        serde_json::from_str(&file_project_tree_json(&child, Some("main"), None).expect("project"))
            .expect("child head json");
    assert!(find_node_by_id(&child_head, "text_1").is_some());
    assert!(
        find_node_by_id(&child_head, "text_2").is_none(),
        "fork at revision 0 must not contain the revision-1 node text_2"
    );

    let _ = fs::remove_file(source);
    let _ = fs::remove_file(child);
}

#[test]
fn soft_fork_writes_no_file_and_returns_view_ref() {
    let source = temp_path("soft_fork_source");
    let child = temp_path("soft_fork_child");
    let root = sample_root_store();
    file_create_from_ledger_json(&source, &root, 8).expect("create source");

    let report_json = file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Soft,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("soft fork");
    let report: Value = serde_json::from_str(&report_json).expect("report json");

    assert_eq!(report["mode"], "soft");
    assert_eq!(report["view_ref"]["source_root_id"], "root");
    assert_eq!(report["view_ref"]["source_branch_id"], "main");
    assert_eq!(report["view_ref"]["kind"], "read_only_pinned");

    assert!(!child.is_file(), "soft fork must not write a child file");

    let _ = fs::remove_file(source);
}

fn resolver_child_with_parent(parent_root_id: &str) -> crate::store::PactParentProvenance {
    crate::store::PactParentProvenance {
        parent_ref: crate::store::PactRef {
            root_id: parent_root_id.to_string(),
            branch_id: "main".to_string(),
            revision: Some(1),
            transaction_id: None,
            component_id: None,
            content_ref: None,
        },
        locator_hints: Vec::new(),
        aux: BTreeMap::new(),
    }
}

#[test]
fn parent_resolver_explicit_map_hit() {
    let dir = std::env::temp_dir().join(format!(
        "parent_resolver_explicit_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let parent_path = dir.join("somewhere_else.pact");
    file_create_from_ledger_json(&parent_path, &sample_root_store(), 8).expect("create parent");

    let provenance = resolver_child_with_parent("root");
    let mut explicit = BTreeMap::new();
    explicit.insert("root".to_string(), parent_path.clone());
    let config = ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents: explicit,
    };


    let verified = resolve_parent(None, &provenance, &config).expect("resolve via explicit map");
    assert_eq!(verified.path, parent_path);
    assert_eq!(verified.ledger.root_id, "root");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn parent_resolver_same_directory_default() {
    let dir = std::env::temp_dir().join(format!(
        "parent_resolver_samedir_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");

    let parent_path = dir.join("root.pact");
    let child_path = dir.join("child.pact");
    file_create_from_ledger_json(&parent_path, &sample_root_store(), 8).expect("create parent");

    let provenance = resolver_child_with_parent("root");
    let config = ParentResolverConfig::default();

    let verified =
        resolve_parent(Some(&child_path), &provenance, &config).expect("resolve same-dir");
    assert_eq!(verified.path, parent_path);
    assert_eq!(verified.ledger.root_id, "root");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn parent_resolver_missing_parent_returns_typed_error() {
    let dir = std::env::temp_dir().join(format!(
        "parent_resolver_missing_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let child_path = dir.join("child.pact");


    let provenance = resolver_child_with_parent("root");
    let config = ParentResolverConfig {
        base_dir: Some(dir.clone()),
        search_paths: vec![dir.clone()],
        explicit_parents: BTreeMap::new(),
    };

    let err = resolve_parent(Some(&child_path), &provenance, &config)
        .expect_err("missing parent must error");
    assert_eq!(err.kind(), crate::error::PactErrorKind::UnresolvedParent);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn parent_resolver_wrong_root_returns_typed_mismatch() {
    let dir = std::env::temp_dir().join(format!(
        "parent_resolver_wrong_root_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");


    let parent_path = dir.join("different_root.pact");
    file_create_from_ledger_json(&parent_path, &sample_root_store(), 8).expect("create parent");

    let provenance = resolver_child_with_parent("different_root");
    let mut explicit = BTreeMap::new();
    explicit.insert("different_root".to_string(), parent_path.clone());
    let config = ParentResolverConfig {
        base_dir: None,
        search_paths: Vec::new(),
        explicit_parents: explicit,
    };

    let err = resolve_parent(None, &provenance, &config).expect_err("wrong root must error");
    assert_eq!(err.kind(), crate::error::PactErrorKind::ParentMismatch);

    let _ = fs::remove_dir_all(dir);
}



#[test]
fn parent_aware_select_child_only_runs_fast_path_without_parent() {
    let dir = std::env::temp_dir().join(format!(
        "parent_aware_child_only_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let source = dir.join("root.pact");
    let child = dir.join("child.pact");
    file_create_from_ledger_json(&source, &sample_root_store(), 8).expect("create source");
    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("hard fork");


    fs::remove_file(&source).expect("remove parent");

    let query = PactFileSelectorQuery::new("@t0 .text_content");
    let config = ParentResolverConfig::default();
    let result =
        file_select_with_parents(&child, query, &config).expect("child-only select succeeds");
    assert!(
        result.hits.iter().any(|hit| hit.component_id == "text_1"),
        "child fork-point head must contain text_1: {result:?}"
    );

    let _ = fs::remove_dir_all(dir);
}



#[test]
fn parent_aware_select_history_missing_parent_is_typed_unresolved() {
    let dir = std::env::temp_dir().join(format!(
        "parent_aware_history_missing_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let source = dir.join("root.pact");
    let child = dir.join("child.pact");
    file_create_from_ledger_json(&source, &sample_root_store(), 8).expect("create source");
    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(1),
        None,
        8,
    )
    .expect("hard fork");



    fs::remove_file(&source).expect("remove parent");
    let empty = dir.join("empty");
    fs::create_dir_all(&empty).expect("mkdir empty");

    let query = PactFileSelectorQuery::new("@history .text_content");
    let config = ParentResolverConfig {
        base_dir: Some(empty.clone()),
        search_paths: vec![empty],
        explicit_parents: BTreeMap::new(),
    };
    let err = file_select_with_parents(&child, query, &config)
        .expect_err("history with missing parent must error");
    assert_eq!(
        err.kind(),
        crate::error::PactErrorKind::UnresolvedParent,
        "got: {err}"
    );

    let _ = fs::remove_dir_all(dir);
}



#[test]
fn parent_aware_select_history_wrong_parent_is_typed_mismatch() {
    let dir = std::env::temp_dir().join(format!(
        "parent_aware_history_wrong_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let source = dir.join("root.pact");
    let child = dir.join("child.pact");
    file_create_from_ledger_json(&source, &sample_root_store(), 8).expect("create source");
    let fork_report: Value = serde_json::from_str(
        &file_fork_json(
            &source,
            Some(&child),
            PactForkMode::Hard,
            Some("main"),
            Some(1),
            None,
            8,
        )
        .expect("hard fork"),
    )
    .expect("fork report");
    let parent_root_id = fork_report["parent_ref"]["root_id"]
        .as_str()
        .expect("parent root id")
        .to_string();
    assert_eq!(parent_root_id, "root");



    fs::remove_file(&source).expect("remove real parent");
    let imposter_tree = r#"{
        "id":"imposter_root",
        "componentType":"root",
        "children":[{
            "id":"turn_x","componentType":"message_turn","type":"message_turn",
            "role":"user","pact_depth":0,"attrs":{"cycle":1},
            "children":[{"id":"text_x","componentType":"text_content","attrs":{"cycle":1},"content":"imposter"}]
        }]
    }"#;
    let imposter_ledger = new_ledger_json(imposter_tree, Some("main"), 8).expect("imposter ledger");


    let imposter_path = dir.join(format!("{parent_root_id}.pact"));
    file_create_from_ledger_json(&imposter_path, &imposter_ledger, 8).expect("create imposter");

    let query = PactFileSelectorQuery::new("@history .text_content");
    let config = ParentResolverConfig::default();
    let err = file_select_with_parents(&child, query, &config)
        .expect_err("history with wrong parent must error");
    assert_eq!(
        err.kind(),
        crate::error::PactErrorKind::ParentMismatch,
        "got: {err}"
    );

    let _ = fs::remove_dir_all(dir);
}











#[test]
#[ignore = "parent-crossing @history blocked on streaming-projection stitch primitive (streaming-projection-replaces-materialization)"]
fn parent_aware_select_history_crosses_into_resolvable_parent() {
    let dir = std::env::temp_dir().join(format!(
        "parent_aware_history_cross_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");



    let source = dir.join("root.pact");
    let child = dir.join("child.pact");
    file_create_from_ledger_json(&source, &sample_root_store(), 8).expect("create source");
    file_fork_json(
        &source,
        Some(&child),
        PactForkMode::Hard,
        Some("main"),
        Some(0),
        None,
        8,
    )
    .expect("hard fork at rev 0");

    let query = PactFileSelectorQuery::new("@history .text_content");
    let config = ParentResolverConfig::default();
    let result = file_select_with_parents(&child, query, &config)
        .expect("history crosses into resolvable parent");


    assert!(
        result.hits.iter().any(|hit| hit.component_id == "text_2"),
        "parent-crossing @history must include pre-fork parent content text_2: {result:?}"
    );

    let _ = fs::remove_dir_all(dir);
}





#[test]
fn same_file_branch_op_is_not_public_hard_fork() {
    let dir = std::env::temp_dir().join(format!(
        "same_file_branch_not_hard_fork_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let same_file = dir.join("session.pact");
    file_create_from_ledger_json(&same_file, &sample_root_store(), 8).expect("create");



    let branch_report: Value = serde_json::from_str(
        &file_create_branch_at_json(&same_file, Some("main"), Some(1), Some("branchy"), 8)
            .expect("create branch at"),
    )
    .expect("branch report");
    assert_eq!(branch_report["schema"], "pact.file.v0.fork_at.v1");
    assert!(
        branch_report.get("child_root_id").is_none(),
        "same-file branch op must NOT mint a child root id"
    );
    let same_file_ledger = open_ledger(&same_file);
    assert!(
        same_file_ledger.parent.is_none(),
        "same-file branch op must NOT write parent provenance"
    );
    assert!(
        same_file_ledger.branches.contains_key("branchy"),
        "same-file branch op adds a branch to the one ledger"
    );



    let child = dir.join("child.pact");
    let hard_report: Value = serde_json::from_str(
        &file_fork_json(
            &same_file,
            Some(&child),
            PactForkMode::Hard,
            Some("main"),
            Some(1),
            None,
            8,
        )
        .expect("hard fork"),
    )
    .expect("hard report");
    assert_eq!(hard_report["mode"], "hard");
    assert!(
        child.is_file(),
        "hard fork writes an independent child file"
    );
    let child_ledger = open_ledger(&child);
    assert!(
        child_ledger.parent.is_some(),
        "hard fork child carries parent provenance"
    );
    assert_ne!(
        child_ledger.root_id, same_file_ledger.root_id,
        "hard fork child has a distinct root id"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn file_record_branch_decode_rejects_removed_patch_history() {



    let removed_field = concat!("patch", "es");
    let root_json = new_ledger_json(
        r#"{"id":"root","componentType":"root","children":[]}"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let path = temp_path("file_record_branch_reject");
    file_create_from_ledger_json(&path, &root_json, 8).expect("create");
    let mut scanned = scan_file(&fs::read(&path).expect("read"), true);
    let branch_record = scanned
        .records
        .iter_mut()
        .find(|record| record.kind == "branch")
        .expect("a branch record");
    branch_record.payload[removed_field] = json!([{ "rev": 1 }]);

    let err = ledger_from_records(&scanned.records)
        .expect_err("removed patch-shaped branch history must be rejected");
    assert!(
        err.to_string()
            .contains("removed patch-shaped mutation history"),
        "unexpected error: {err}"
    );
    let _ = fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
use content::encode_content_body;

#[cfg(feature = "compression-zstd")]
use layout::PACT_RECORD_ZSTD_DICT_CODEC;
use layout::{
    COMMIT_RECORD_SCHEMA, ENGLISH_LEXPACK_ID, RECORD_PAYLOAD_MAGIC, SHELL_LEXPACK_ID,
    TOKEN_SPAN_PART_LEX_REF, TOKEN_SPAN_PART_LEX_REF_RUN, TOKEN_SPAN_PART_SPAN_REF,
    TOKEN_SPAN_PART_TOKEN_TABLE_RUN, TOKEN_SPAN_STRINGS_V1, VALUE_TAG_FILE_LOCAL_ID_REF,
    VALUE_TAG_OBJECT_SYMBOL_KEYS, VALUE_TAG_STRING, VALUE_TAG_SYMBOL_REF,
    VALUE_TAG_TOKEN_SPAN_STRING, VALUE_TAG_VAR_U64, VALUE_TAG_ZIGZAG_I64,
};

fn decode_record_payload(kind: &str, payload: &[u8]) -> PactResult<Value> {
    page_codec::decode_record_payload_with_symbols(kind, payload, &SymbolTable::default())
}

pub(crate) use parent::resolve_parent;

use record_codec::expand_canonical_record_payload;

use value_codec::encode_pact_value_payload;

fn encode_pact_value_payload_with_symbols(
    value: &Value,
    symbols: &SymbolTable,
) -> PactResult<Vec<u8>> {
    value_codec::encode_pact_value_payload_with_symbols_and_options(
        value,
        symbols,
        PactValueCodecOptions::default(),
    )
}

use value_codec::{read_len, write_len};
