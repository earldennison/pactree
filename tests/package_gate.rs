use std::path::{Path, PathBuf};
use std::process::Command;

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn package_listing_excludes_untracked_capture_sentinel() {
    let sentinel = repository_root().join("nexus-pipe-package-gate-sentinel.bin");
    assert!(!sentinel.exists(), "sentinel must not pre-exist");
    std::fs::write(&sentinel, b"not-a-real-key").expect("write sentinel");
    let _cleanup = RemoveOnDrop(sentinel);

    let output = Command::new(env!("CARGO"))
        .args(["package", "--allow-dirty", "--list"])
        .current_dir(repository_root())
        .output()
        .expect("run cargo package --list");

    assert!(
        output.status.success(),
        "cargo package --list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing = String::from_utf8(output.stdout).expect("package list is UTF-8");
    assert!(
        !listing.contains("nexus-pipe-package-gate-sentinel.bin"),
        "untracked capture sentinel entered the crate package"
    );
}

#[test]
fn package_listing_includes_the_separate_one_shot_converter() {
    let output = Command::new(env!("CARGO"))
        .args(["package", "--allow-dirty", "--list"])
        .current_dir(repository_root())
        .output()
        .expect("run cargo package --list");
    assert!(output.status.success(), "cargo package --list must succeed");
    let listing = String::from_utf8(output.stdout).expect("package list is UTF-8");
    assert!(
        listing
            .lines()
            .any(|path| path == "tools/pact-v01-convert/main.rs")
    );
}

#[test]
fn package_listing_includes_docs_referenced_by_compile_time_includes() {
    let output = Command::new(env!("CARGO"))
        .args(["package", "--allow-dirty", "--list"])
        .current_dir(repository_root())
        .output()
        .expect("run cargo package --list");
    assert!(output.status.success(), "cargo package --list must succeed");
    let listing = String::from_utf8(output.stdout).expect("package list is UTF-8");
    for required in [
        "docs/internals/adapter-boundaries.md",
        "docs/reference/file-format.md",
    ] {
        assert!(
            listing.lines().any(|path| path == required),
            "compile-time documentation input missing from package: {required}"
        );
    }
}

#[test]
fn release_features_exclude_storage_dev_dual() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(repository_root())
        .output()
        .expect("run cargo metadata");
    assert!(output.status.success(), "cargo metadata must succeed");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| packages.iter().find(|package| package["name"] == "pactree"))
        .expect("pactree package metadata");
    let features = package["features"].as_object().expect("features object");

    let removed_feature = ["storage", "-dev-dual"].concat();
    assert!(
        !features.contains_key(&removed_feature),
        "the development backend selector must not ship"
    );
    let default = features["default"].as_array().expect("default features");
    assert!(
        !default.iter().any(|feature| feature == &removed_feature),
        "the development backend selector must not be enabled by default"
    );
    assert!(
        package["metadata"].get("backend-selector").is_none(),
        "release metadata must not expose a backend selector"
    );
}
