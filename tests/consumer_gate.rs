#[test]
fn consumer_gate_installs_only_into_temporary_targets() {
    let script = std::fs::read_to_string("scripts/run-consumer-contracts.sh")
        .expect("read consumer contract runner");
    assert!(script.contains("mktemp -d"));
    assert!(script.contains("pip install"));
    assert!(script.contains("--target"));
    assert!(script.contains("status --porcelain=v1 -z"));
    assert!(script.contains("diff --binary"));
    assert!(script.contains("cmp -s"));
    assert!(script.contains("PYTHONDONTWRITEBYTECODE=1"));
    assert!(!script.contains("maturin develop"));
    assert!(!script.contains("uv sync"));
}
