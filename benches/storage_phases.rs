use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use pactree::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs,
    PactFileReadHandle, trace::BoundaryKind,
};
use serde_json::{Value, json};

const PHASES: &[&str] = &[
    "indexed_open",
    "latest_20",
    "materialized_open",
    "first_append",
    "repeated_append",
    "drop_reopen",
    "maintenance",
];

const SCENARIOS: &[&str] = &[
    "turns",
    "payload-sweep",
    "large-body",
    "many-tool-parts",
    "empty",
    "compact-baseline",
    "real-copy",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1).cloned())
    };
    if let Some(phase) = flag("--phase") {
        let file = PathBuf::from(flag("--file").expect("--phase requires --file"));
        let result = run_phase(&phase, &file);
        println!("{result}");
        return;
    }
    let scenario = flag("--scenario").unwrap_or_else(|| {
        eprintln!(
            "usage: --scenario <{}> [--history-scale N] [--output-dir DIR] [--input PATH]",
            SCENARIOS.join("|")
        );
        std::process::exit(2);
    });
    let scale: usize = flag("--history-scale")
        .map(|value| value.parse().expect("--history-scale must be an integer"))
        .unwrap_or(1);
    let output_dir = flag("--output-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pactree-memory-bench"));
    std::fs::create_dir_all(&output_dir).expect("create benchmark directory");
    let input = flag("--input").map(PathBuf::from);
    let report = run_scenario(&scenario, scale, &output_dir, input.as_deref());
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("encode benchmark report")
    );
}

fn run_scenario(scenario: &str, scale: usize, output_dir: &Path, input: Option<&Path>) -> Value {
    let (fixture, description) = generate_fixture(scenario, scale, output_dir, input);
    let fixture_bytes = std::fs::metadata(&fixture).expect("fixture metadata").len();
    let fixture_hash = blake3::hash(&std::fs::read(&fixture).expect("read fixture"))
        .to_hex()
        .to_string();
    let exe = std::env::current_exe().expect("current executable");
    let mut phases = Vec::new();
    for phase in PHASES {
        let file = output_dir.join(format!("{scenario}-{scale}x-{phase}.pact"));
        let _ = std::fs::remove_file(&file);
        std::fs::copy(&fixture, &file).expect("copy fixture for phase");
        let output = Command::new(&exe)
            .arg("--phase")
            .arg(phase)
            .arg("--file")
            .arg(&file)
            .output()
            .expect("spawn phase child");
        let child: Value = if output.status.success() {
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                json!({"phase": phase, "error": format!("unparseable child output: {error}")})
            })
        } else {
            json!({
                "phase": phase,
                "error": String::from_utf8_lossy(&output.stderr).trim().to_string(),
                "exit_code": output.status.code(),
            })
        };
        phases.push(child);
    }
    json!({
        "schema": "pact.memory.benchmark.v1",
        "source": source_identity(&exe),
        "scenario": scenario,
        "history_scale": scale,
        "fixture": {
            "path": fixture.display().to_string(),
            "bytes": fixture_bytes,
            "blake3": fixture_hash,
            "description": description,
        },
        "phases": phases,
    })
}

fn source_identity(exe: &Path) -> Value {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let git = |args: &[&str]| -> Option<String> {
        Command::new("git")
            .arg("-C")
            .arg(manifest_dir)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let rustc = Command::new("rustc")
        .arg("-Vv")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let binary_hash = std::fs::read(exe)
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .unwrap_or_default();
    json!({
        "package_version": env!("CARGO_PKG_VERSION"),
        "git_head": git(&["rev-parse", "HEAD"]),
        "git_dirty": git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
        "rustc": rustc,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "binary_blake3": binary_hash,
    })
}

fn deterministic_options(prefix: &str) -> ContextTreeOptions {
    ContextTreeOptions {
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new(prefix.to_string(), 1_000_000, 1)
                .expect("deterministic authoring options"),
        ),
    }
}

fn stage_turn(tree: &mut ContextTree, turn: usize, parts: Vec<String>) {
    let role = if turn % 2 == 0 { "user" } else { "assistant" };
    let components = parts
        .into_iter()
        .map(|content| Component::text_authoring(content, tree.next_authoring_component_options()))
        .collect();
    tree.add_message(role, components, PactAttrs::new())
        .expect("stage message");
    tree.seal_staged(BoundaryKind::Adhoc).expect("seal turn");
}

fn small_part(turn: usize) -> String {
    format!("turn {turn}: {}", "fixed small part ".repeat(4))
}

fn generate_fixture(
    scenario: &str,
    scale: usize,
    output_dir: &Path,
    input: Option<&Path>,
) -> (PathBuf, String) {
    let path = output_dir.join(format!("{scenario}-{scale}x-fixture.pact"));
    let _ = std::fs::remove_file(&path);
    match scenario {
        "turns" => {
            let turns = 100 * scale.max(1);
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            tree.save_file(&path).expect("create fixture");
            for turn in 0..turns {
                stage_turn(&mut tree, turn, vec![small_part(turn)]);
            }
            (
                path,
                format!("{turns} incremental turns, one fixed small text part each"),
            )
        }
        "payload-sweep" => {
            let sizes = [1_usize << 10, 1 << 12, 1 << 14, 1 << 16, 1 << 18, 1 << 20];
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            tree.save_file(&path).expect("create fixture");
            for (turn, size) in sizes.iter().cycle().take(6 * scale.max(1)).enumerate() {
                stage_turn(&mut tree, turn, vec!["x".repeat(*size)]);
            }
            (
                path,
                format!("{} turns cycling part sizes 1 KiB..1 MiB", 6 * scale.max(1)),
            )
        }
        "large-body" => {
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            tree.save_file(&path).expect("create fixture");
            stage_turn(&mut tree, 0, vec![small_part(0)]);
            stage_turn(&mut tree, 1, vec!["body ".repeat(32 * 1024 * 1024 / 5)]);
            stage_turn(&mut tree, 2, vec![small_part(2)]);
            (
                path,
                "one 32 MiB text body between two small turns".to_string(),
            )
        }
        "many-tool-parts" => {
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            tree.save_file(&path).expect("create fixture");
            for turn in 0..(4 * scale.max(1)) {
                let parts = (0..200)
                    .map(|part| format!("tool part {part} of turn {turn}"))
                    .collect();
                stage_turn(&mut tree, turn, parts);
            }
            (
                path,
                format!("{} turns with 200 parts each", 4 * scale.max(1)),
            )
        }
        "empty" => {
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            tree.save_file(&path).expect("create fixture");
            (path, "empty tree".to_string())
        }
        "compact-baseline" => {
            let turns = 100 * scale.max(1);
            let mut tree =
                ContextTree::with_id_and_options("bench_root", deterministic_options("bench_"))
                    .expect("tree");
            for turn in 0..turns {
                stage_turn(&mut tree, turn, vec![small_part(turn)]);
            }
            tree.save_file(&path).expect("save cold baseline");
            (
                path,
                format!("{turns} turns sealed in memory, saved once as a cold baseline"),
            )
        }
        "real-copy" => {
            let input = input.expect("--scenario real-copy requires --input <disposable copy>");
            std::fs::copy(input, &path).expect("copy real history");
            (path, format!("copy of {}", input.display()))
        }
        other => panic!(
            "unknown scenario {other}; expected one of {}",
            SCENARIOS.join(", ")
        ),
    }
}

struct Measurement {
    elapsed_micros: u64,
    rss_baseline_bytes: u64,
    rss_live_bytes: u64,
    rss_peak_bytes: u64,

    tracked_bytes: usize,
    counters: pactree::PactStorageCounters,
}

fn measure(operation: impl FnOnce()) -> Measurement {
    let rss_baseline_bytes = status_bytes("VmRSS:");
    pactree::reset_storage_counters();
    pactree::reset_storage_memory_peak();
    let started = Instant::now();
    operation();
    let elapsed_micros = started.elapsed().as_micros() as u64;
    let memory = pactree::storage_memory();
    Measurement {
        elapsed_micros,
        rss_baseline_bytes,
        rss_live_bytes: status_bytes("VmRSS:"),
        rss_peak_bytes: status_bytes("VmHWM:"),
        tracked_bytes: memory.cache_resident_bytes + memory.in_flight_peak_bytes,
        counters: pactree::storage_counters(),
    }
}

fn append_one(tree: &mut ContextTree, label: &str) {
    stage_turn(
        tree,
        1,
        vec![format!("{label}: {}", "fixed small part ".repeat(4))],
    );
}

fn run_phase(phase: &str, file: &Path) -> Value {
    let mut temp_bytes = 0_u64;
    let measurement = match phase {
        "indexed_open" => measure(|| {
            let handle = PactFileReadHandle::open(file, None).expect("indexed open");
            handle.info().expect("info");
        }),
        "latest_20" => {
            let handle = PactFileReadHandle::open(file, None).expect("indexed open");
            measure(|| {
                handle
                    .stream("current_surface", None, None, "before", 20)
                    .expect("latest window");
            })
        }
        "materialized_open" => measure(|| {
            let tree = ContextTree::open_file(file, None).expect("materialized open");
            tree.head().expect("head");
        }),
        "first_append" => {
            let mut tree = ContextTree::open_file(file, None).expect("materialized open");
            measure(|| append_one(&mut tree, "first append"))
        }
        "repeated_append" => {
            let mut tree = ContextTree::open_file(file, None).expect("materialized open");
            append_one(&mut tree, "warm-up append");
            measure(|| append_one(&mut tree, "repeated append"))
        }
        "drop_reopen" => {
            let mut tree = ContextTree::open_file(file, None).expect("materialized open");
            append_one(&mut tree, "append before reopen");
            drop(tree);
            measure(|| {
                let tree = ContextTree::open_file(file, None).expect("reopen");
                tree.head().expect("head");
            })
        }
        "maintenance" => {
            let output = file.with_extension("repacked.pact");
            let _ = std::fs::remove_file(&output);
            let measurement = measure(|| {
                pactree::file_format::file_repack_json(file, &output).expect("repack");
            });
            temp_bytes = std::fs::metadata(&output)
                .map(|m| m.len())
                .unwrap_or_default();
            measurement
        }
        other => panic!(
            "unknown phase {other}; expected one of {}",
            PHASES.join(", ")
        ),
    };
    let counters = &measurement.counters;
    json!({
        "phase": phase,
        "elapsed_micros": measurement.elapsed_micros,
        "rss_baseline_bytes": measurement.rss_baseline_bytes,
        "rss_live_bytes": measurement.rss_live_bytes,
        "rss_peak_bytes": measurement.rss_peak_bytes,
        "tracked_bytes": measurement.tracked_bytes,
        "temp_bytes": temp_bytes,
        "decoded_records": counters.records_decoded,
        "decoded_bytes": counters.bytes_read,
        "replayed_operations": counters.operations_replayed,
        "ledger_hydrations": counters.ledger_hydrations,
        "pages_read": counters.pages_read,
        "components_hydrated": counters.components_hydrated,
    })
}

#[cfg(target_os = "linux")]
fn status_bytes(label: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix(label)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or_default()
        .saturating_mul(1024)
}

#[cfg(not(target_os = "linux"))]
fn status_bytes(_label: &str) -> u64 {
    0
}
