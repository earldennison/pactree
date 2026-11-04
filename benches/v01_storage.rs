use pactree::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactFileReadHandle,
    ProviderThreadOptions,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn main() {
    let output_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("pactree-v01-bench"));
    std::fs::create_dir_all(&output_dir).expect("create benchmark directory");

    let small = run_workload(&output_dir, 34);
    let large = run_workload(&output_dir, 300);
    let horizon = run_workload(&output_dir, 600);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "pactree.performance.v1",
            "workloads": {
                "34_turn": small,
                "300_turn": large,
                "600_append": horizon,
            }
        }))
        .expect("encode benchmark evidence")
    );
}

fn run_workload(output_dir: &Path, turns: usize) -> Value {
    let rss_before_workload = resident_bytes();
    let path = output_dir.join(format!("{turns}-turn.pact"));
    let _ = std::fs::remove_file(&path);
    let options = ContextTreeOptions {
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new(format!("bench_{turns}_"), 1_000_000, 1)
                .expect("deterministic options"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options(format!("bench_{turns}_root"), options)
        .expect("create benchmark tree");
    tree.save_file(&path).expect("create benchmark file");

    let write_started = Instant::now();
    let mut component_id = String::new();
    let mut content_id = String::new();
    for turn in 0..turns {
        let content = if turn == 0 {
            format!(
                "turn {turn}: {}",
                "externalized benchmark payload ".repeat(48)
            )
        } else {
            format!("turn {turn}: {}", "repeated context payload ".repeat(8))
        };
        let component = Component::text_authoring(content, tree.next_authoring_component_options());
        let trace = tree.insert(component, "root").expect("insert turn");
        if component_id.is_empty() {
            component_id = trace.produced_component_ids[0].clone();
            content_id = tree
                .project_component(&component_id)
                .expect("project benchmark component")["content_ref"]["id"]
                .as_str()
                .expect("externalized benchmark content id")
                .to_string();
        }
        tree.send(&ProviderThreadOptions::default())
            .expect("seal turn");
    }
    let write_micros = write_started.elapsed().as_micros() as u64;
    let write_rss_delta_bytes = resident_bytes().saturating_sub(rss_before_workload);

    let bytes = std::fs::read(&path).expect("read benchmark file");
    let file_hash = blake3::hash(&bytes).to_hex().to_string();
    let physical_layout = physical_layout(&path);
    let projection = tree
        .project(pactree::ProjectionRequest::Json {
            mode: pactree::ViewMode::Current,
        })
        .expect("project semantic head");
    let semantic = match projection {
        pactree::ProjectionOutput::Json(value) => value,
        other => panic!("unexpected benchmark projection: {other:?}"),
    };
    let semantic_bytes = serde_json::to_vec(&semantic).expect("encode semantic head");
    let semantic_hash = blake3::hash(&semantic_bytes).to_hex().to_string();

    let mut open_samples = Vec::new();
    let mut cold_open_rss_delta_bytes = 0;
    for sample in 0..15 {
        let rss_before_open = resident_bytes();
        let started = Instant::now();
        let opened = ContextTree::open_file(&path, None).expect("open benchmark file");
        opened.head().expect("read benchmark head");
        open_samples.push(started.elapsed().as_micros() as u64);
        if sample == 0 {
            cold_open_rss_delta_bytes = resident_bytes().saturating_sub(rss_before_open);
        }
    }

    let rss_before = resident_bytes();
    let handles = (0..20)
        .map(|_| ContextTree::open_file(&path, None).expect("open shared benchmark handle"))
        .collect::<Vec<_>>();
    for handle in &handles {
        handle.head().expect("touch shared benchmark handle");
    }
    let rss_after = resident_bytes();

    let component_samples = sample_micros(15, || {
        PactFileReadHandle::open(&path, None)
            .expect("open cold component handle")
            .component(&component_id)
            .expect("read benchmark component by id");
    });
    let content_samples = sample_micros(15, || {
        PactFileReadHandle::open(&path, None)
            .expect("open cold content handle")
            .content_payload(&content_id)
            .expect("read benchmark content by id");
    });
    let read_handle = PactFileReadHandle::open(&path, None).expect("open benchmark read handle");
    let selector_samples = sample_micros(15, || {
        let mut query =
            pactree::file_format::PactFileSelectorQuery::new("@t0 .text_content @limit=1");
        query.limit = 1;
        pactree::file_format::file_select_json(&path, query)
            .expect("read benchmark current selector");
    });
    let mut stream_samples = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        read_handle
            .stream("current_surface", None, None, "before", 20)
            .expect("stream benchmark window");
        stream_samples.push(started.elapsed().as_micros() as u64);
    }
    let historical_revision = u64::try_from(turns / 2).expect("historical revision");
    let historical_samples = sample_micros(15, || {
        read_handle
            .project(Some(historical_revision))
            .expect("read benchmark historical projection");
    });

    json!({
        "turns": turns,
        "file_bytes": bytes.len(),
        "physical_layout": physical_layout,
        "file_blake3": file_hash,
        "semantic_blake3": semantic_hash,
        "write_micros": write_micros,
        "write_rss_delta_bytes": write_rss_delta_bytes,
        "open_median_micros": percentile(&open_samples, 50),
        "open_p95_micros": percentile(&open_samples, 95),
        "cold_open_micros": open_samples[0],
        "cold_open_rss_delta_bytes": cold_open_rss_delta_bytes,
        "component_by_id_median_micros": percentile(&component_samples, 50),
        "component_by_id_p95_micros": percentile(&component_samples, 95),
        "component_by_id_measurement": "fresh_read_handle_os_page_cache_warm",
        "content_by_id_median_micros": percentile(&content_samples, 50),
        "content_by_id_p95_micros": percentile(&content_samples, 95),
        "content_by_id_measurement": "fresh_read_handle_os_page_cache_warm",
        "current_selector_median_micros": percentile(&selector_samples, 50),
        "current_selector_p95_micros": percentile(&selector_samples, 95),
        "stream_20_median_micros": percentile(&stream_samples, 50),
        "stream_20_p95_micros": percentile(&stream_samples, 95),
        "current_stream_20_median_micros": percentile(&stream_samples, 50),
        "current_stream_20_p95_micros": percentile(&stream_samples, 95),
        "historical_projection_median_micros": percentile(&historical_samples, 50),
        "historical_projection_p95_micros": percentile(&historical_samples, 95),
        "same_file_20_handles_rss_delta_bytes": rss_after.saturating_sub(rss_before),
        "process_peak_rss_bytes": peak_resident_bytes(),
    })
}

fn physical_layout(path: &Path) -> Value {
    let audit_bin = std::env::var_os("PACTREE_STORAGE_AUDIT_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/pactree"));
    let output = Command::new(&audit_bin)
        .arg("storage-audit")
        .arg(path)
        .output()
        .expect("run storage audit");
    assert!(
        output.status.success(),
        "storage audit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let audit: Value = serde_json::from_slice(&output.stdout).expect("decode storage audit");
    audit["physical_layout"].clone()
}

fn sample_micros(mut samples: usize, mut operation: impl FnMut()) -> Vec<u64> {
    let mut elapsed = Vec::with_capacity(samples);
    while samples > 0 {
        let started = Instant::now();
        operation();
        elapsed.push(started.elapsed().as_micros() as u64);
        samples -= 1;
    }
    elapsed
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len().saturating_sub(1) * percentile).div_ceil(100);
    sorted[index]
}

#[cfg(target_os = "linux")]
fn resident_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmRSS:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or_default()
        .saturating_mul(1024)
}

#[cfg(target_os = "linux")]
fn peak_resident_bytes() -> u64 {
    status_bytes("VmHWM:")
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
fn resident_bytes() -> u64 {
    0
}

#[cfg(not(target_os = "linux"))]
fn peak_resident_bytes() -> u64 {
    0
}
