use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(label: &str, extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "pactree-{label}-{}-{nonce}.{extension}",
        std::process::id()
    ))
}

fn exported_records() -> String {
    let source = temp_path("converter-export-source", "pact");
    let tree = json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "turn",
            "componentType": "message_turn",
            "role": "user",
            "children": [{
                "id": "text",
                "componentType": "text_content",
                "content": "content preserved by the standalone converter"
            }]
        }]
    });
    let ledger =
        pactree::new_ledger_json(&tree.to_string(), Some("main"), 8).expect("build ledger");
    pactree::file_format::file_create_from_ledger_json(&source, &ledger, 8).expect("write fixture");
    let scanned = pactree::file_format::scan_file(&fs::read(&source).expect("read fixture"), true);
    fs::remove_file(source).expect("remove fixture");
    serde_json::to_string(
        &scanned
            .records
            .iter()
            .map(|record| {
                json!({
                    "sequence": record.sequence,
                    "kind": record.kind,
                    "payload_bytes": record.payload_bytes,
                    "payload": record.payload,
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("encode export")
}

fn fake_pre_v01_reader(export_path: &Path) -> PathBuf {
    let script = temp_path("fake-v01-reader", "sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nexec /bin/cat '{}'\n", export_path.display()),
    )
    .expect("write fake reader");
    let mut permissions = fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&script, permissions).expect("make fake reader executable");
    script
}

#[test]
fn converter_preserves_source_and_publishes_verified_pactbin2() {
    let source = temp_path("pre-v01-source", "pact");
    let output = temp_path("converted-output", "pact");
    let export = temp_path("pre-v01-export", "json");
    let source_bytes = [b"PACTBIN1".as_slice(), b"immutable source bytes"].concat();
    fs::write(&source, &source_bytes).expect("write source");
    fs::write(&export, exported_records()).expect("write export");
    let pre_v01_reader = fake_pre_v01_reader(&export);

    let process = Command::new(env!("CARGO_BIN_EXE_pact-v01-convert"))
        .arg("--pre-v01-pactree")
        .arg(&pre_v01_reader)
        .arg(&source)
        .arg(&output)
        .output()
        .expect("run converter");

    assert!(
        process.status.success(),
        "converter failed: {}",
        String::from_utf8_lossy(&process.stderr)
    );
    assert_eq!(fs::read(&source).expect("read source"), source_bytes);
    assert_eq!(&fs::read(&output).expect("read output")[..8], b"PACTBIN2");
    let report: Value = serde_json::from_slice(&process.stdout).expect("report json");
    assert_eq!(report["verified"], true);
    assert_eq!(report["source_bytes"], source_bytes.len());

    for path in [source, output, export, pre_v01_reader] {
        fs::remove_file(path).expect("remove fixture");
    }
}

#[test]
fn converter_refuses_to_overwrite_destination_before_running_pre_v01_reader() {
    let source = temp_path("pre-v01-existing-source", "pact");
    let output = temp_path("pre-v01-existing-output", "pact");
    fs::write(&source, [b"PACTBIN1".as_slice(), b"source"].concat()).expect("write source");
    fs::write(&output, b"existing destination").expect("write destination");

    let process = Command::new(env!("CARGO_BIN_EXE_pact-v01-convert"))
        .arg("--pre-v01-pactree")
        .arg("/path/that/must/not/run")
        .arg(&source)
        .arg(&output)
        .output()
        .expect("run converter");

    assert!(!process.status.success());
    assert!(String::from_utf8_lossy(&process.stderr).contains("already exists"));
    assert_eq!(
        fs::read(&output).expect("read destination"),
        b"existing destination"
    );
    fs::remove_file(source).expect("remove source");
    fs::remove_file(output).expect("remove output");
}
