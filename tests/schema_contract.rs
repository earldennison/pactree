use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use pactree::{
    Component, ContextTree, ContextTreeOptions, DeterministicAuthoringOptions, PactAttrs,
    ProjectionOutput, ProjectionRequest, ProviderThreadOptions, ViewMode,
};
use serde::Serialize;
use serde_json::{Value, json};

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn pretty(value: &impl Serialize) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("serialize schema fixture")
    )
}

fn fixture_path(name: &str) -> PathBuf {
    repository_root().join("contracts/schemas").join(name)
}

fn assert_fixture(name: &str, actual: &str) {
    let path = fixture_path(name);
    if std::env::var_os("PACTREE_UPDATE_SCHEMA_FIXTURES").is_some() {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture dir");
        fs::write(&path, actual).expect("write fixture");
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(actual, expected, "schema drift in {}", path.display());
}

fn current_projection(tree: &ContextTree, request: ProjectionRequest) -> ProjectionOutput {
    tree.project(request).expect("project current")
}

#[test]
fn serialized_facets_match_v010_contract() {
    let options = ContextTreeOptions {
        derived_depth: false,
        deterministic_authoring: Some(
            DeterministicAuthoringOptions::new("contract_", 1_000_000, 10)
                .expect("deterministic options"),
        ),
    };
    let mut tree = ContextTree::with_id_and_options("contract_root", options).expect("tree");
    let path = std::env::temp_dir().join(format!(
        "pactree-schema-contract-{}.pact",
        std::process::id()
    ));
    fs::remove_file(&path).ok();
    tree.save_file(&path).expect("save fixture");

    let mut content = Component::text_authoring(
        format!("protected contract payload {}", "x".repeat(1100)),
        tree.next_authoring_component_options(),
    );
    content.key = Some("contract:user:1".to_string());
    tree.add_message("user", vec![content], PactAttrs::new())
        .expect("add message");
    tree.send(&ProviderThreadOptions::default())
        .expect("seal message");

    let selected = tree
        .select("*", false)
        .expect("select contract component")
        .into_iter()
        .find(|component| component.key.as_deref() == Some("contract:user:1"))
        .expect("contract component");
    let content_id = selected
        .content_ref
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .expect("externalized content id")
        .to_string();

    let view = match current_projection(
        &tree,
        ProjectionRequest::View {
            mode: ViewMode::Current,
        },
    ) {
        ProjectionOutput::View(view) => view,
        other => panic!("expected view, got {other:?}"),
    };
    let json_current = match current_projection(
        &tree,
        ProjectionRequest::Json {
            mode: ViewMode::Current,
        },
    ) {
        ProjectionOutput::Json(value) => {
            serde_json::from_str::<Value>(&value).expect("current JSON")
        }
        other => panic!("expected JSON, got {other:?}"),
    };
    let xml_current = match current_projection(
        &tree,
        ProjectionRequest::Xml {
            mode: ViewMode::Current,
        },
    ) {
        ProjectionOutput::Xml(value) => value,
        other => panic!("expected XML, got {other:?}"),
    };
    let provider_thread = match current_projection(
        &tree,
        ProjectionRequest::ProviderThread {
            options: ProviderThreadOptions::default(),
        },
    ) {
        ProjectionOutput::ProviderThread(value) => value,
        other => panic!("expected provider thread, got {other:?}"),
    };

    assert_fixture("pact-view.json", &pretty(&view));
    assert_fixture("json-current.json", &pretty(&json_current));
    assert_fixture("xml-current.xml", &xml_current);
    assert_fixture("provider-thread.json", &pretty(&provider_thread));
    assert_fixture(
        "stream-current.json",
        &pretty(
            &tree
                .stream(
                    "current_surface",
                    None,
                    None,
                    "after",
                    100,
                    Some("<PACT_PATH>"),
                )
                .expect("current stream"),
        ),
    );
    assert_fixture(
        "stream-activity.json",
        &pretty(
            &tree
                .stream(
                    "activity_surface",
                    None,
                    None,
                    "after",
                    100,
                    Some("<PACT_PATH>"),
                )
                .expect("activity stream"),
        ),
    );
    assert_fixture("traces.json", &pretty(&tree.traces().expect("traces")));
    assert_fixture(
        "rewind-targets.json",
        &pretty(&tree.rewind_targets().expect("rewind targets")),
    );
    assert_fixture(
        "component.json",
        &pretty(
            &tree
                .project_component(&selected.id)
                .expect("project component"),
        ),
    );
    assert_fixture(
        "content-payload.json",
        &pretty(&tree.content_payload(&content_id).expect("content payload")),
    );
    assert_fixture("head.json", &pretty(tree.head().expect("head")));

    let invalid = tree
        .select("$$??>>", false)
        .expect_err("invalid selector must fail");
    let missing = tree
        .project_component("missing-contract-component")
        .expect_err("missing component must fail");
    let errors = BTreeMap::from([
        (
            "invalid_selector",
            json!({"kind": invalid.kind().as_str(), "message": invalid.to_string()}),
        ),
        (
            "missing_component",
            json!({"kind": missing.kind().as_str(), "message": missing.to_string()}),
        ),
    ]);
    assert_fixture("errors.json", &pretty(&errors));

    fs::remove_file(path).ok();
}

#[test]
fn search_selector_eligibility_schema_is_stable() {
    let eligibility = pactree::PactSearchSelectorEligibility {
        total_components: 1300,
        eligible_components: 3,
        eligible_ordinals: vec![1, 65, 1299],
        execution_plan: "indexed_bitmap".to_string(),
    };
    assert_eq!(
        serde_json::to_value(eligibility).expect("eligibility JSON"),
        json!({
            "total_components": 1300,
            "eligible_components": 3,
            "eligible_ordinals": [1, 65, 1299],
            "execution_plan": "indexed_bitmap",
        })
    );
}
