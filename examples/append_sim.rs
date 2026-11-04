use pactree::{
    Component, ComponentChanges, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions,
    ProviderThreadOptions,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sim.pact".to_string());
    let turns: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(80);
    let _ = std::fs::remove_file(&out);

    let session_id = "sim00000000000000000000000000000001";
    let mut tree =
        ContextTree::with_id_and_options(session_id, deterministic_options()).expect("with_id");
    tree.save_file(&out).expect("save_file (create + bind)");

    let fm_id = seed(&mut tree, "file_manager scaffold");
    let task_id = seed(&mut tree, "task_list scaffold");
    let notes_id = seed(&mut tree, "internal_notes scaffold");
    let sys_id = seed(&mut tree, "system_control scaffold");

    for t in 0..turns {
        let user = text_component(&mut tree, format!("user turn {t}"));
        tree.insert(user, "root").expect("insert user");
        let assistant = text_component(&mut tree, format!("assistant turn {t}"));
        tree.insert(assistant, "root").expect("insert assistant");

        update_many(&mut tree, &fm_id, file_manager_fields(t));
        update_many(&mut tree, &task_id, task_list_fields(t));
        update_many(&mut tree, &notes_id, internal_notes_fields(t));
        update_many(&mut tree, &sys_id, system_control_fields(t));

        tree.send(&ProviderThreadOptions::default())
            .expect("send (seal)");

        if t % 10 == 0 {
            let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            eprintln!("[turn {t:>4}] file_bytes={bytes}");
        }
    }

    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("turns={turns} file={out} bytes={bytes}");
}

fn deterministic_options() -> ContextTreeOptions {
    ContextTreeOptions {
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("sim_", 1_000_000, 1)
                .expect("deterministic options"),
        ),
        ..ContextTreeOptions::default()
    }
}

fn text_component(tree: &mut ContextTree, content: String) -> Component {
    Component::text_authoring(content, tree.next_authoring_component_options())
}

fn seed(tree: &mut ContextTree, label: &str) -> String {
    let component = text_component(tree, label.to_string());
    let tr = tree.insert(component, "root").expect("seed insert");
    let id = first_produced(&tr);
    tree.send(&ProviderThreadOptions::default())
        .expect("seed seal");
    id
}

fn first_produced(tr: &pactree::TraceRef) -> String {
    tr.produced_component_ids
        .first()
        .cloned()
        .unwrap_or_default()
}

fn update_many(tree: &mut ContextTree, id: &str, attrs: BTreeMap<String, Value>) {
    tree.update(
        id,
        ComponentChanges {
            aux: attrs,
            ..ComponentChanges::default()
        },
    )
    .expect("update");
}

fn file_manager_fields(t: usize) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("cwd".into(), json!(format!("/repo/branch-{t}")));
    m.insert(
        "open_files".into(),
        json!([
            format!("src/turn_{t}.rs"),
            "src/lib.rs",
            "src/main.rs",
            "tests/integration.rs",
            "Cargo.toml"
        ]),
    );
    m.insert("last_command".into(), json!("cargo test --release"));
    m.insert("last_exit_code".into(), json!(t.is_multiple_of(7)));
    m.insert("git_branch".into(), json!(format!("feature/iter-{t}")));
    m.insert("git_dirty".into(), json!(true));
    m.insert(
        "modified_files".into(),
        json!(["src/lib.rs", "src/context_tree.rs", "README.md"]),
    );
    m.insert("cursor_file".into(), json!("src/lib.rs"));
    m.insert("cursor_line".into(), json!(120 + t));
    m.insert("undo_stack_depth".into(), json!(t % 5));
    m
}

fn task_list_fields(t: usize) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert(
        "current_task".into(),
        json!(format!("implement feature {t}")),
    );
    m.insert("active_index".into(), json!(t % 3));
    m.insert(
        "tasks".into(),
        json!([
            { "id": format!("t-{t}-1"), "status": "in_progress", "text": "wire up the resolver path" },
            { "id": format!("t-{t}-2"), "status": "pending", "text": "add tests for the append regime" },
            { "id": format!("t-{t}-3"), "status": "done", "text": "land the codec hook" },
            { "id": format!("t-{t}-4"), "status": "pending", "text": "measure the a/b" }
        ]),
    );
    m.insert("completed_count".into(), json!(t / 2));
    m.insert("blocked_on".into(), json!("reviewer"));
    m
}

fn internal_notes_fields(t: usize) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert(
        "working_theory".into(),
        json!(format!(
            "the append path compresses per-page; turn {t} confirms it"
        )),
    );
    m.insert(
        "open_questions".into(),
        json!(["why does repack shrink 4x?", "does interning beat zstd?"]),
    );
    m.insert("scratch".into(), json!(format!("scratchpad iteration {t}")));
    m.insert("confidence".into(), json!(0.6 + 0.01 * (t as f64 % 40.0)));
    m
}

fn system_control_fields(t: usize) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("mode".into(), json!("executing"));
    m.insert("provider".into(), json!("glm-5.2"));
    m.insert("turn_index".into(), json!(t));
    m.insert("tokens_in".into(), json!(1200 + t * 13));
    m.insert("tokens_out".into(), json!(340 + t * 5));
    m.insert(
        "capabilities".into(),
        json!(["read", "write", "bash", "grep", "glob"]),
    );
    m
}
