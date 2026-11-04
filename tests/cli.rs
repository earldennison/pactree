use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_path(name: &str, extension: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}_{nanos}.{extension}"))
}

#[test]
fn invalid_file_validate_exits_nonzero_with_json_body() {
    let path = unique_temp_path("pact_cli_invalid_validate", "pact");
    std::fs::write(&path, b"not a Pact file").expect("write invalid file");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-validate")
        .arg(&path)
        .output()
        .expect("run file-validate");

    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("validation JSON");
    assert_eq!(report["ok"], false);
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_check_exits_nonzero() {
    assert_invalid_validation_alias_exits_nonzero("check");
}

#[test]
fn invalid_doctor_exits_nonzero() {
    assert_invalid_validation_alias_exits_nonzero("doctor");
}

fn assert_invalid_validation_alias_exits_nonzero(command: &str) {
    let path = unique_temp_path(&format!("pact_cli_invalid_{command}"), "pact");
    std::fs::write(&path, b"not a Pact file").expect("write invalid file");
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg(command)
        .arg(&path)
        .output()
        .expect("run validation alias");
    assert!(!output.status.success(), "{command} must fail");
    let _ = std::fs::remove_file(path);
}

#[test]
fn repair_invalid_output_exits_nonzero_and_publishes_nothing() {
    let source = unique_temp_path("pact_cli_invalid_repair_source", "pact");
    let output_path = unique_temp_path("pact_cli_invalid_repair_output", "pact");
    std::fs::write(&source, b"not a Pact file").expect("write invalid source");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-repair")
        .arg(&source)
        .arg(&output_path)
        .output()
        .expect("run file-repair");

    assert!(!output.status.success());
    assert!(!output_path.exists(), "failed repair must publish nothing");
    let _ = std::fs::remove_file(source);
}

#[test]
fn rewind_optional_branch_does_not_consume_revision() {
    let path = unique_temp_path("pact_cli_rewind_optional_branch", "pact");
    write_sample_pact(&path);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-rewind-to")
        .arg(&path)
        .arg("0")
        .output()
        .expect("run rewind without branch");

    assert!(
        output.status.success(),
        "rewind failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn malformed_revision_is_rejected_not_silently_dropped() {
    let root_store = pactree::new_ledger_json_without_base(
        r#"{"id":"revision_root","componentType":"root","children":[]}"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("root-branch-revision")
        .arg("main")
        .arg("not-a-revision")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn root-branch-revision");
    let write_result = child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes());
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    let output = child.wait_with_output().expect("wait");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid revision"));
}

#[test]
fn all_optional_revision_arguments_reject_malformed_values() {
    let root_store = pactree::new_ledger_json_without_base(
        r#"{"id":"optional_revision_root","componentType":"root","children":[]}"#,
        Some("main"),
        8,
    )
    .expect("root store");
    for args in [
        vec!["root-project-branch", "main", "bad"],
        vec!["store-project", "bad"],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn revision command");
        let write_result = child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(root_store.as_bytes());
        if let Err(error) = write_result {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let output = child.wait_with_output().expect("wait");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid revision"));
    }
}

#[test]
fn version_matches_cargo_package() {
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("--version")
        .output()
        .expect("run version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("version utf8")
            .trim(),
        format!("pactree {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_uses_pactree_binary_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("help utf8");
    assert!(help.contains("USAGE:\n  pactree "));
    assert!(!help.contains("\n  pact "));
}

#[test]
fn cli_inspects_stdin_tree() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("inspect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(
            br#"{
                "id":"root",
                "componentType":"ContextRoot",
                "children":[{"id":"a","componentType":"TextContext","content":"abc"}]
            }"#,
        )
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"components\": 2"));
    assert!(stdout.contains("\"content_bytes\": 3"));
}

#[test]
fn cli_normalize_matches_pact_schema() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("normalize")
        .env("PACT_BLOB_INLINE_LIMIT", "8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(
            br#"{
                "id":"root",
                "componentType":"root",
                "content":"short",
                "children":[
                    {
                        "id":"msg_1",
                        "componentType":"message_turn",
                        "role":"user",
                        "content":"abcdefghijklmnopqrstuvwxyz"
                    }
                ]
            }"#,
        )
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\":\"pact.normalized.v1\""));
    assert!(stdout.contains("\"root_id\":\"root\""));
    assert!(stdout.contains("\"content_ref\":\"blob_00000000\""));
}

#[test]
fn cli_select_uses_depth_selectors() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("select")
        .arg("d0 > .text_content")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(
            br#"{
                "id":"root",
                "componentType":"root",
                "children":[
                    {
                        "id":"active",
                        "componentType":"message_turn",
                        "type":"message_turn",
                        "role":"user",
                        "pact_depth":0,
                        "children":[
                            {
                                "id":"active_text",
                                "componentType":"text_content",
                                "type":"text_content",
                                "offset":0,
                                "content":"question"
                            }
                        ]
                    }
                ]
            }"#,
        )
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"id\": \"active_text\""));
    assert!(stdout.contains("\"path\": ["));
}

#[test]
fn cli_prune_removes_tool_call_pairs() {
    let input_tree = br#"{
        "id":"root",
        "componentType":"root",
        "children":[{
            "id":"turn",
            "componentType":"message_turn",
            "children":[{
                "id":"container",
                "componentType":"message_container",
                "children":[
                    {
                        "id":"call",
                        "componentType":"tool_call",
                        "type":"tool_call",
                        "tool_call_id":"toolu_123"
                    },
                    {
                        "id":"result",
                        "componentType":"tool_result",
                        "type":"tool_result",
                        "tool_call_id":"toolu_123"
                    },
                    {
                        "id":"text",
                        "componentType":"text_content",
                        "content":"keep"
                    }
                ]
            }]
        }]
    }"#;

    let mut root_new = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("root-new")
        .arg("--no-base")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn root-new");
    root_new
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input_tree)
        .expect("write root-new stdin");
    let root_output = root_new.wait_with_output().expect("wait root-new");
    assert!(root_output.status.success());

    let mut prune = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("prune")
        .arg("#call")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn prune");
    prune
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&root_output.stdout)
        .expect("write prune stdin");
    let prune_output = prune.wait_with_output().expect("wait prune");
    assert!(prune_output.status.success());

    let mut project = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("root-project-branch")
        .arg("main")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn project");
    project
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&prune_output.stdout)
        .expect("write project stdin");
    let output = project.wait_with_output().expect("wait project");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(!stdout.contains("\"id\":\"call\""));
    assert!(!stdout.contains("\"id\":\"result\""));
    assert!(stdout.contains("\"id\":\"text\""));
}

#[test]
fn cli_prune_alias_removes_selected_component() {
    let input_tree = br#"{
        "id":"root",
        "componentType":"root",
        "children":[
            {"id":"remove_me","componentType":"text_content","content":"gone"},
            {"id":"keep_me","componentType":"text_content","content":"stay"}
        ]
    }"#;

    let mut root_new = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("root-new")
        .arg("--no-base")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn root-new");
    root_new
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input_tree)
        .expect("write root-new stdin");
    let root_output = root_new.wait_with_output().expect("wait root-new");
    assert!(root_output.status.success());

    let mut prune = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("prune")
        .arg("#remove_me")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn prune");
    prune
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&root_output.stdout)
        .expect("write prune stdin");
    let prune_output = prune.wait_with_output().expect("wait prune");
    assert!(prune_output.status.success());

    let mut project = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("root-project-branch")
        .arg("main")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn project");
    project
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&prune_output.stdout)
        .expect("write project stdin");
    let output = project.wait_with_output().expect("wait project");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(!stdout.contains("\"id\":\"remove_me\""));
    assert!(stdout.contains("\"id\":\"keep_me\""));
}

#[test]
fn cli_projection_scope_archive_includes_deleted_nodes() {
    let store_json = pactree::new_store_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn",
                "componentType":"message_turn",
                "children":[
                    {"id":"text","componentType":"text_content","content":"hello"},
                    {"id":"deleted_text","componentType":"text_content","content":"archive me"}
                ]
            }]
        }"#,
        8,
    )
    .expect("store");
    let seeded_root_store =
        pactree::ledger_from_store_json(&store_json, Some("main")).expect("root store");

    let mut prune = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("prune")
        .arg("#deleted_text")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn prune");
    prune
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(seeded_root_store.as_bytes())
        .expect("write prune stdin");
    let prune_output = prune.wait_with_output().expect("wait prune");
    assert!(prune_output.status.success());
    let root_store = String::from_utf8(prune_output.stdout).expect("pruned root store utf8");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_archive_scope_{nanos}.pact"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&path)
        .output()
        .expect("run view head");
    assert!(output.status.success());
    let head_out = String::from_utf8(output.stdout).expect("head utf8");
    assert!(
        !head_out.contains("deleted_text"),
        "view head must exclude the deleted node: {head_out}"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("archived")
        .arg(&path)
        .output()
        .expect("run view archived");
    assert!(output.status.success());
    let archive: serde_json::Value = serde_json::from_slice(&output.stdout).expect("archive json");
    assert_eq!(archive["scope"], "archive");
    assert!(
        archive["branch"]["components"]
            .get("deleted_text")
            .is_some()
    );
    assert_eq!(
        archive["branch"]["components"]["deleted_text"]["aux"]["pact_lifecycle"],
        "deleted"
    );
    assert_eq!(
        archive["branch"]["components"]["deleted_text"]["aux"]["visible_in_head"],
        false
    );

    let archived_rows = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("archived")
        .arg(&path)
        .arg("--format")
        .arg("rows")
        .output()
        .expect("run view archived rows");
    assert!(
        archived_rows.status.success(),
        "view archived --format rows failed: {}",
        String::from_utf8_lossy(&archived_rows.stderr)
    );
    let archived_rows_json: serde_json::Value =
        serde_json::from_slice(&archived_rows.stdout).expect("archived rows json");
    assert_eq!(archived_rows_json["scope"], "archive");
    assert!(
        archived_rows_json["rows"].is_array(),
        "view archived --format rows must return a rows array"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn cli_creates_validates_and_queries_pact_file() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "attrs":{"cycle":1},
                "pact_depth":0,
                "children":[
                    {"id":"text","componentType":"text_content","content":"hello","attrs":{"cycle":1}},
                    {"id":"call","componentType":"tool_call","tool_call_id":"toolu_cli","attrs":{"cycle":1}},
                    {"id":"result","componentType":"tool_result","tool_call_id":"toolu_cli","attrs":{"cycle":1}}
                ]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_{nanos}.pact"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.file.v2\""));
    assert!(stdout.contains("\"record_count\""));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-validate")
        .arg(&path)
        .output()
        .expect("run file-validate");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"ok\": true"));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("info")
        .arg(&path)
        .output()
        .expect("run info");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.file.v0.info.v1\""));
    assert!(stdout.contains("\"record_count\""));
    assert!(stdout.contains("\"records\""));
    assert!(stdout.contains("\"fields\""));
    assert!(stdout.contains("\"bundle\""));
    assert!(stdout.contains("\"user_messages\""));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("run inspect file");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"record_count\""));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&path)
        .output()
        .expect("run view head");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.head.v1\""));
    assert!(stdout.contains("\"branch_id\": \"main\""));
    assert!(stdout.contains("\"revision\": 0"));
    assert!(stdout.contains("\"trace_count\": 0"));

    let session_dir = std::env::temp_dir().join(format!("pact_cli_session_{nanos}"));
    let session_pact_dir = session_dir.join("pact");
    std::fs::create_dir_all(&session_pact_dir).expect("create session pact dir");
    let session_pact = session_pact_dir.join("current.pact");
    std::fs::copy(&path, &session_pact).expect("copy current pact");
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&session_dir)
        .arg("--format")
        .arg("xml")
        .output()
        .expect("run view head xml");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("<pact_head"));
    assert!(stdout.contains("branch_id=\"main\""));
    assert!(stdout.contains("revision=\"0\""));

    let healed_path = std::env::temp_dir().join(format!("pact_cli_healed_{nanos}.pact"));
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("heal")
        .arg(&path)
        .arg("--out")
        .arg(&healed_path)
        .output()
        .expect("run heal");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"repaired_validation\""));
    assert!(stdout.contains("\"rebuilt_indexes\": true"));
    assert!(stdout.contains("\"is_healed\""));

    let view_head = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&path)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run view head");
    assert!(view_head.status.success());
    let view_head_out = String::from_utf8(view_head.stdout).expect("utf8");
    assert!(
        view_head_out.contains("\"schema\": \"pact.head.v1\""),
        "view head --format json is the canonical head: {view_head_out}"
    );

    let view_rows = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&path)
        .arg("--format")
        .arg("rows")
        .output()
        .expect("run view head rows");
    assert!(
        view_rows.status.success(),
        "view head --format rows failed: {}",
        String::from_utf8_lossy(&view_rows.stderr)
    );
    let rows_json: serde_json::Value =
        serde_json::from_slice(&view_rows.stdout).expect("rows json");
    assert!(
        rows_json["rows"].is_array(),
        "view head --format rows must return a rows array"
    );

    let view_archived = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("archived")
        .arg(&path)
        .arg("--format")
        .arg("json")
        .output()
        .expect("run view archived");
    assert!(
        view_archived.status.success(),
        "view archived should succeed: {}",
        String::from_utf8_lossy(&view_archived.stderr)
    );
    let archived_out = String::from_utf8(view_archived.stdout).expect("utf8");
    serde_json::from_str::<serde_json::Value>(&archived_out)
        .expect("view archived emits valid JSON");

    let load_components = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("load")
        .arg("components")
        .arg(&path)
        .arg("text")
        .output()
        .expect("run load components");
    assert!(load_components.status.success());
    let load_out = String::from_utf8(load_components.stdout).expect("utf8");
    assert!(
        load_out.contains("\"id\": \"text\""),
        "load components should return the requested component: {load_out}"
    );

    let bad_target = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("projection")
        .arg(&path)
        .output()
        .expect("run view projection");
    assert!(
        !bad_target.status.success(),
        "view projection must not be a public target"
    );

    let timeline = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("inspect")
        .arg("timeline")
        .arg(&path)
        .output()
        .expect("run inspect timeline");
    assert!(
        timeline.status.success(),
        "inspect timeline failed: {}",
        String::from_utf8_lossy(&timeline.stderr)
    );
    let timeline_json: serde_json::Value =
        serde_json::from_slice(&timeline.stdout).expect("timeline json");
    assert!(
        timeline_json["schema"]
            .as_str()
            .unwrap_or_default()
            .contains("timeline"),
        "inspect timeline schema: {timeline_json}"
    );
    assert!(
        timeline_json["rows"].is_array(),
        "inspect timeline must return a rows array"
    );

    let slice_path = std::env::temp_dir().join(format!("pact_cli_slice_{nanos}.pact"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-slice")
        .arg(&path)
        .arg(&slice_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact file-slice");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(br#"{"mode":"branch-with-lineage","branch_id":"main"}"#)
        .expect("write slice spec");
    let output = child.wait_with_output().expect("wait file-slice");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"ok\": true"));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("import")
        .arg(&path)
        .arg(&slice_path)
        .output()
        .expect("run import");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.file.v0.import\""));
    assert!(stdout.contains("\"applied\": true"));
    assert!(stdout.contains("imported_main"));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("merge")
        .arg(&path)
        .arg(&slice_path)
        .output()
        .expect("run merge");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"applied\": true"));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-import-check")
        .arg(&slice_path)
        .output()
        .expect("run file-import-check");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"ok\": true"));

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(slice_path);
    let _ = std::fs::remove_file(healed_path);
    let _ = std::fs::remove_dir_all(session_dir);
}

#[test]
fn cli_help_documents_view_and_load_verbs() {
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("utf8");

    assert!(
        help.contains("pactree view head"),
        "help should document view head"
    );
    assert!(
        help.contains("pactree view archived"),
        "help should document view archived"
    );
    assert!(
        help.contains("pactree load content-ref"),
        "help should document load content-ref"
    );
    assert!(
        help.contains("pactree load components"),
        "help should document load components"
    );
    assert!(
        help.contains("pactree store-project"),
        "help should document store projection"
    );

    assert!(
        help.contains("pactree branch-view append"),
        "branch management moved to branch-view"
    );

    for gone in [
        "pactree project ",
        "pactree projection ",
        "pactree file-projection ",
        concat!("pactree file-", "material", "ize "),
        "pactree file-stream ",
        "pactree file-buffered-read ",
        "pactree file-query ",
        "pactree cycles ",
        "pactree text-search ",
        "pactree open ",
        "pactree head ",
        "pactree render ",
        "pactree branch-view projection ",
        "pactree branch-view query ",
        concat!("pactree store-", "material", "ize "),
        concat!("pactree root-", "material", "ize-branch "),
        "pactree delete ",
        "pactree store-fork ",
        "pactree store-checkpoint ",
    ] {
        assert!(
            !help.contains(gone),
            "removed command still in help: {gone}"
        );
    }
}

#[test]
fn cli_removed_read_commands_error_as_unknown() {
    for cmd in [
        "projection",
        "project",
        "file-projection",
        concat!("file-", "material", "ize"),
        concat!("file-", "material", "ize-current"),
        "file-stream",
        "stream",
        "file-buffered-read",
        "buffered-read",
        "cycles",
        "file-cycles",
        "file-query",
        "text-search",
        "file-text-search",
        "open",
        "file-open",
        "head",
        "file-head",
        "pactree-head",
        "render",
        "render-xml",
        "delete",
        "store-materialize",
        "root-materialize-branch",
        "store-fork",
        "store-checkpoint",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
            .arg(cmd)
            .arg("/tmp/does-not-matter.pact")
            .output()
            .expect("run removed command");
        assert!(
            !output.status.success(),
            "removed command {cmd:?} must not succeed"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unknown command"),
            "removed command {cmd:?} should report unknown command, got: {stderr}"
        );
    }
}

#[test]
fn cli_text_search_queries_pact_file() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "children":[{"id":"text","componentType":"text_content","content":"hello searchable memory"}]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_text_search_{nanos}.pact"));
    pactree::file_format::file_create_from_ledger_json(&path, &root_store, 8).expect("create pact");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("search")
        .arg("text")
        .arg(&path)
        .arg("searchable")
        .arg("--scope")
        .arg("head")
        .arg("--json")
        .output()
        .expect("run text-search");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.file.v0.text_search.v1\""));
    assert!(stdout.contains("\"component_id\": \"text\""));
    assert!(stdout.contains("\"resident_in_current_surface\": true"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn cli_reanchor_keeps_semantic_checkpoints_out_of_the_public_record_surface() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn_1",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "pact_depth":0,
                "children":[{"id":"text_0","componentType":"text_content","content":"start"}]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_no_anchor_{nanos}.pact"));
    let anchored_path =
        std::env::temp_dir().join(format!("pact_cli_no_anchor_anchored_{nanos}.pact"));
    let stripped_path =
        std::env::temp_dir().join(format!("pact_cli_no_anchor_stripped_{nanos}.pact"));
    pactree::file_format::file_create_from_ledger_json(&path, &root_store, 8).expect("create pact");

    for index in 1..=3 {
        let component_id = format!("text_{index}");
        let node = serde_json::json!({
            "id": component_id,
            "componentType": "text_content",
            "type": "text_content",
            "offset": index,
            "content": format!("content {index}")
        });
        let commit = serde_json::json!({
            "schema": "pact.commit.v1",
            "branch_id": "main",
            "cycle": index,
            "revision": index,
            "ops": [{
                "op": "insert_component",
                "parent_id": "turn_1",
                "node_ref": node["id"],
                "position": index
            }]
        });
        pactree::file_format::file_append_commit_json(
            &path,
            &commit.to_string(),
            &serde_json::json!([node]).to_string(),
        )
        .expect("append commit");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-reanchor")
        .arg(&path)
        .arg(&anchored_path)
        .arg("--freeze-cycle-interval")
        .arg("1")
        .output()
        .expect("run file-reanchor");
    assert!(output.status.success());
    let anchored_report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("anchored report");
    assert_eq!(anchored_report["anchors_written"], 0);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("no-anchor")
        .arg(&anchored_path)
        .arg(&stripped_path)
        .output()
        .expect("run no-anchor");
    assert!(output.status.success());
    let stripped_report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stripped report");
    assert_eq!(stripped_report["anchors_written"], 0);
    assert_eq!(stripped_report["output_manifest"]["frozen_cycle_count"], 0);

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(anchored_path);
    let _ = std::fs::remove_file(stripped_path);
}

#[test]
fn cli_rejects_component_snapshot_commands() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{"id":"turn","componentType":"message_turn","type":"message_turn","role":"user","pact_depth":0}]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_reject_components_{nanos}.pact"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait create");
    assert!(output.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-append-components")
        .arg(&path)
        .output()
        .expect("run removed append command");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn cli_projects_pact_views() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{"id":"turn","componentType":"message_turn","type":"message_turn","role":"user","pact_depth":0}]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let root_store = pactree::ledger_create_branch_at_revision_json(
        &root_store,
        "main",
        Some(0),
        Some("branch_view"),
        None,
        8,
    )
    .expect("create branch view");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_view_{nanos}.pact"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success());

    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("branch-view")
        .arg("append")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pact view append");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(
            br#"{"views":[{"view_id":"view_alpha","root_id":"root","source_branch_id":"main","source_revision":0,"mode":"mutable_live","view_branch_id":"branch_view","changed_component_ids":["cmp_file_index"]}]}"#,
        )
        .expect("write view");
    let output = child.wait_with_output().expect("wait view append");
    assert!(output.status.success());

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("branch-view")
        .arg("rebase")
        .arg(&path)
        .arg("view_alpha")
        .output()
        .expect("run view rebase");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"schema\": \"pact.file.v0.view.rebase\""));
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("view rebase report");
    assert!(report.get("replayed_patch_count").is_none());

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("branch-view")
        .arg("detach")
        .arg(&path)
        .arg("view_alpha")
        .arg("detached_view")
        .output()
        .expect("run view detach");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"record_count\""));

    let _ = std::fs::remove_file(path);
}

fn build_corpus_selector_cli_fixture(path: &std::path::Path) {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn_1",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "pact_depth":0,
                "attrs":{"cycle":1},
                "children":[{
                    "id":"text_1",
                    "componentType":"text_content",
                    "type":"text_content",
                    "content":"cycle one origin",
                    "attrs":{"cycle":1}
                }]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    pactree::file_format::file_create_from_ledger_json(path, &root_store, 8).expect("create pact");

    for (cycle, content) in [
        (2u64, "cycle two: cli needle here"),
        (3, "cycle three: distinct phrase"),
        (4, "cycle four: cli needle again"),
    ] {
        let component_id = format!("cli_text_{cycle}");
        let node = serde_json::json!({
            "id": component_id,
            "componentType": "text_content",
            "type": "text_content",
            "offset": cycle as i64,
            "content": content,
            "aux": {"cycle": cycle}
        });
        let commit = serde_json::json!({
            "schema": "pact.commit.v1",
            "branch_id": "main",
            "cycle": cycle,
            "revision": cycle,
            "ops": [{
                "op": "insert_component",
                "parent_id": "turn_1",
                "node_ref": node["id"],
                "position": cycle as i64
            }]
        });
        pactree::file_format::file_append_commit_json(
            path,
            &commit.to_string(),
            &serde_json::json!([node]).to_string(),
        )
        .expect("append commit");
    }
}

#[test]
fn cli_file_select_queries_temporal_corpus_selector() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_file_select_{nanos}.pact"));
    build_corpus_selector_cli_fixture(&path);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("select")
        .arg(&path)
        .arg("@history .text_content")
        .arg("--limit")
        .arg("2")
        .output()
        .expect("run select over .pact");
    assert!(
        output.status.success(),
        "file-select failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("file-select json");
    assert_eq!(value["schema"], "pact.file.v0.selector.v1");
    let hits = value["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 2);
    let cycles: Vec<u64> = hits
        .iter()
        .filter_map(|hit| hit["cycle"].as_u64())
        .collect();
    for window in cycles.windows(2) {
        assert!(
            window[0] >= window[1],
            "expected newest-first ordering, got {cycles:?}"
        );
    }
    assert_eq!(value["temporal"]["normalized"], "@history");
    assert_eq!(value["source"], "stream");

    let _ = std::fs::remove_file(path);
}

#[test]
fn cli_text_search_accepts_temporal_selector_filter() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pact_cli_text_search_temporal_{nanos}.pact"));
    build_corpus_selector_cli_fixture(&path);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("search")
        .arg("text")
        .arg(&path)
        .arg("cli needle")
        .arg("--select")
        .arg("@c2 .text_content")
        .output()
        .expect("run text-search");
    assert!(
        output.status.success(),
        "text-search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("text-search json");
    let hits = value["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["component_id"], "cli_text_2");
    assert_eq!(hits[0]["cycle"], 2);
    assert!(
        value["source"]
            .as_str()
            .unwrap_or("")
            .starts_with("selector_stream"),
        "expected selector_stream source, got {:?}",
        value["source"]
    );

    let _ = std::fs::remove_file(path);
}

#[cfg(feature = "compression-zstd")]
#[test]
fn cli_repack_rejects_the_removed_packed_format() {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "pact_depth":0,
                "children":[
                    {"id":"text","componentType":"text_content","content":"hello packed world"}
                ]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("pact_pack_src_{nanos}.pact"));
    let output = std::env::temp_dir().join(format!("pact_pack_out_{nanos}.pact"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(&source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    assert!(child.wait_with_output().expect("wait").status.success());

    let out = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-repack")
        .arg(&source)
        .arg(&output)
        .arg("--format")
        .arg("packed-v1")
        .output()
        .expect("run file-repack --format packed-v1");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unsupported"),
        "unexpected error: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!output.exists(), "removed format must not create output");

    let _ = std::fs::remove_file(source);
    let _ = std::fs::remove_file(output);
}

#[test]
fn cli_repack_refuses_to_overwrite_existing_output() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let source = std::env::temp_dir().join(format!("pact_repack_source_{nanos}.pact"));
    let output = std::env::temp_dir().join(format!("pact_repack_existing_{nanos}.pact"));
    write_sample_pact(&source);
    let sentinel = b"existing destination must survive";
    std::fs::write(&output, sentinel).expect("seed existing output");

    let result = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-repack")
        .arg(&source)
        .arg(&output)
        .output()
        .expect("run file-repack");

    assert!(!result.status.success(), "repack must reject overwrite");
    assert_eq!(
        std::fs::read(&output).expect("read preserved output"),
        sentinel
    );

    let _ = std::fs::remove_file(source);
    let _ = std::fs::remove_file(output);
}

fn write_sample_pact(path: &std::path::Path) {
    let root_store = pactree::new_ledger_json(
        r#"{
            "id":"root",
            "componentType":"root",
            "children":[{
                "id":"turn_1",
                "componentType":"message_turn",
                "type":"message_turn",
                "role":"user",
                "attrs":{"cycle":1},
                "pact_depth":0,
                "children":[
                    {"id":"text_1","componentType":"text_content","content":"hello fork","attrs":{"cycle":1}}
                ]
            }]
        }"#,
        Some("main"),
        8,
    )
    .expect("root store");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-create")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn file-create");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(root_store.as_bytes())
        .expect("write stdin");
    assert!(
        child.wait_with_output().expect("wait").status.success(),
        "file-create must succeed"
    );
}

#[test]
fn cli_fork_hard_creates_child_file() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_fork_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let parent = dir.join("parent.pact");
    let child = dir.join("child.pact");
    write_sample_pact(&parent);

    let parent_bytes_before = std::fs::read(&parent).expect("read parent before");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("fork")
        .arg(&parent)
        .arg(&child)
        .arg("--mode")
        .arg("hard")
        .arg("--branch")
        .arg("main")
        .output()
        .expect("run pact fork --mode hard");
    assert!(
        output.status.success(),
        "fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"mode\": \"hard\""), "report: {stdout}");
    assert!(
        stdout.contains("\"child_root_id\""),
        "hard fork report must carry a child root id: {stdout}"
    );
    assert!(
        stdout.contains("\"parent_ref\""),
        "hard fork report must carry parent_ref: {stdout}"
    );

    assert!(child.is_file(), "hard fork must write a child .pact");
    let validate = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-validate")
        .arg(&child)
        .output()
        .expect("run file-validate on child");
    assert!(validate.status.success());
    assert!(
        String::from_utf8_lossy(&validate.stdout).contains("\"ok\": true"),
        "child must validate"
    );

    let parent_bytes_after = std::fs::read(&parent).expect("read parent after");
    assert_eq!(
        parent_bytes_before, parent_bytes_after,
        "hard fork must not mutate the parent file"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_fork_soft_writes_no_file() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_fork_soft_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let parent = dir.join("parent.pact");
    let child = dir.join("child.pact");
    write_sample_pact(&parent);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("fork")
        .arg(&parent)
        .arg("--mode")
        .arg("soft")
        .output()
        .expect("run pact fork --mode soft");
    assert!(
        output.status.success(),
        "soft fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"mode\": \"soft\""), "report: {stdout}");
    assert!(!child.is_file(), "soft fork must not write a child file");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_help_documents_fork_as_new_file_and_drops_fork_at() {
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("utf8");

    assert!(
        help.contains("pactree fork parent.pact child.pact --mode hard"),
        "help should document true hard fork"
    );
    assert!(
        help.contains("pactree fork parent.pact --mode soft"),
        "help should document soft fork"
    );

    assert!(
        !help.contains("pactree file-fork-at"),
        "help must not document file-fork-at"
    );
    assert!(
        !help.contains("root-fork-revision"),
        "help must not call same-ledger branch creation a fork"
    );
}

#[test]
fn cli_old_fork_at_aliases_are_unknown_commands() {
    for cmd in ["fork-at", "file-fork-at", "root-fork-revision"] {
        let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
            .arg(cmd)
            .arg("/tmp/does-not-matter.pact")
            .output()
            .expect("run retired fork alias");
        assert!(
            !output.status.success(),
            "retired alias {cmd:?} must not succeed"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("unknown command"),
            "retired alias {cmd:?} should report unknown command, got: {stderr}"
        );
    }
}

#[test]
fn cli_fork_delta_pinned_creates_child_overlay() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_fork_delta_pinned_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let parent = dir.join("parent.pact");
    let child = dir.join("child.pact");
    write_sample_pact(&parent);

    let parent_bytes_before = std::fs::read(&parent).expect("read parent before");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("fork")
        .arg(&parent)
        .arg(&child)
        .arg("--mode")
        .arg("delta")
        .arg("--base")
        .arg("pinned")
        .arg("--branch")
        .arg("main")
        .output()
        .expect("run pact fork --mode delta --base pinned");
    assert!(
        output.status.success(),
        "delta fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("\"mode\": \"delta\""), "report: {stdout}");
    assert!(
        stdout.contains("\"base_mode\": \"pinned\""),
        "delta fork report must carry base_mode pinned: {stdout}"
    );
    assert!(
        stdout.contains("\"base_ref\""),
        "delta fork report must carry base_ref: {stdout}"
    );

    assert!(child.is_file(), "delta fork must write a child .pact");
    let validate = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-validate")
        .arg(&child)
        .output()
        .expect("run file-validate on child");
    assert!(validate.status.success());
    assert!(
        String::from_utf8_lossy(&validate.stdout).contains("\"ok\": true"),
        "delta child must validate"
    );

    let parent_bytes_after = std::fs::read(&parent).expect("read parent after");
    assert_eq!(
        parent_bytes_before, parent_bytes_after,
        "delta fork must not mutate the parent file"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_fork_delta_following_creates_child_overlay() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_fork_delta_following_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let parent = dir.join("parent.pact");
    let child = dir.join("child.pact");
    write_sample_pact(&parent);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("fork")
        .arg(&parent)
        .arg(&child)
        .arg("--mode")
        .arg("delta")
        .arg("--base")
        .arg("following")
        .output()
        .expect("run pact fork --mode delta --base following");
    assert!(
        output.status.success(),
        "delta following fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.contains("\"base_mode\": \"following\""),
        "delta fork report must carry base_mode following: {stdout}"
    );
    assert!(
        child.is_file(),
        "delta following fork must write a child .pact"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_fork_delta_requires_child_path() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_fork_delta_nopath_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let parent = dir.join("parent.pact");
    write_sample_pact(&parent);

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("fork")
        .arg(&parent)
        .arg("--mode")
        .arg("delta")
        .output()
        .expect("run pact fork --mode delta with no output");
    assert!(
        !output.status.success(),
        "delta fork without a child path must fail"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_send_message_delivers_into_target_only() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pact_cli_send_message_{nanos}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let from = dir.join("from.pact");
    let to = dir.join("to.pact");
    write_sample_pact(&from);
    write_sample_pact(&to);

    let from_bytes_before = std::fs::read(&from).expect("read from before");

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("send-message")
        .arg("--from")
        .arg(&from)
        .arg("--to")
        .arg(&to)
        .arg("--text")
        .arg("cross-context hello")
        .arg("--source-trace-id")
        .arg("trace_cli_1")
        .arg("--created-at")
        .arg("2026-06-05T00:00:00Z")
        .output()
        .expect("run pact send-message");
    assert!(
        output.status.success(),
        "send-message failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("send-message report json");
    assert_eq!(
        report["schema"], "pact.cross_context_message.delivery.v1",
        "send-message report schema: {stdout}"
    );
    let delivered_component_id = report["delivered_component_id"]
        .as_str()
        .expect("delivered_component_id");
    assert!(
        report["delivery_trace_id"].as_str().is_some(),
        "report must carry a delivery trace id: {stdout}"
    );

    let from_bytes_after = std::fs::read(&from).expect("read from after");
    assert_eq!(
        from_bytes_before, from_bytes_after,
        "send-message must not mutate the source file"
    );

    let view = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("view")
        .arg("head")
        .arg(&to)
        .arg("--format")
        .arg("rows")
        .arg("--content")
        .arg("full")
        .output()
        .expect("run view head rows on target");
    assert!(
        view.status.success(),
        "view head rows on target failed: {}",
        String::from_utf8_lossy(&view.stderr)
    );
    let view_str = String::from_utf8_lossy(&view.stdout);
    assert!(
        view_str.contains(delivered_component_id),
        "delivered message turn must be present in the target context: {view_str}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn file_order_check_reports_no_carried_rows_for_a_fresh_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("fresh.pact");
    let mut tree = pactree::ContextTree::with_id("order_check").expect("tree");
    for n in 0..3 {
        let options = tree.next_authoring_component_options();
        tree.add_message(
            "user",
            vec![pactree::Component::text_authoring(
                format!("turn {n}"),
                options,
            )],
            pactree::PactAttrs::new(),
        )
        .expect("stage");
        tree.seal_staged(pactree::trace::BoundaryKind::Adhoc)
            .expect("seal");
    }
    tree.save_file(&path).expect("save");
    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .args(["file-order-check", path.to_str().unwrap()])
        .output()
        .expect("run pactree");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(report["schema"], "pact.message_order_report.v1");
    assert_eq!(report["message_rows"], 3);
    assert_eq!(report["carried_rows"], 0);
    assert_eq!(report["head_index_matches"]["neither"], 0);
}

#[test]
fn file_order_repair_requires_the_copy_flag_and_reports_the_check() {
    let path = unique_temp_path("pact_cli_order_repair", "pact");
    let mut writer =
        pactree::PactAppendWriter::open(&path, "main", pactree::PactMemoryOptions::default())
            .expect("writer");
    writer
        .append(pactree::PactAppendBatch {
            source: "lens".to_string(),
            batch_id: "b1".to_string(),
            expected: writer.version().clone(),
            previous_checkpoint: None,
            checkpoint: Some("cp1".to_string()),
            messages: vec![pactree::PactAppendMessage {
                row_id: "r1".to_string(),
                role: "user".to_string(),
                parts: vec![
                    pactree::Component::from_component_value(
                        &serde_json::json!({"componentType": "text_content", "content": "one"}),
                    )
                    .expect("part"),
                ],
                aux: pactree::PactAttrs::new(),
            }],
        })
        .expect("append");
    writer.close();

    let refused = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-order-repair")
        .arg(&path)
        .output()
        .expect("run file-order-repair");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--i-have-a-copy"));

    let output = Command::new(env!("CARGO_BIN_EXE_pactree"))
        .arg("file-order-repair")
        .arg(&path)
        .arg("--i-have-a-copy")
        .output()
        .expect("run file-order-repair");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(report["schema"], "pact.message_order_repair.v1");
    assert_eq!(report["repair"]["transaction_id"], serde_json::Value::Null);
    assert_eq!(report["repair"]["rows_reindexed"], 0);
    assert_eq!(report["order_check"]["head_index_matches"]["neither"], 0);
    let _ = std::fs::remove_file(path);
}
