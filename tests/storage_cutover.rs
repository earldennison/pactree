use pactree::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions,
    ProviderThreadOptions,
};
use serde_json::json;
use std::io::Write;

fn text_component(id: &str, content: &str) -> Component {
    Component::from_component_value(&json!({
        "id": id,
        "componentType": "text_content",
        "content": content,
    }))
    .expect("text component")
}

#[test]
fn public_context_tree_persists_only_pactbin2() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("cutover.pact");
    let mut tree = ContextTree::with_id("cutover_root").expect("tree");

    tree.save_file(&path).expect("save empty tree");
    assert_eq!(
        &std::fs::read(&path).expect("read header")[..8],
        b"PACTBIN2"
    );
    assert_eq!(
        ContextTree::open_file(&path, None)
            .expect("open empty v2 file")
            .head()
            .expect("empty head")
            .head_revision,
        0
    );
    let validation: serde_json::Value = serde_json::from_str(
        &pactree::file_format::file_validate_json(&path).expect("validate PACTBIN2"),
    )
    .expect("validation JSON");
    assert_eq!(validation["manifest"]["schema"], "pact.file.v2");
    assert_eq!(validation["manifest"]["version"], 2);

    tree.insert(text_component("cutover_text", "candidate storage"), "root")
        .expect("insert");
    tree.send(&ProviderThreadOptions::default()).expect("seal");
    let reopened = ContextTree::open_file(&path, None).expect("reopen committed v2 file");
    assert!(
        reopened
            .project_component("cutover_root:cutover_text")
            .expect("project committed component")["content"]
            == "candidate storage"
    );
    assert!(
        reopened
            .project_at_revision(0)
            .expect("project genesis")
            .branch
            .component("cutover_root:cutover_text")
            .is_none()
    );
}

#[test]
fn source_file_identity_is_physical_stable_and_public() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("identity.pact");
    let repacked_path = temp.path().join("identity-repacked.pact");
    let mut tree = ContextTree::with_id("identity_root").expect("tree");
    tree.save_file(&path).expect("save");

    let initial_info = pactree::PactFileReadHandle::open(&path, None)
        .expect("open initial")
        .info()
        .expect("initial info");
    let initial_identity = initial_info["source_file_identity"]
        .as_str()
        .expect("source identity");
    assert_eq!(initial_identity.len(), 64);
    assert_ne!(initial_identity, initial_info["context_tree_id"]);

    for revision in 1..=20 {
        tree.insert(
            text_component(
                &format!("identity_text_{revision}"),
                &format!("identity payload {revision}: {}", "repeat ".repeat(32)),
            ),
            "root",
        )
        .expect("insert");
        tree.send(&ProviderThreadOptions::default()).expect("seal");
    }

    let appended_info = pactree::PactFileReadHandle::open(&path, None)
        .expect("open appended")
        .info()
        .expect("appended info");
    assert_eq!(appended_info["source_file_identity"], initial_identity);

    pactree::file_format::file_repack_json(&path, &repacked_path).expect("repack");
    let repacked_info = pactree::PactFileReadHandle::open(&repacked_path, None)
        .expect("open repacked")
        .info()
        .expect("repacked info");
    assert_ne!(repacked_info["source_file_identity"], initial_identity);
    assert_eq!(
        repacked_info["context_tree_id"],
        appended_info["context_tree_id"]
    );
    assert_eq!(repacked_info["revision"], appended_info["revision"]);
}

#[test]
fn cold_baseline_at_a_high_revision_accepts_the_next_normal_commit() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("high-revision-baseline.pact");
    let root = json!({
        "id": "high_revision_root",
        "componentType": "root",
        "children": []
    });
    let mut ledger: serde_json::Value = serde_json::from_str(
        &pactree::new_ledger_json(&root.to_string(), Some("main"), 8).expect("new ledger"),
    )
    .expect("ledger value");
    ledger["branches"]["main"]["head_revision"] = json!(604);
    ledger["branches"]["main"]["current"]["head_revision"] = json!(604);
    ledger["branches"]["main"]["base_ref"]["revision"] = json!(604);
    pactree::file_format::file_create_from_ledger_json(&path, &ledger.to_string(), 8)
        .expect("create cold baseline");

    let mut tree = ContextTree::open_file(&path, None).expect("open cold baseline");
    tree.insert(
        text_component("revision_605", "normal append after a cold baseline"),
        "root",
    )
    .expect("insert next revision");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal next revision");

    let reopened = ContextTree::open_file(&path, None).expect("reopen appended baseline");
    assert_eq!(reopened.head().expect("head").head_revision, 605);
    assert_eq!(
        reopened
            .project_component("high_revision_root:revision_605")
            .expect("project appended component")["content"],
        "normal append after a cold baseline"
    );
}

#[test]
fn repair_reports_source_and_healed_candidate_separately() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = temp.path().join("torn.pact");
    let repaired = temp.path().join("repaired.pact");
    let mut tree = ContextTree::with_id("repair_evidence_root").expect("tree");
    tree.save_file(&source).expect("save");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .expect("open torn tail")
        .write_all(b"uncommitted-tail")
        .expect("append torn tail");

    let report: serde_json::Value = serde_json::from_str(
        &pactree::file_format::file_repair_json(&source, &repaired, true).expect("repair"),
    )
    .expect("repair report");
    assert!(
        !report["source_validation"]["manifest"]["capabilities"]
            .as_array()
            .expect("source capabilities")
            .iter()
            .any(|capability| capability == "is_healed")
    );
    assert!(
        report["repaired_validation"]["manifest"]["capabilities"]
            .as_array()
            .expect("repaired capabilities")
            .iter()
            .any(|capability| capability == "is_healed")
    );
}

#[test]
fn three_hundred_turn_file_stays_within_one_mib() {
    const MAX_FILE_BYTES: u64 = 1024 * 1024;

    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("three-hundred-turns.pact");
    let options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("size_budget_", 1_000_000, 1)
                .expect("deterministic options"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options("size_budget_root", options).expect("tree");
    tree.save_file(&path).expect("save");

    for turn in 0..300 {
        let content = format!("turn {turn}: {}", "repeated context payload ".repeat(8));
        let component_options = tree.next_authoring_component_options();
        tree.insert(
            Component::text_authoring(content, component_options),
            "root",
        )
        .expect("insert turn");
        tree.send(&ProviderThreadOptions::default())
            .expect("seal turn");
    }

    let file_bytes = std::fs::metadata(&path).expect("file metadata").len();
    assert!(
        file_bytes <= MAX_FILE_BYTES,
        "300-turn Pact file is {file_bytes} bytes; budget is {MAX_FILE_BYTES}"
    );
    let reopened = ContextTree::open_file(&path, None).expect("reopen compact file");
    assert_eq!(reopened.head().expect("head").head_revision, 300);
}

#[test]
fn read_handle_exposes_stable_source_file_identity() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("identity.pact");
    let mut tree = ContextTree::with_id("identity_root").expect("tree");
    tree.save_file(&path).expect("save");
    tree.insert(text_component("identity_text", "first"), "root")
        .expect("insert");
    tree.send(&ProviderThreadOptions::default()).expect("seal");

    let info = pactree::PactFileReadHandle::open(&path, None)
        .expect("open handle")
        .info()
        .expect("info");
    let identity = info["source_file_identity"]
        .as_str()
        .expect("source_file_identity must be exposed")
        .to_string();
    assert_eq!(identity.len(), 64);
    assert_ne!(
        identity,
        info["context_tree_id"].as_str().expect("context_tree_id"),
        "file identity must not alias the root id"
    );

    tree.insert(text_component("identity_text_2", "second"), "root")
        .expect("insert second");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal second");
    let info_after = pactree::PactFileReadHandle::open(&path, None)
        .expect("reopen handle")
        .info()
        .expect("info after append");
    assert_eq!(
        info_after["source_file_identity"]
            .as_str()
            .expect("identity"),
        identity,
        "identity must be stable across appends"
    );
    assert_eq!(
        info_after["context_tree_id"], info["context_tree_id"],
        "existing fields must remain unchanged"
    );

    let path_b = temp.path().join("identity_b.pact");
    let mut tree_b = ContextTree::with_id("identity_root_b").expect("tree b");
    tree_b.save_file(&path_b).expect("save b");
    tree_b
        .insert(text_component("identity_text_b", "other"), "root")
        .expect("insert b");
    tree_b
        .send(&ProviderThreadOptions::default())
        .expect("seal b");
    let info_b = pactree::PactFileReadHandle::open(&path_b, None)
        .expect("open handle b")
        .info()
        .expect("info b");
    assert_ne!(
        info_b["source_file_identity"].as_str().expect("identity b"),
        identity,
        "a true new file must carry a new identity"
    );
}
