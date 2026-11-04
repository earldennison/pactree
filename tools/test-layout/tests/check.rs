use std::path::PathBuf;
use std::process::{Command, Output};

fn check(fixture: &str) -> Output {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    Command::new(env!("CARGO_BIN_EXE_pactree-test-layout"))
        .arg(fixture)
        .output()
        .expect("run test-layout checker")
}

#[test]
fn accepts_only_out_of_line_tests_module() {
    let output = check("allowed");
    assert!(
        output.status.success(),
        "checker rejected allowed layout:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_inline_test_module_with_location() {
    let output = check("inline-module");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "checker accepted inline tests");
    assert!(stderr.contains("inline.rs:1"), "missing location: {stderr}");
    assert!(
        stderr.contains("inline test module"),
        "missing reason: {stderr}"
    );
}

#[test]
fn rejects_test_only_items_with_location() {
    let output = check("test-item");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "checker accepted test-only helper"
    );
    assert!(stderr.contains("helper.rs:1"), "missing location: {stderr}");
    assert!(
        stderr.contains("test-only function"),
        "missing reason: {stderr}"
    );
}
