use std::path::Path;

#[test]
fn v010_metadata_and_governance_are_complete() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    for required in [
        "version = \"0.1.0\"",
        "rust-version = \"1.93\"",
        "repository = \"https://github.com/earldennison/pactree\"",
        "documentation = \"https://docs.rs/pactree\"",
    ] {
        assert!(manifest.contains(required), "missing metadata: {required}");
    }
    for file in [
        "LICENSE",
        "CHANGELOG.md",
        "SECURITY.md",
        "CONTRIBUTING.md",
        "rust-toolchain.toml",
        "docs/reference/api-stability.md",
        "docs/reference/file-format.md",
        "docs/reference/cli.md",
    ] {
        assert!(root.join(file).is_file(), "missing release file: {file}");
    }
}

#[test]
fn published_docs_distinguish_runtime_readers_from_the_converter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let docs = ["README.md", "docs/reference/file-format.md"]
        .into_iter()
        .map(|path| std::fs::read_to_string(root.join(path)).expect("read release documentation"))
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "PACTBIN2",
        "pre-v0.1 Pact files are unsupported",
        "pact-v01-convert",
        "optional fallback reader",
    ] {
        assert!(docs.contains(required), "missing format policy: {required}");
    }
    assert!(!docs.contains("no migration command"));
}
