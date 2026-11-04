use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn json_stems(directory: PathBuf) -> BTreeSet<String> {
    fs::read_dir(directory)
        .expect("read contract directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect()
}

#[test]
fn final_backend_has_a_frozen_observation_for_every_scenario() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scenarios = json_stems(root.join("contracts/scenarios"));
    let observations = json_stems(root.join("contracts/observations/current"));

    assert_eq!(scenarios.len(), 14);
    assert_eq!(observations, scenarios);
    for scenario in scenarios {
        let bytes = fs::read(
            root.join("contracts/observations/current")
                .join(format!("{scenario}.json")),
        )
        .expect("read current observation");
        let observation: serde_json::Value =
            serde_json::from_slice(&bytes).expect("parse current observation");
        assert_eq!(
            observation["schema"], "pactree.behavior_observation.v1",
            "unexpected observation schema for {scenario}"
        );
        assert!(
            observation["step"].is_u64(),
            "missing observation step for {scenario}"
        );
    }
}
