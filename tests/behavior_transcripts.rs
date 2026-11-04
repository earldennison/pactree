use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;

use pactree::file_format::{PactForkMode, file_fork_json, file_repair_json};
use pactree::{
    Component, ComponentChanges, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions,
    PactAttrs, PactError, ProjectionOutput, ProjectionRequest, ProviderThreadOptions, ViewMode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Debug, Deserialize)]
struct Scenario {
    schema: String,
    name: String,
    operations: Vec<Operation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    User {
        content: String,
        key: String,
    },
    LargeUser {
        key: String,
    },
    TtlUser {
        content: String,
        key: String,
        ttl: i64,
    },
    Receive {
        content: String,
    },
    Send,
    Insert {
        content: String,
        key: String,
    },
    Update {
        key: String,
        content: String,
    },
    Replace {
        key: String,
        content: String,
    },
    ToolPair {
        call_id: String,
    },
    SaveReopen,
    PruneWithHistoryProbe {
        key: String,
    },
    SoftForkProbe,
    HardFork,
    ReferencePointInTimeProbe,
    StreamCursorProbe,
    InvalidSelectAtomicityProbe,
    ConcurrentReaderProbe,
    TornTailProbe,
}

#[derive(Debug, Serialize)]
struct Observation {
    schema: &'static str,
    step: usize,
    revision: u64,
    view: Value,
    json: Value,
    xml: String,
    provider_thread: Value,
    selectors: Vec<Value>,
    stream: Value,
    traces: Value,
    rewind_targets: Value,
    components: Value,
    content: Value,
    head: Value,
    error: Option<Value>,
}

struct Runner {
    tree: ContextTree,
    file_path: PathBuf,
    temp_root: PathBuf,
    probes: Vec<Value>,
    error: Option<Value>,
}

impl Runner {
    fn new(scenario: &str) -> Self {
        let stable = scenario.replace('-', "_");
        let options = ContextTreeOptions {
            derived_depth: false,
            deterministic_authoring: Some(
                DeterministicAuthoringOptions::new(format!("{stable}_"), 2_000_000, 10)
                    .expect("deterministic options"),
            ),
        };
        let temp_root = std::env::temp_dir().join(format!(
            "pactree-behavior-{}-{scenario}",
            std::process::id()
        ));
        fs::remove_dir_all(&temp_root).ok();
        fs::create_dir_all(&temp_root).expect("create scenario temp root");
        let file_path = temp_root.join("session.pact");
        let mut tree =
            ContextTree::with_id_and_options(format!("{stable}_root"), options).expect("tree");
        tree.save_file(&file_path).expect("save scenario tree");
        Self {
            tree,
            file_path,
            temp_root,
            probes: Vec::new(),
            error: None,
        }
    }

    fn authored_text(&mut self, content: String, key: String) -> Component {
        let mut component =
            Component::text_authoring(content, self.tree.next_authoring_component_options());
        component.key = Some(key);
        component
    }

    fn component_id_for_key(&self, key: &str) -> String {
        self.tree
            .select("*", false)
            .expect("select components")
            .into_iter()
            .find(|component| component.key.as_deref() == Some(key))
            .map(|component| component.id)
            .unwrap_or_else(|| panic!("component key {key:?} not found"))
    }

    fn current_xml(&self) -> String {
        match self
            .tree
            .project(ProjectionRequest::Xml {
                mode: ViewMode::Current,
            })
            .expect("current XML")
        {
            ProjectionOutput::Xml(xml) => xml,
            other => panic!("expected XML, got {other:?}"),
        }
    }

    fn record_error(&mut self, error: &PactError) {
        self.error = Some(json!({
            "kind": error.kind().as_str(),
            "message": error.to_string(),
        }));
    }

    fn apply(&mut self, operation: Operation) {
        match operation {
            Operation::User { content, key } => {
                let component = self.authored_text(content, key);
                self.tree
                    .add_message("user", vec![component], PactAttrs::new())
                    .expect("add user");
            }
            Operation::LargeUser { key } => {
                let component = self.authored_text(format!("large:{}", "x".repeat(1400)), key);
                self.tree
                    .add_message("user", vec![component], PactAttrs::new())
                    .expect("add large user");
            }
            Operation::TtlUser { content, key, ttl } => {
                let mut component = self.authored_text(content, key);
                component.lifecycle.policy.ttl = Some(ttl);
                self.tree
                    .add_message("user", vec![component], PactAttrs::new())
                    .expect("add ttl user");
            }
            Operation::Receive { content } => {
                let component = self.authored_text(content, "assistant".to_string());
                self.tree
                    .receive(vec![component], PactAttrs::new())
                    .expect("receive");
            }
            Operation::Send => {
                self.tree
                    .send(&ProviderThreadOptions::default())
                    .expect("send");
            }
            Operation::Insert { content, key } => {
                let component = self.authored_text(content, key);
                self.tree.insert(component, "root").expect("insert");
            }
            Operation::Update { key, content } => {
                let id = self.component_id_for_key(&key);
                self.tree
                    .update(
                        &id,
                        ComponentChanges {
                            content: Some(content),
                            ..ComponentChanges::default()
                        },
                    )
                    .expect("update");
            }
            Operation::Replace { key, content } => {
                let id = self.component_id_for_key(&key);
                let mut replacement = self.authored_text(content, key);
                replacement.id = id.clone();
                self.tree.replace(replacement, &id).expect("replace");
            }
            Operation::ToolPair { call_id } => {
                let call = Component::tool_call_authoring(
                    call_id.clone(),
                    "shell".to_string(),
                    Some(json!({"command": "pwd"})),
                    self.tree.next_authoring_component_options(),
                );
                let result = Component::tool_result_authoring(
                    call_id,
                    Some("shell".to_string()),
                    Some(true),
                    Some(json!({"stdout": "/workspace"})),
                    None,
                    Some("/workspace".to_string()),
                    self.tree.next_authoring_component_options(),
                );
                self.tree
                    .receive(vec![call, result], PactAttrs::new())
                    .expect("receive tool pair");
            }
            Operation::SaveReopen => {
                let options = ContextTreeOptions {
                    derived_depth: false,
                    deterministic_authoring: Some(
                        DeterministicAuthoringOptions::new("reopened_", 3_000_000, 10)
                            .expect("reopen deterministic options"),
                    ),
                };
                self.tree =
                    ContextTree::open_file_with_options(&self.file_path, Some("main"), options)
                        .expect("reopen tree");
            }
            Operation::PruneWithHistoryProbe { key } => {
                let revision = self.tree.head().expect("head").head_revision;
                let id = self.component_id_for_key(&key);
                self.tree.prune(&id).expect("prune");
                let current_absent = !self.current_xml().contains("dead turn");
                let historical = self
                    .tree
                    .project_at_revision(revision)
                    .expect("historical projection");
                self.probes.push(json!({
                    "probe": "prune_history",
                    "component_id": id,
                    "current_absent": current_absent,
                    "historical": historical,
                }));
            }
            Operation::SoftForkProbe => {
                let report = file_fork_json(
                    &self.file_path,
                    None,
                    PactForkMode::Soft,
                    Some("main"),
                    None,
                    None,
                    1024,
                )
                .expect("soft fork");
                self.probes.push(json!({
                    "probe": "soft_fork",
                    "report": serde_json::from_str::<Value>(&report).expect("soft report"),
                }));
            }
            Operation::HardFork => {
                let child_path = self.temp_root.join("hard-child.pact");
                let report = file_fork_json(
                    &self.file_path,
                    Some(&child_path),
                    PactForkMode::Hard,
                    Some("main"),
                    None,
                    None,
                    1024,
                )
                .expect("hard fork");
                let report: Value = serde_json::from_str(&report).expect("hard report");
                let child =
                    ContextTree::open_file(&child_path, Some("main")).expect("open hard child");
                let child_xml = match child
                    .project(ProjectionRequest::Xml {
                        mode: ViewMode::Current,
                    })
                    .expect("hard child XML")
                {
                    ProjectionOutput::Xml(xml) => xml,
                    other => panic!("expected XML, got {other:?}"),
                };
                self.probes.push(json!({
                    "probe": "hard_fork",
                    "mode": report["mode"],
                    "revision": report["revision"],
                    "child_head": report["child_head"],
                    "fresh_root": report["child_root_id"] != report["parent_ref"]["root_id"],
                    "preserved_content": child_xml.contains("fork base"),
                }));
            }
            Operation::ReferencePointInTimeProbe => {
                let child = self.tree.fork_reference().expect("reference fork");
                let later = self.authored_text("parent later".to_string(), "later".to_string());
                self.tree
                    .insert(later, "root")
                    .expect("parent later insert");
                self.tree
                    .send(&ProviderThreadOptions::default())
                    .expect("seal parent later");
                let child_xml = match child
                    .project(ProjectionRequest::Xml {
                        mode: ViewMode::Current,
                    })
                    .expect("child XML")
                {
                    ProjectionOutput::Xml(xml) => xml,
                    other => panic!("expected XML, got {other:?}"),
                };
                self.probes.push(json!({
                    "probe": "reference_point_in_time",
                    "child_excludes_later_parent_write": !child_xml.contains("parent later"),
                }));
                self.tree = child;
            }
            Operation::StreamCursorProbe => {
                let first = self
                    .tree
                    .stream(
                        "current_surface",
                        None,
                        None,
                        "after",
                        1,
                        Some("<PACT_PATH>"),
                    )
                    .expect("first stream window");
                let row = &first["rows"][0];
                let row_id = row["row_id"].as_str().expect("row id");
                let sequence = row["sequence"].as_u64();
                let second = self
                    .tree
                    .stream(
                        "current_surface",
                        Some(row_id),
                        sequence,
                        "after",
                        2,
                        Some("<PACT_PATH>"),
                    )
                    .expect("second stream window");
                self.probes.push(json!({
                    "probe": "stream_cursor",
                    "first": first,
                    "second": second,
                }));
            }
            Operation::InvalidSelectAtomicityProbe => {
                let before = self.current_xml();
                let error = self
                    .tree
                    .select("$$??>>", false)
                    .expect_err("invalid selector");
                let unchanged = before == self.current_xml();
                self.probes.push(json!({
                    "probe": "error_atomicity",
                    "unchanged": unchanged,
                    "kind": error.kind().as_str(),
                }));
                self.record_error(&error);
            }
            Operation::ConcurrentReaderProbe => {
                let path = self.file_path.clone();
                let results = thread::scope(|scope| {
                    let readers: Vec<_> = (0..2)
                        .map(|_| {
                            let path = path.clone();
                            scope.spawn(move || {
                                let reader = ContextTree::open_file(&path, Some("main"))
                                    .expect("concurrent open");
                                match reader
                                    .project(ProjectionRequest::Xml {
                                        mode: ViewMode::Current,
                                    })
                                    .expect("reader XML")
                                {
                                    ProjectionOutput::Xml(xml) => xml,
                                    other => panic!("expected XML, got {other:?}"),
                                }
                            })
                        })
                        .collect();
                    readers
                        .into_iter()
                        .map(|reader| reader.join().expect("reader join"))
                        .collect::<Vec<_>>()
                });
                self.probes.push(json!({
                    "probe": "concurrent_readers",
                    "identical": results[0] == results[1],
                    "reader_count": results.len(),
                }));
            }
            Operation::TornTailProbe => {
                let before = self.current_xml();
                let mut file = fs::OpenOptions::new()
                    .append(true)
                    .open(&self.file_path)
                    .expect("open torn tail");
                file.write_all(b"INCOMPLETE_TAIL").expect("write torn tail");
                file.sync_all().expect("sync torn tail");
                let repaired_path = self.temp_root.join("repaired.pact");
                file_repair_json(&self.file_path, &repaired_path, true).expect("repair torn tail");
                let recovered = ContextTree::open_file(&repaired_path, Some("main"))
                    .expect("open repaired file");
                self.file_path = repaired_path;
                self.tree = recovered;
                self.probes.push(json!({
                    "probe": "torn_tail_recovery",
                    "preserved_head": before == self.current_xml(),
                }));
            }
        }
    }

    fn observe(&self, step: usize) -> Observation {
        let view = match self
            .tree
            .project(ProjectionRequest::View {
                mode: ViewMode::Current,
            })
            .expect("view")
        {
            ProjectionOutput::View(view) => view,
            other => panic!("expected view, got {other:?}"),
        };
        let json_current = match self
            .tree
            .project(ProjectionRequest::Json {
                mode: ViewMode::Current,
            })
            .expect("json")
        {
            ProjectionOutput::Json(value) => serde_json::from_str(&value).expect("JSON value"),
            other => panic!("expected JSON, got {other:?}"),
        };
        let provider_thread = match self.tree.project(ProjectionRequest::ProviderThread {
            options: ProviderThreadOptions {
                include_partial_tool_calls: true,
                fail_on_orphan_tool_calls: false,
            },
        }) {
            Ok(ProjectionOutput::ProviderThread(value)) => {
                serde_json::to_value(value).expect("provider JSON")
            }
            Ok(other) => panic!("expected provider thread, got {other:?}"),
            Err(error) => json!({"error": error.kind().as_str(), "message": error.to_string()}),
        };
        let selected = self.tree.select("*", false).expect("select all");
        let mut components = Map::new();
        let mut content = Map::new();
        for component in &selected {
            components.insert(
                component.id.clone(),
                self.tree
                    .project_component(&component.id)
                    .expect("project selected component"),
            );
            if let Some(content_id) = component
                .content_ref
                .as_ref()
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
            {
                content.entry(content_id.to_string()).or_insert_with(|| {
                    self.tree
                        .content_payload(content_id)
                        .expect("content payload")
                });
            }
        }
        let mut selectors = self.probes.clone();
        selectors.push(json!({"query": "*", "matches": selected}));
        Observation {
            schema: "pactree.behavior_observation.v1",
            step,
            revision: view.head_revision,
            view: serde_json::to_value(&view).expect("view JSON"),
            json: json_current,
            xml: self.current_xml(),
            provider_thread,
            selectors,
            stream: self
                .tree
                .stream(
                    "current_surface",
                    None,
                    None,
                    "after",
                    100,
                    Some("<PACT_PATH>"),
                )
                .expect("stream"),
            traces: self.tree.traces().expect("traces"),
            rewind_targets: self.tree.rewind_targets().expect("rewind targets"),
            components: Value::Object(components),
            content: Value::Object(content),
            head: serde_json::to_value(self.tree.head().expect("head")).expect("head JSON"),
            error: self.error.clone(),
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.temp_root).ok();
    }
}

fn scenario_files() -> Vec<PathBuf> {
    let mut files = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/scenarios"))
        .expect("read scenarios")
        .map(|entry| entry.expect("scenario entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn normalize_paths(value: &mut Value, temp_root: &Path) {
    match value {
        Value::String(text) => {
            *text = text.replace(&temp_root.display().to_string(), "<TMP>");
        }
        Value::Array(values) => {
            for value in values {
                normalize_paths(value, temp_root);
            }
        }
        Value::Object(values) => {
            if let Some(Value::String(source_path)) = values
                .get_mut("report")
                .and_then(|report| report.get_mut("source_path"))
            {
                let root = temp_root.display();
                if *source_path == format!("{root}/session.pact")
                    || *source_path == format!(r"{root}\session.pact")
                {
                    *source_path = "<TMP>/session.pact".to_string();
                }
            }
            for value in values.values_mut() {
                normalize_paths(value, temp_root);
            }
        }
        _ => {}
    }
}

fn normalize_storage_format_metadata(value: &mut Value) {
    value
        .get_mut("stream")
        .and_then(|stream| stream.get_mut("head"))
        .and_then(Value::as_object_mut)
        .map(|head| head.remove("record_count"));
    if value.get("mode").is_some() {
        value
            .get_mut("head")
            .and_then(Value::as_object_mut)
            .map(|head| head.remove("record_count"));
    }
}

fn first_difference(left: &Value, right: &Value, path: &str) -> Option<String> {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        if let Some(found) = first_difference(left, right, &format!("{path}.{key}"))
                        {
                            return Some(found);
                        }
                    }
                    pair => return Some(format!("{path}.{key}: {pair:?}")),
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!("{path}: lengths {} != {}", left.len(), right.len()));
            }
            left.iter()
                .zip(right)
                .enumerate()
                .find_map(|(index, (left, right))| {
                    first_difference(left, right, &format!("{path}[{index}]"))
                })
        }
        _ if left == right => None,
        _ => Some(format!("{path}: {left:?} != {right:?}")),
    }
}

#[test]
fn report_source_path_normalization_accepts_both_platform_separators() {
    for root in [r"C:\temp\scenario", "/tmp/scenario"] {
        for separator in ['\\', '/'] {
            let mut observation = json!({
                "selectors": [{"report": {
                    "source_path": format!("{root}{separator}session.pact"),
                }}],
            });
            normalize_paths(&mut observation, Path::new(root));
            assert_eq!(
                observation,
                json!({"selectors": [{"report": {"source_path": "<TMP>/session.pact"}}]}),
            );
        }
    }
}

#[test]
fn report_source_path_normalization_preserves_unrelated_backslashes() {
    let root = r"C:\temp\scenario";
    let mut observation = json!({
        "content": r"literal\n and C:\other\session.pact",
        "source_path": format!(r"{root}\session.pact"),
        "selectors": [{"report": {
            "source_path": format!(r"{root}\session.pact\literal"),
            "content": format!(r"prefix {root}\session.pact and literal\n"),
        }}],
    });
    normalize_paths(&mut observation, Path::new(root));
    assert_eq!(
        observation,
        json!({
            "content": r"literal\n and C:\other\session.pact",
            "source_path": r"<TMP>\session.pact",
            "selectors": [{"report": {
                "source_path": r"<TMP>\session.pact\literal",
                "content": r"prefix <TMP>\session.pact and literal\n",
            }}],
        })
    );
}

#[test]
fn current_engine_matches_behavior_transcripts() {
    let mut observed_names = Vec::new();
    for path in scenario_files() {
        let scenario: Scenario = serde_json::from_slice(&fs::read(&path).expect("read scenario"))
            .expect("scenario JSON");
        assert_eq!(scenario.schema, "pactree.behavior_scenario.v1");
        observed_names.push(scenario.name.clone());
        let mut runner = Runner::new(&scenario.name);
        let step = scenario.operations.len();
        for operation in scenario.operations {
            runner.apply(operation);
        }
        let mut actual = serde_json::to_value(runner.observe(step)).expect("observation JSON");
        normalize_paths(&mut actual, &runner.temp_root);
        let golden = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("contracts/observations/current")
            .join(format!("{}.json", scenario.name));
        if std::env::var_os("PACTREE_UPDATE_BEHAVIOR_GOLDENS").is_some() {
            let rendered = format!(
                "{}\n",
                serde_json::to_string_pretty(&actual).expect("render observation")
            );
            fs::create_dir_all(golden.parent().expect("golden parent")).expect("create golden dir");
            fs::write(&golden, rendered).expect("write golden");
        } else {
            let expected = fs::read_to_string(&golden)
                .unwrap_or_else(|error| panic!("read {}: {error}", golden.display()));
            let mut expected: Value = serde_json::from_str(&expected).expect("golden JSON");
            normalize_storage_format_metadata(&mut actual);
            normalize_storage_format_metadata(&mut expected);
            if actual != expected {
                panic!(
                    "behavior drift in {}: {}",
                    scenario.name,
                    first_difference(&actual, &expected, "$").unwrap_or_default()
                );
            }
        }
    }
    assert_eq!(observed_names.len(), 14, "all protected scenarios must run");
}
