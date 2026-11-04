use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
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

fn fake_pre_v01_reader() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("reader fixture directory");
    let source = directory.path().join("reader.rs");
    let executable = directory
        .path()
        .join(format!("reader{}", std::env::consts::EXE_SUFFIX));
    fs::write(
        &source,
        r#"fn main() {
    assert_eq!(std::env::args().nth(1).as_deref(), Some("file-records"));
    let export = std::env::var_os("PACTREE_TEST_RECORD_EXPORT").expect("export path");
    print!("{}", std::fs::read_to_string(export).expect("read export"));
}"#,
    )
    .expect("write fake reader");
    let compiler = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile fake reader");
    assert!(
        compiler.status.success(),
        "fake reader compilation failed: {}",
        String::from_utf8_lossy(&compiler.stderr)
    );
    (directory, executable)
}

#[test]
fn converter_preserves_source_and_publishes_verified_pactbin2() {
    let source = temp_path("pre-v01-source", "pact");
    let output = temp_path("converted-output", "pact");
    let export = temp_path("pre-v01-export", "json");
    let source_bytes = [b"PACTBIN1".as_slice(), b"immutable source bytes"].concat();
    fs::write(&source, &source_bytes).expect("write source");
    fs::write(&export, exported_records()).expect("write export");
    let (_reader_directory, pre_v01_reader) = fake_pre_v01_reader();

    let process = Command::new(env!("CARGO_BIN_EXE_pact-v01-convert"))
        .arg("--pre-v01-pactree")
        .arg(&pre_v01_reader)
        .arg(&source)
        .arg(&output)
        .env("PACTREE_TEST_RECORD_EXPORT", &export)
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

    for path in [source, output, export] {
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
