#[cfg(feature = "compression-zstd")]
use super::*;
#[cfg(feature = "compression-zstd")]
use serde_json::json;

#[cfg(feature = "compression-zstd")]
#[test]
fn semantic_archive_decode_rejects_removed_patch_history() {
    let removed_field = concat!("patch", "es");
    let mut root_value = json!({ "branches": { "main": { "branch_id": "main" } } });
    root_value["branches"]["main"][removed_field] = json!([{ "rev": 1 }]);
    let payload = serde_json::to_vec(&json!({ "root_store": root_value })).expect("payload");
    let compressed = compress_zstd_chunk(&payload).expect("compress");
    let mut bytes = b"PACTAR1\0".to_vec();
    bytes.extend_from_slice(&compressed);

    let err = semantic_archive_root_store(&bytes)
        .expect_err("removed patch-shaped archive history must be rejected");
    assert!(
        err.to_string()
            .contains("removed patch-shaped mutation history"),
        "unexpected error: {err}"
    );
}
