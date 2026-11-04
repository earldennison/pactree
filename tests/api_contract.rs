use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn normalize(text: &[u8]) -> String {
    String::from_utf8_lossy(text).replace("\r\n", "\n")
}

fn command_output(command: &mut Command, label: &str) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("run {label}: {error}"));
    assert!(
        output.status.success(),
        "{label} failed:\n{}",
        normalize(&output.stderr)
    );
    output
}

fn compact_unified_diff(expected: &str, actual: &str) -> String {
    let expected_lines: Vec<_> = expected.lines().collect();
    let actual_lines: Vec<_> = actual.lines().collect();
    let first = expected_lines
        .iter()
        .zip(&actual_lines)
        .position(|(expected, actual)| expected != actual)
        .unwrap_or_else(|| expected_lines.len().min(actual_lines.len()));
    let start = first.saturating_sub(3);
    let end = (first + 4).max(start + 1);

    let mut diff = format!(
        "--- expected\n+++ actual\n@@ -{},{} +{},{} @@\n",
        start + 1,
        end - start,
        start + 1,
        end - start
    );
    for index in start..end {
        if let Some(line) = expected_lines.get(index) {
            diff.push('-');
            diff.push_str(line);
            diff.push('\n');
        }
        if let Some(line) = actual_lines.get(index) {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    diff
}

fn assert_snapshot(path: PathBuf, actual: String) {
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        expected == actual,
        "snapshot drift in {}:\n{}",
        path.display(),
        compact_unified_diff(&expected, &actual)
    );
}

#[test]
fn rust_public_api_matches_v010_snapshot() {
    let output = command_output(
        Command::new("cargo")
            .args([
                "+nightly-2026-05-31",
                "public-api",
                "--all-features",
                "--simplified",
                "--color",
                "never",
            ])
            .current_dir(repository_root()),
        "cargo public-api",
    );
    assert_snapshot(
        repository_root().join("api/pactree-0.1.0.txt"),
        normalize(&output.stdout),
    );
}

#[test]
fn cli_help_matches_v010_snapshot() {
    let output = command_output(
        Command::new(env!("CARGO_BIN_EXE_pactree")).arg("--help"),
        "pactree --help",
    );
    let path = repository_root().join("api/pactree-cli-0.1.0.txt");
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let expected = expected
        .strip_prefix("$ pactree --help\n")
        .expect("CLI snapshot command header");
    let actual = normalize(&output.stdout);
    assert!(
        expected == actual,
        "snapshot drift in {}:\n{}",
        path.display(),
        compact_unified_diff(expected, &actual)
    );
}
