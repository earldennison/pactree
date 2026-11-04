const BOUNDARY_DOCS: &str = include_str!("../docs/internals/adapter-boundaries.md");

const CORE_RESOLVER_SOURCES: &[(&str, &str)] = &[
    ("src/resolver/api.rs", include_str!("resolver/api.rs")),
    (
        "src/resolver/mutations.rs",
        include_str!("resolver/mutations.rs"),
    ),
    ("src/resolver/tools.rs", include_str!("resolver/tools.rs")),
];

const FORBIDDEN_CORE_WRAPPERS: &[&str] = &[
    "apply_mutation_value(",
    "delete_value(",
    "ledger_delete_json(",
    "pact_node_from_component_json(",
    "pact_node_project_json(",
    "select_json(",
];

fn production_rust_sources(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut sources = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("read production source directory") {
            let path = entry.expect("read production source entry").path();
            if path.is_dir() {
                if path.file_name().and_then(|name| name.to_str()) != Some("tests") {
                    pending.push(path);
                }
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
                && path.file_name().and_then(|name| name.to_str()) != Some("tests.rs")
                && path.file_name().and_then(|name| name.to_str()) != Some("architecture_tests.rs")
            {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

fn contains_invocation(source: &str, invocation: &str) -> bool {
    regex::Regex::new(&format!(r"\b{}", regex::escape(invocation)))
        .expect("valid invocation pattern")
        .is_match(source)
}

fn portable_source_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn source_path_checks_use_portable_separators() {
    for path in [
        "file_format/conversion/unsupported_pre_v01.rs",
        r"file_format\conversion\unsupported_pre_v01.rs",
    ] {
        assert_eq!(
            portable_source_path(std::path::Path::new(path)),
            "file_format/conversion/unsupported_pre_v01.rs"
        );
    }
}

#[test]
fn invocation_guard_detects_bare_and_qualified_forbidden_calls() {
    for (source, invocation) in [
        ("File::create(path)", "File::create("),
        ("std::fs::File::create(path)", "File::create("),
        ("let file = File::create(path)?;", "File::create("),
        ("fs::write(path, bytes)", "fs::write("),
        ("std::fs::write(path, bytes)", "std::fs::write("),
        ("std::fs::write(path, bytes)", "fs::write("),
        (
            "ScratchFile::create(None, 4096)?; File::create(path)",
            "File::create(",
        ),
    ] {
        assert!(
            contains_invocation(source, invocation),
            "must detect {invocation} in {source}"
        );
    }
}

#[test]
fn invocation_guard_ignores_identifier_suffixes() {
    for (source, invocation) in [
        ("ScratchFile::create(None, 4096)", "File::create("),
        (
            "super::scratch::ScratchFile::create(None, 4096)",
            "File::create(",
        ),
        ("_File::create(path)", "File::create("),
        ("Scratch2File::create(path)", "File::create("),
        ("\u{0394}File::create(path)", "File::create("),
        ("Cafe\u{0301}File::create(path)", "File::create("),
        ("scratch_fs::write(path, bytes)", "fs::write("),
        ("scratch_std::fs::write(path, bytes)", "std::fs::write("),
        ("", "File::create("),
    ] {
        assert!(
            !contains_invocation(source, invocation),
            "must not detect identifier suffix {invocation} in {source}"
        );
    }
}

#[test]
fn v0_1_storage_has_one_spine_and_no_legacy_decoder_or_writer() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["storage-dev-dual", "DevBackend", "current_oracle"] {
        assert!(
            !manifest.contains(forbidden),
            "Cargo manifest reintroduced storage selector {forbidden}"
        );
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in production_rust_sources(&root) {
        let source = std::fs::read_to_string(&path).expect("read production Rust source");
        let relative = path.strip_prefix(&root).expect("relative source path");
        let relative = portable_source_path(relative);
        if relative == "file_format/conversion.rs"
            || relative.starts_with("file_format/conversion/")
        {
            continue;
        }

        for forbidden in [
            "DevBackend",
            "storage-dev-dual",
            "legacy_v1",
            "current_oracle",
        ] {
            assert!(
                !source.contains(forbidden),
                "{relative} reintroduced forbidden storage symbol {forbidden}"
            );
        }
        if relative != "pact_file/envelope.rs"
            && relative != concat!("file_format/conversion/", "leg", "acy.rs")
        {
            assert!(
                !source.contains("PACTBIN1"),
                "{relative} may not read or write the rejected pre-v0.1 identity"
            );
        }
        if relative != "atomic_output.rs" {
            for forbidden in ["std::fs::write(", "fs::write(", "File::create("] {
                assert!(
                    !contains_invocation(&source, forbidden),
                    "{relative} bypasses the atomic whole-file publication policy with {forbidden}"
                );
            }
        }
        if relative.starts_with("pact_file/") {
            for forbidden in [
                "project_store_json(",
                "ledger_project_branch_json(",
                "apply_mutation_value(",
                "select_json(",
                "diff_value_trees(",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{relative} performs semantic JSON projection/mutation inside storage: {forbidden}"
                );
            }
        }
    }
}

#[test]
fn one_shot_converter_keeps_old_file_decoding_outside_v0_1_runtime() {
    let runtime = include_str!("file_format/conversion.rs");
    let converter = include_str!("../tools/pact-v01-convert/main.rs");
    let reference = include_str!("../docs/reference/file-format.md");
    let old_magic = concat!("PACT", "BIN1");

    assert!(!runtime.contains(old_magic));
    assert!(converter.contains(old_magic));
    assert!(converter.contains("file-records"));
    assert!(reference.contains("pact-v01-convert --pre-v01-pactree"));
}

fn source_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing source section start `{start}`"));
    let section = &source[start_index..];
    let end_index = section.find(end).unwrap_or(section.len());
    &section[..end_index]
}

#[test]
fn core_resolver_paths_do_not_call_json_operation_wrappers() {
    for (path, source) in CORE_RESOLVER_SOURCES {
        for forbidden in FORBIDDEN_CORE_WRAPPERS {
            assert!(
                !source.contains(forbidden),
                "{path} must not call JSON operation wrapper `{forbidden}`"
            );
        }
    }
}

#[test]
fn mutation_value_adapter_delegates_to_component_tree() {
    let production = include_str!("mutation.rs");
    let tests = include_str!("mutation/tests.rs");
    assert!(tests.contains("ComponentTree::from_value"));
    assert!(tests.contains("apply_mutation_tree(&mut component_tree"));
    assert!(tests.contains("prune_tree(&mut component_tree"));
    assert!(!production.contains("pub(crate) fn apply_mutation_value"));
    assert!(!production.contains("pub(crate) fn apply_mutation_tree_value"));
    assert!(!production.contains("pub(crate) fn delete_value"));
    assert!(!production.contains("fn apply_insert_value"));
    assert!(!production.contains("fn apply_update_value"));
}

#[test]
fn resolver_stages_native_component_trees() {
    let source = include_str!("resolver.rs");
    assert!(
        source.contains("staged_state: BTreeMap<String, (ComponentTree, Vec<MutationRequest>)>")
    );
    assert!(!source.contains("staged_state: BTreeMap<String, (Value, Vec<MutationRequest>)>"));
}

#[test]
fn context_tree_is_public_working_view_facade_not_ledger() {
    let source = include_str!("context_tree.rs");
    assert!(source.contains("pub struct ContextTree"));
    assert!(source.contains("resolver: PactResolver"));
    assert!(source.contains("branch_id: String"));
    assert!(source.contains("pub fn new("));
    assert!(source.contains("pub fn open_file("));
    assert!(source.contains("pub fn view(&self) -> PactResult<PactView>"));
    assert!(source.contains("pub fn preview(&self) -> PactResult<PactView>"));
    assert!(source.contains("pub fn prune("));
    assert!(source.contains("pub fn send("));
    assert!(source.contains("pub fn receive("));
    assert!(source.contains("ProjectionRequest"));
    assert!(source.contains("ProjectionOutput"));
    assert!(source.contains("ProjectionRequest::ProviderThread"));
    assert!(source.contains("ProjectionRequest::Xml"));
    assert!(source.contains("ProjectionRequest::Json"));
    assert!(source.contains("ProjectionOutput::ProviderThread"));
    assert!(source.contains("ProjectionOutput::Xml"));
    assert!(source.contains("ProjectionOutput::Json"));
    assert!(source.contains("let mut tree = ContextTree::new()?;"));
    assert!(source.contains("pub fn with_id("));
    assert!(source.contains("pub fn id(&self) -> &str"));
    assert!(source.contains("pub fn context_tree_id(&self) -> &str"));
    assert!(source.contains("tree.insert(component, Placement::from(\"root\"))?;"));
    assert!(source.contains("tree.send(&ProviderThreadOptions::default())?;"));
    assert!(source.contains("pub struct Placement"));
    assert!(source.contains("pub enum PlacementTarget"));

    for forbidden in [
        "pub fn ledger(",
        "pub fn resolver(",
        "pub fn component_tree(",
        "pub fn delete(",
        "pub fn insert_report(",
        "pub fn append_report(",
        "pub fn update_report(",
        "pub fn replace_report(",
        "pub fn upsert_report(",
        "pub fn prune_report(",
        "pub fn send_report(",
        "pub fn insert_value_report(",
        "pub fn append_value_report(",
        "pub fn replace_value_report(",
        "pub fn upsert_value_report(",
        "pub fn provider_thread(",
        "pub fn xml(",
        "pub fn json(",
        "pub fn into_ledger(",
        "pub fn into_resolver(",
    ] {
        assert!(
            !source.contains(forbidden),
            "ContextTree must not expose substrate accessor `{forbidden}`"
        );
    }
    for forbidden_signature in [
        "pub fn insert(\n        &mut self,\n        component: Component,\n        target_selector:",
        "pub fn append(\n        &mut self,\n        component: Component,\n        target_component_id:",
        "pub fn update(\n        &mut self,\n        target_component_id:",
        "pub fn replace(\n        &mut self,\n        component: Component,\n        target_component_id:",
        "pub fn upsert(\n        &mut self,\n        component: Component,\n        target_component_id:",
        "pub fn prune(\n        &mut self,\n        target_component_id:",
    ] {
        assert!(
            !source.contains(forbidden_signature),
            "ContextTree write surface leaked stale selector/target plumbing"
        );
    }
    for forbidden_return in [
        "-> PactResolver",
        "-> &PactResolver",
        "-> ContextTreeLedger",
        "-> &ContextTreeLedger",
        "-> ComponentTree",
        "-> &ComponentTree",
    ] {
        assert!(
            !source.contains(forbidden_return),
            "ContextTree public API must not return substrate type `{forbidden_return}`"
        );
    }

    let store_model = include_str!("store/model.rs");
    assert!(!store_model.contains("pub struct ContextTree {"));
    assert!(BOUNDARY_DOCS.contains("Mutable native execution substrate"));
}

#[test]
fn transaction_trace_module_exports_trace_vocabulary() {
    let module_source = include_str!("transaction_trace/mod.rs");
    let trace_op_source = include_str!("transaction_trace/trace_op.rs");
    let encode_source = include_str!("transaction_trace/encode.rs");
    let pack_source = include_str!("pact_pack/mod.rs");

    assert!(module_source.contains("pub struct TransactionTrace"));
    assert!(module_source.contains("pub ops: Vec<TraceOp>"));
    assert!(module_source.contains("pub use trace_op::{ComponentChange, TraceOp, TraceOpKind};"));
    assert!(trace_op_source.contains("pub struct TraceOp"));
    assert!(trace_op_source.contains("pub enum TraceOpKind"));
    assert!(trace_op_source.contains("pub enum ComponentChange"));
    assert!(trace_op_source.contains("pub fn into_trace_op_kind(self) -> TraceOpKind"));
    assert!(trace_op_source.contains("InsertComponent"));
    assert!(trace_op_source.contains("DeleteComponent"));
    assert!(trace_op_source.contains("MoveComponent"));

    for (path, source) in [
        ("src/transaction_trace/mod.rs", module_source),
        ("src/transaction_trace/trace_op.rs", trace_op_source),
        ("src/transaction_trace/encode.rs", encode_source),
        ("src/pact_pack/mod.rs", pack_source),
    ] {
        for forbidden in [
            concat!("Patch", "Ledger"),
            concat!("Node", "Delta"),
            concat!("Node", "DeltaKind"),
            concat!("Insert", "Node"),
            concat!("Delete", "Node"),
            concat!("Move", "Node"),
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not use old replay vocabulary `{forbidden}`"
            );
        }
    }
}

#[test]
fn current_file_wire_ops_use_component_vocabulary() {
    let wire_source = include_str!("file_format/wire.rs");
    for required in [
        "InsertComponent",
        "ReplaceComponent",
        "DeleteComponent",
        "AppendComponents",
        "SetComponentAux",
        "UnsetComponentAux",
    ] {
        assert!(
            wire_source.contains(required),
            "missing current wire op {required}"
        );
    }
    for forbidden in [
        concat!("Insert", "Node"),
        concat!("Replace", "Node"),
        "DeleteSubtree",
        "AppendChildren",
        concat!("Set", "NodeAttr"),
        concat!("Unset", "NodeAttr"),
    ] {
        assert!(
            !wire_source.contains(forbidden),
            "current file wire must not expose old op {forbidden}"
        );
    }
}

#[test]
fn resolver_tool_cleanup_uses_component_tree() {
    let source = include_str!("resolver/tools.rs");
    assert!(source.contains("tree: &mut ComponentTree"));
    assert!(source.contains("sync_tool_pair_lifecycle(\n    tree: &mut ComponentTree"));
    assert!(!source.contains("apply_mutation_tree_value"));
}

#[test]
fn resolver_mutation_lowering_uses_native_selector_helpers() {
    let source = include_str!("resolver/mutations.rs");
    assert!(source.contains("tree: &ComponentTree"));
    assert!(source.contains("selected_component_ids_tree(tree"));
    for forbidden in [
        "prune_target_ids_value",
        "resolve_parent_id(",
        "resolve_target_id(",
        "selected_node_ids(",
        "find_node_by_id",
    ] {
        assert!(
            !source.contains(forbidden),
            "resolver/mutations.rs must not call JSON selector helper `{forbidden}`"
        );
    }
}

#[test]
fn native_selector_entry_points_execute_over_components() {
    let source = include_str!("selector.rs");
    for ast_type in [
        "struct SelectorStep",
        "struct SimpleSelector",
        "struct AttributeSelector",
        "struct BehaviorSelector",
        "struct PseudoSelector",
        "enum AttributeOperator",
        "enum Combinator",
    ] {
        assert!(
            source.contains(ast_type),
            "missing selector AST type {ast_type}"
        );
    }
    assert!(source.contains("impl SelectorNode for Component"));
    assert!(source.contains("\"node_type\" => None"));
    assert!(source.contains("\"cad\" | \"cadence\" => self\n                .lifecycle\n                .policy\n                .cad"));
    assert!(source.contains("fn select_tree_matches(tree: &ComponentTree"));
    assert!(source.contains("fn indexed_selector_seed_ids(tree: &ComponentTree"));
    assert!(source.contains("tree.ids_by_type(component_type).to_vec()"));
    assert!(source.contains("let matches = select_tree_matches(tree, selector)?;"));
    assert!(source.contains("let tree = ComponentTree::from_value(&value)?;"));
    assert!(source.contains("let matches = select_tree_matches(&tree, selector)?;"));
    assert!(!source.contains("PactTreeComponent"));
    assert!(!source.contains("SerializedSelectorComponent"));
    assert!(!source.contains("select_serialized"));
}

#[test]
fn resolver_provider_thread_egress_uses_component_tree_renderer() {
    let resolver_source = include_str!("resolver/api.rs");
    assert!(resolver_source.contains("build_provider_thread_tree_with_content_store"));
    assert!(!resolver_source.contains(
        "crate::provider_thread::build_provider_thread_with_content_store(\n            &materialized"
    ));

    let provider_source = include_str!("provider_thread.rs");
    assert!(provider_source.contains("pub(crate) fn build_provider_thread_tree("));
    assert!(provider_source.contains("tree: &ComponentTree"));
    assert!(provider_source.contains("build_provider_thread_tree(&hydrated, options)"));
    assert!(!provider_source.contains(
        "fn build_reasoning_content_component(node: &Component) -> Option<Value> {\n    build_reasoning_content(&node.project_component_value())"
    ));
    assert!(!provider_source.contains(
        "fn build_multimedia_content_component(node: &Component) -> Option<Value> {\n    build_multimedia_content(&node.project_component_value(), node.component_type())"
    ));
}

#[test]
fn native_provider_thread_builder_does_not_take_value() {
    let source = include_str!("provider_thread.rs");
    assert!(source.contains(
        "pub(crate) fn build_provider_thread_tree(\n    tree: &ComponentTree,\n    options: &ProviderThreadOptions,"
    ));
    assert!(BOUNDARY_DOCS.contains("This is a compatibility adapter; provider-thread behavior"));
    assert!(source.contains("ComponentTree::from_value(serialized_projection)"));
    assert!(source.contains("build_provider_thread_tree(&tree, options)"));
    assert!(!source.contains("fn build_provider_thread_value("));
    assert!(!source.contains("pub fn build_provider_thread_tree(\n    materialized: &Value"));
}

#[test]
fn core_projection_module_defines_selector_and_component_id_sinks() {
    let source = include_str!("projection/tests.rs");
    for required in [
        "pub(crate) struct SelectorSink",
        "pub(crate) struct ComponentIdHydrationSink",
        "impl ProjectionSink for SelectorSink",
        "impl ProjectionSink for ComponentIdHydrationSink",
        "select_components_tree(&tree, &self.selector, self.include_archived)",
        ".filter_map(|component_id| tree.component(component_id).cloned())",
    ] {
        assert!(
            source.contains(required),
            "projection/tests.rs missing `{required}`"
        );
    }
}

#[test]
fn xml_rendering_has_native_component_entry_points() {
    let source = include_str!("render.rs");
    assert!(source.contains("pub fn render_component_xml(root: &Component)"));
    assert!(source.contains("pub(crate) fn render_tree_xml(tree: &ComponentTree)"));
    assert!(source.contains("struct XmlComponent"));
    assert!(source.contains("fn from_component(component: &'a Component"));
    assert!(source.contains("fn render_xml_component(component: &XmlComponent<'_>"));
    assert!(source.contains("for (key, value) in &component.attrs"));
}

#[test]
fn resolver_has_typed_component_authoring_methods() {
    let source = include_str!("resolver/api.rs");
    for method in [
        "pub fn insert_component(",
        "pub fn replace_component(",
        "pub fn append_component(",
        "pub fn upsert_component(",
        "pub fn insert_json(",
        "pub fn replace_json(",
        "pub fn append_json(",
        "pub fn upsert_json(",
    ] {
        assert!(
            source.contains(method),
            "missing typed authoring method {method}"
        );
    }
    assert!(source.contains("fn component_from_authoring_value(value: Value"));
    assert!(source.contains("component_from_authoring_value(component, \"insert\")"));
    assert!(source.contains("component_from_authoring_value(component, \"append\")"));
    assert!(source.contains("component_from_authoring_value(component, \"replace\")"));
    assert!(source.contains("component_from_authoring_value(component, \"upsert\")"));
    for plain_value_method in [
        "pub fn insert(\n",
        "pub fn replace(\n",
        "pub fn append(\n",
        "pub fn upsert(\n",
    ] {
        assert!(
            !source.contains(plain_value_method),
            "serialized primitive authoring wrappers must be explicit *_json adapters"
        );
    }
    assert!(BOUNDARY_DOCS.contains("Serialized component adapter for insert"));
    assert!(BOUNDARY_DOCS.contains("Serialized component adapter for replace"));
    assert!(BOUNDARY_DOCS.contains("Serialized component adapter for append"));
    assert!(BOUNDARY_DOCS.contains("Serialized component adapter for upsert"));
}

#[test]
fn resolver_add_message_lowers_parts_through_components() {
    let source = include_str!("resolver/api.rs");
    assert!(source.contains("pub fn add_message("));
    assert!(source.contains("parts: Vec<Component>"));
    assert!(source.contains("pub fn add_message_json("));
    assert!(source.contains("fn message_parts_from_values("));

    assert!(source.contains("lower_message(&root_id, branch_id, role, parts, aux, self)?"));
    let shared = include_str!("resolver/message_authoring.rs");
    assert!(shared.contains("let mut children: Vec<Component> = Vec::new();"));
    assert!(shared.contains("message_turn_component(message_id, root_id, role, children, aux)?"));
    assert!(!source.contains("component_from_authoring_value(node, \"add_message\")"));
    assert!(
        !source.contains("component_from_authoring_value(Value::Object(message), \"add_message\")")
    );
    assert!(source.contains("component,"));
}

#[test]
fn lifecycle_cadence_clone_uses_component_tree() {
    let source = include_str!("resolver/lifecycle.rs");
    assert!(source.contains("fn lifecycle_cadence_clone_tree(\n    tree: &ComponentTree"));
    assert!(source.contains("tree.component(component_id)"));
    assert!(!source.contains("fn lifecycle_cadence_clone_value("));
}

#[test]
fn lifecycle_expiry_query_uses_component_tree() {
    let lifecycle_source = include_str!("resolver/lifecycle.rs");
    assert!(
        lifecycle_source.contains("fn lifecycle_expired_top_components(\n    tree: &ComponentTree")
    );
    assert!(lifecycle_source.contains(".ttl_candidate_ids()"));
    assert!(lifecycle_source.contains("tree.tool_pair_ids(tool_call_id)"));
    assert!(!lifecycle_source.contains("PactProjection"));
    assert!(!lifecycle_source.contains("component_records()"));

    let resolver_source = include_str!("resolver/api.rs");
    assert!(resolver_source.contains("lifecycle_expired_top_components(&tree"));
    assert!(!resolver_source.contains("lifecycle_expired_top_nodes("));
}

#[test]
fn typed_authoring_preserves_native_lifecycle_payloads() {
    let ops_source = include_str!("ops.rs");
    assert!(ops_source.contains("use crate::components::Component;"));
    assert_eq!(ops_source.matches("component: Component").count(), 4);
    assert!(!ops_source.contains("component: Value"));

    let override_map = concat!("typed_component_", "overrides");
    let stage_override = concat!("stage_typed_component_", "override");
    let take_override = concat!("take_typed_component_", "override");
    let remove_override = concat!("remove_typed_component_", "override");
    let lowering_with_component = concat!("op_to_mutations_", "with_component");

    let resolver_source = include_str!("resolver.rs");
    assert!(!resolver_source.contains(override_map));
    assert!(!resolver_source.contains("TypedComponentOverride"));
    assert!(!resolver_source.contains("typed_component: Option<Component>"));

    let api_source = include_str!("resolver/api.rs");
    assert!(!api_source.contains("resolve_and_seal_typed_component_operation"));
    assert!(!api_source.contains("resolve_typed_component_operation"));
    assert!(!api_source.contains(stage_override));
    assert!(!api_source.contains(take_override));
    assert!(!api_source.contains(remove_override));
    assert!(!api_source.contains(lowering_with_component));
    assert!(api_source.contains("build_projection_from_component(&tree_component"));
    assert!(api_source.contains("pub fn receive(\n        &mut self,\n        branch_id: &str,\n        parts: Vec<Component>"));
    assert!(api_source.contains("pub fn receive_json("));
    assert!(BOUNDARY_DOCS.contains("Serialized provider-response adapter"));

    let mutation_source = include_str!("resolver/mutations.rs");
    assert!(!mutation_source.contains(lowering_with_component));
    assert!(!mutation_source.contains("fn operation_component("));
    assert!(mutation_source.contains("pub(super) fn op_to_mutations("));

    let context_tree_source = include_str!("context_tree.rs");
    assert!(!context_tree_source.contains("component: component.project_component_value()"));
    assert!(context_tree_source.contains(".receive(&self.branch_id, parts, aux)"));
}

#[test]
fn component_tree_mutation_is_coherent_by_construction() {
    let tree_source = include_str!("tree.rs");

    assert!(
        tree_source.contains("    root: Component,"),
        "ComponentTree.root must be a private field"
    );
    assert!(
        !tree_source.contains("pub root: Component"),
        "ComponentTree.root must not be public"
    );
    assert!(
        tree_source.contains("pub fn root(&self) -> &Component"),
        "ComponentTree must expose a read-only root() accessor"
    );

    assert!(
        tree_source.contains(
            "pub(crate) fn edit<T>(\n        &mut self,\n        f: impl FnOnce(&mut Component) -> PactResult<T>,\n    ) -> PactResult<T>"
        ),
        "ComponentTree::edit closure-based mutation entry point must exist"
    );
    assert!(
        tree_source.contains("let mut candidate_root = self.root.clone();"),
        "edit must operate on a cloned candidate root"
    );
    assert!(
        tree_source
            .contains("let candidate_index = ComponentTreeIndex::try_derive(&candidate_root)?;"),
        "edit must derive + validate a candidate index before commit"
    );
    assert!(
        tree_source
            .contains("#[cfg(debug_assertions)]\n        self.debug_assert_index_coherent();"),
        "the debug coherence assert must be present in the edit path"
    );
    assert!(
        tree_source.contains("fn try_derive(root: &Component) -> PactResult<Self>"),
        "ComponentTreeIndex::try_derive must exist as the fallible derivation path"
    );
    assert!(
        tree_source.contains("fn debug_assert_index_coherent(&self)"),
        "debug_assert_index_coherent helper must exist"
    );

    for forbidden in [
        "pub fn component_mut",
        "pub(crate) fn component_mut",
        "pub fn children_mut",
        "pub(crate) fn children_mut",
        "fn component_mut(&mut self",
        "fn children_mut(&mut self",
    ] {
        assert!(
            !tree_source.contains(forbidden),
            "ComponentTree must not expose raw mutable accessor `{forbidden}`"
        );
    }

    assert!(
        !tree_source.contains("&mut self, component_id: &str) -> Option<&mut Component>"),
        "no method may return &mut Component from the live tree"
    );
    assert!(
        !tree_source.contains("&mut self, component_id: &str) -> Option<&mut Vec<Component>>"),
        "no method may return &mut Vec<Component> from the live tree"
    );

    for forbidden in [
        "pub fn index_subtree_at_path",
        "pub(crate) fn index_subtree_at_path",
        "pub fn sort_children_recursive",
        "pub(crate) fn sort_children_recursive",
    ] {
        assert!(
            !tree_source.contains(forbidden),
            "`{forbidden}` must be private on ComponentTree"
        );
    }

    let mutation_source = include_str!("mutation.rs");
    assert!(
        mutation_source.contains("tree.edit(|root|"),
        "mutation engine must route structural edits through ComponentTree::edit"
    );
    assert!(
        !mutation_source.contains("&mut tree.root.children"),
        "mutation engine must not mutate tree.root.children directly"
    );
    assert!(
        !mutation_source.contains("tree.rebuild_index()"),
        "mutation engine must not call rebuild_index manually"
    );
    assert!(
        !mutation_source.contains("tree.children_mut("),
        "mutation engine must not call ComponentTree::children_mut"
    );
    assert!(
        !mutation_source.contains("tree.component_mut("),
        "mutation engine must not call ComponentTree::component_mut"
    );

    let tools_source = include_str!("resolver/tools.rs");
    assert!(
        tools_source.contains("tree.edit(|root|"),
        "resolver tool cleanup must route structural edits through ComponentTree::edit"
    );
    assert!(
        !tools_source.contains("&mut tree.root"),
        "resolver tool cleanup must not mutate tree.root directly"
    );
    assert!(
        !tools_source.contains("tree.rebuild_index()"),
        "resolver tool cleanup must not call rebuild_index manually"
    );
}

#[test]
fn component_tree_index_is_private_and_bucketed() {
    let tree_source = include_str!("tree.rs");
    assert!(tree_source.contains("index: ComponentTreeIndex"));
    assert!(!tree_source.contains("pub index: ComponentTreeIndex"));
    assert!(tree_source.contains("struct ComponentTreeIndex"));
    assert!(!tree_source.contains("pub struct ComponentTreeIndex"));
    for bucket in [
        "by_type",
        "by_role",
        "by_key",
        "by_tag",
        "by_lifecycle_state",
        "ttl_candidates",
        "cad_candidates",
        "tool_calls_by_call_id",
        "tool_results_by_call_id",
    ] {
        assert!(
            tree_source.contains(bucket),
            "missing private index bucket {bucket}"
        );
    }
    for helper in [
        "pub(crate) fn ids_by_type",
        "pub(crate) fn ids_by_role",
        "pub(crate) fn id_by_key",
        "pub(crate) fn ids_by_tag",
        "pub(crate) fn ids_by_lifecycle_state",
        "pub(crate) fn ttl_candidate_ids",
        "pub(crate) fn cad_candidate_ids",
        "pub(crate) fn tool_pair_ids",
    ] {
        assert!(tree_source.contains(helper), "missing tree helper {helper}");
    }

    let lib_source = include_str!("lib.rs");
    assert!(!lib_source.contains("ComponentTreeIndex"));

    let lifecycle_source = include_str!("resolver/lifecycle.rs");
    assert!(lifecycle_source.contains(".ttl_candidate_ids()"));
    assert!(lifecycle_source.contains("tree.tool_pair_ids(tool_call_id)"));

    let selector_source = include_str!("selector.rs");
    assert!(selector_source.contains("fn indexed_selector_seed_ids(tree: &ComponentTree"));

    let tools_source = include_str!("resolver/tools.rs");
    assert!(tools_source.contains("tree.tool_pair_ids(&tool_call_id)"));

    let provider_source = include_str!("provider_thread.rs");
    assert!(provider_source.contains("tree.indexed_unpaired_tool_call_ids()"));

    assert!(!tree_source.contains("ComponentRegistry"));
}

#[test]
fn component_lifecycle_unification_has_file_row_guards() {
    let components_source = include_str!("components.rs");
    let store_model_source = include_str!("store/model.rs");
    let view_source = include_str!("view.rs");
    let projection_source = include_str!("store/projection.rs");
    let record_codec_source = include_str!("file_format/record_codec.rs");

    let production_component_record_defs = [
        components_source,
        store_model_source,
        view_source,
        projection_source,
        record_codec_source,
    ]
    .iter()
    .map(|source| source.matches("struct ComponentRecord").count())
    .sum::<usize>();
    assert_eq!(
        production_component_record_defs, 1,
        "there must be exactly one production ComponentRecord definition"
    );

    let component_struct = source_section(
        components_source,
        "pub struct Component {",
        "impl serde::Serialize for Component",
    );
    for forbidden_field in [
        "pub attrs:",
        "pub org:",
        "pub ttl:",
        "pub cad:",
        "pub cadence:",
        "current_state_ref",
        "current_projection_ref",
    ] {
        assert!(
            !component_struct.contains(forbidden_field),
            "public Component must not expose stale top-level field `{forbidden_field}`"
        );
    }
    assert!(component_struct.contains("pub aux: PactAttrs"));
    assert!(component_struct.contains("pub lifecycle: ComponentLifecycle"));

    let component_record_struct = source_section(
        store_model_source,
        "pub(crate) struct ComponentRecord {",
        "impl ComponentRecord",
    );
    assert!(component_record_struct.contains("pub lifecycle: ComponentLifecycle"));
    for forbidden_field in [
        "current_state_ref",
        "current_projection_ref",
        "pub org:",
        "pub attrs:",
    ] {
        assert!(
            !component_record_struct.contains(forbidden_field),
            "ComponentRecord must not expose stale top-level field `{forbidden_field}`"
        );
    }

    assert!(store_model_source.contains("fn component_record_lifecycle(component: &Component)"));
    assert!(store_model_source.contains("policy: ComponentLifecyclePolicy::default()"));
    assert!(
        store_model_source.contains("internal_refs: component.lifecycle.internal_refs.clone()")
    );
    assert!(record_codec_source.contains("\"lifecycle\","));
}

#[test]
fn ledger_delete_json_is_native_prune_adapter() {
    let source = include_str!("store/api.rs");
    assert!(source.contains("impl ContextTreeLedger"));
    assert!(source.contains("pub fn prune("));
    assert!(source.contains("root_store.prune("));
    assert!(!source.contains("let mut resolver = PactResolver::from_ledger(root_store);"));
}

#[test]
fn ledger_project_and_inspect_wrappers_use_native_methods() {
    let source = include_str!("store/api.rs");
    for method in [
        "pub fn project_branch_with_scope(",
        "pub fn project_branch(",
        "pub fn inspect(&self) -> Value",
        "pub fn inspect_branch(&self, branch_id: &str)",
    ] {
        assert!(
            source.contains(method),
            "missing native ledger method {method}"
        );
    }
    assert!(source.contains("root_store.project_branch_with_scope("));
    assert!(source.contains("root_store.project_branch(branch_id, revision, blob_inline_limit)"));
    assert!(source.contains("serde_json::to_string(&root_store.inspect())"));
    assert!(source.contains("serde_json::to_string(&root_store.inspect_branch(branch_id)?)"));
}

#[test]
fn ledger_fork_wrappers_use_native_methods() {
    let source = include_str!("store/api.rs");
    assert!(source.contains("pub fn fork_at_revision("));
    assert!(source.contains("pub fn fork_at_component("));
    assert!(source.contains("root_store.fork_at_revision("));
    assert!(source.contains("root_store.fork_at_component("));
}

#[test]
fn ledger_trace_wrappers_use_native_methods() {
    let source = include_str!("store/api.rs");
    assert!(source.contains("pub fn record_transaction_trace("));
    assert!(source.contains("pub fn fork_at_transaction("));
    assert!(source.contains("root_store.record_transaction_trace(trace)"));
    assert!(source.contains("root_store.fork_at_transaction("));
}

#[test]
fn runtime_sealing_records_transaction_traces_directly() {
    let resolver_source = include_str!("resolver/api.rs");
    let old_branch_patch_push = concat!("branch_mut.", "patches.push");
    let direct_trace_lowering = source_section(
        resolver_source,
        "fn mutation_result_trace_ops(",
        "fn sealed_transaction_trace_from_boundary(",
    );
    assert!(resolver_source.contains("fn sealed_transaction_trace_from_boundary("));
    assert!(resolver_source.contains("branch_mut.transaction_traces.push(trace);"));
    assert!(resolver_source.contains("replay_trace,"));
    assert!(!resolver_source.contains("diff_surface("));
    assert!(resolver_source.contains("direct trace lowering incomplete for accepted"));
    assert!(resolver_source.contains("did not replay to the post-mutation tree"));
    assert!(!resolver_source.contains(old_branch_patch_push));
    assert!(direct_trace_lowering.contains("MutationKind::Update if request.node.is_some()"));
    assert!(direct_trace_lowering.contains("MutationKind::Move"));
    assert!(direct_trace_lowering.contains("TraceOpKind::DeleteComponent"));
    assert!(direct_trace_lowering.contains("TraceOpKind::MoveComponent"));
    assert!(direct_trace_lowering.contains("append_insert_trace_ops(after_tree, component_id"));
    assert!(direct_trace_lowering.contains("append_report_position_trace_ops(report"));
    assert!(direct_trace_lowering.contains("top_level_removed_component_ids(before_tree"));
    assert!(direct_trace_lowering.contains("for shift in &report.shifted_offsets"));
    assert!(direct_trace_lowering.contains("for shift in &report.shifted_depths"));
}

#[test]
fn staged_trace_handles_are_private_operation_discard_metadata_until_seal() {
    let trace_source = include_str!("trace.rs");
    let context_tree_source = include_str!("context_tree.rs");

    assert!(trace_source.contains("pub enum StagedTraceTarget"));
    assert!(BOUNDARY_DOCS.contains("These ops become durable replay data only when seal writes"));
    assert!(BOUNDARY_DOCS.contains("not a second durable"));
    assert!(BOUNDARY_DOCS.contains("Private staged operation journal"));
    assert!(BOUNDARY_DOCS.contains("re-resolves the remaining operations"));
    assert!(context_tree_source.contains("pub fn discard_traces("));
    assert!(context_tree_source.contains("StagedTraceTarget"));
}

#[test]
fn full_tree_projection_helpers_are_not_public_api() {
    let store_mod = include_str!("store.rs");
    let branch_projection = include_str!("store/branch_projection.rs");

    assert!(!branch_projection.contains("pub fn project_branch_tree_value("));
    assert!(!branch_projection.contains("pub fn project_branch_archive_tree_value("));
    assert!(
        store_mod.contains("pub(crate) use branch_projection::project_branch_archive_tree_value;")
    );
    assert!(!store_mod.contains(
        "pub use branch_projection::{ProjectionTreeScope, decode_context_tree_ledger, project_branch_archive_tree_value"
    ));
    assert!(branch_projection.contains("pub(crate) fn project_branch_tree_value("));
    assert!(branch_projection.contains("pub(crate) fn project_branch_archive_tree_value("));
    assert!(
        branch_projection.contains("project_branch_component_tree(root_store, branch, revision)")
    );
    assert!(
        branch_projection
            .contains("project_branch_archive_component_tree(root_store, branch, revision)")
    );
    for removed_parallel_engine in [
        "project_branch_tree_value_inner",
        "project_branch_archive_tree_value_inner",
        "replay_branch_transaction_traces_from",
        "replay_branch_archive_transaction_traces_from",
    ] {
        assert!(
            !branch_projection.contains(removed_parallel_engine),
            "parallel JSON projection engine remains: {removed_parallel_engine}"
        );
    }
}

#[test]
fn component_window_current_head_stops_before_full_projection() {
    let source = include_str!("store/branch_projection.rs");
    let section = source_section(
        source,
        "pub(crate) fn project_branch_component_window(",
        "fn collect_component_window(",
    );

    let current_path = section
        .find(".component(component_id)")
        .expect("current-head component-id window should read current PactView directly");
    let fallback_path = section
        .find("let tree = project_branch_component_tree(")
        .expect("historical fallback should still use branch component projection");

    assert!(current_path < fallback_path);
    assert!(section.contains(".current"));
    assert!(section.contains("collect_component_window(&branch.current.tree.root"));
}

#[test]
fn hard_fork_baselines_use_named_head_snapshot_projection() {
    let branch_projection = include_str!("store/branch_projection.rs");
    let api = include_str!("store/api.rs");
    let fork_section = source_section(api, "pub fn fork_at_revision(", "pub fn fork_at_component(");

    assert!(branch_projection.contains("pub(crate) fn project_head_snapshot("));
    assert!(fork_section.contains("project_head_snapshot(&self, &source_branch, Some(revision))"));
    assert!(!fork_section.contains("project_branch_tree_value("));
}

#[test]
fn file_head_and_revision_projection_try_native_before_cycle_replay() {
    let source = include_str!("file_format/projection_replay.rs");
    let revision = source_section(
        source,
        "pub fn file_project_revision_json(",
        "pub fn file_project_head_json(",
    );
    let head = source_section(
        source,
        "pub fn file_project_head_json(",
        "fn project_records_revision_native(",
    );
    let native = source_section(
        source,
        "pub(super) fn project_records_revision_value(",
        "pub fn file_fast_open_summary_json(",
    );
    let file_wire = source_section(
        source,
        "pub(super) fn project_records_revision_file_wire_value(",
        "fn branch_commits_to_cycle(",
    );

    let revision_native = revision
        .find("project_records_revision_value(")
        .expect("revision projection should try native-first projection helper");
    let revision_replay = revision
        .find("project_records_revision_file_wire_value(")
        .expect("revision projection should keep explicit file-wire fallback");
    assert!(revision_native < revision_replay);

    let head_native = head
        .find("project_records_revision_value(")
        .expect("head projection should try native-first projection helper");
    let head_replay = head
        .find("project_records_revision_file_wire_value(")
        .expect("head projection should keep explicit file-wire fallback");
    assert!(head_native < head_replay);

    assert!(native.contains("has_frozen_anchor_at_or_before("));
    assert!(native.contains("branch_has_native_component_projection_at("));
    assert!(native.contains("project_branch_component_tree("));
    assert!(native.contains("project_records_revision_file_wire_value(records, target_cycle)"));
    assert!(native.contains("fn project_records_revision_native("));
    assert!(native.contains("fn project_records_revision_file_wire_value("));
    assert!(file_wire.contains("fn project_file_wire_revision_from_records("));
    assert!(file_wire.contains("fn project_file_wire_branch_cycle_value("));
    assert!(file_wire.contains("fn project_branch_file_wire_base_value("));
    assert!(file_wire.contains("branch_has_native_component_projection_at(branch, Some(0))"));
    assert!(!source.contains("fn project_cycle_from_records("));
    assert!(!source.contains("project_cycle_from_records_for_branch("));
}

#[test]
fn frozen_anchor_build_uses_shared_revision_projection_family() {
    let source = include_str!("file_format/history.rs");
    let section = source_section(
        source,
        "pub(super) fn build_frozen_anchor_records(",
        "fn should_freeze_cycle_interval(",
    );

    assert!(section.contains("project_records_revision_file_wire_value("));
    assert!(!section.contains("project_cycle_from_records("));
}

#[test]
fn tree_projection_uses_the_v2_ledger_facade_without_transcode() {
    let source = include_str!("file_format.rs");
    let section = source_section(
        source,
        "pub(crate) fn file_project_tree_json_with_scope(",
        "pub fn file_hydrate_components_json(",
    );

    assert!(section.contains("let root_store = file_open_ledger(path)?;"));
    assert!(!section.contains("PackedV1"));
    assert!(!section.contains("packed_v1_to_v0_bytes"));
}

#[test]
fn removed_packed_repack_is_a_fail_loud_api_stub() {
    let source = include_str!("file_format/history.rs");
    let section = source_section(
        source,
        "pub fn file_repack_packed_v1_json(",
        "pub(super) fn packed_v1_to_v0_bytes(",
    );

    assert!(section.contains("PactError::unsupported("));
    assert!(!section.contains("publish_bytes("));
    assert!(!section.contains("write_packed_v1_cold_repack"));
}

#[test]
fn file_load_hydrates_commit_records_as_transaction_traces() {
    let file_source = include_str!("file_format.rs");
    let history_source = include_str!("file_format/history.rs");
    let projection_source = include_str!("file_format/projection_replay.rs");
    let pact_pack_source = include_str!("pact_pack/mod.rs");

    assert!(file_source.contains("transaction_traces_from_commit_records_from"));
    assert!(file_source.contains("ensure_trace_content_refs_are_recorded("));
    assert!(
        file_source.contains("cannot append trace before referenced content records are present")
    );
    assert!(history_source.contains("transaction_traces_from_commit_records_from("));
    assert!(history_source.contains("branch.transaction_traces.push(trace);"));
    assert!(!history_source.contains("crate::pact_pack::ledgers_from_revision_trees"));
    assert!(!history_source.contains("write_packed_v1_cold_repack"));
    assert!(
        projection_source.contains("pub(super) fn transaction_traces_from_commit_records_from(")
    );
    assert!(projection_source.contains("append_commit_record_trace_ops("));
    assert!(projection_source.contains("TraceOpKind::InsertComponent"));
    assert!(projection_source.contains("TraceOpKind::SetContent"));
    assert!(!projection_source.contains("TransactionTrace::diff("));
    assert!(projection_source.contains("commit.cycle > start_cycle"));
    assert!(pact_pack_source.contains("Vec<TransactionTrace>"));
    assert!(pact_pack_source.contains("PactError::unsupported(REMOVED_MESSAGE)"));
    assert!(!pact_pack_source.contains("zstd::"));
    assert!(!pact_pack_source.contains("read_varint"));
}

#[test]
fn cli_prune_uses_native_ledger_operation() {
    let source = include_str!("main.rs");
    assert!(source.contains("fn cli_prune_ledger_json("));
    assert!(
        source.contains(
            "pactree::ledger_delete_json(root_store_json, branch_id, None, Some(selector))"
        )
    );
    assert!(!source.contains("store::decode_context_tree_ledger(root_store_json)?"));
    assert!(!source.contains("root_store.prune(branch_id, None, Some(selector.to_string()))"));
    assert!(!source.contains("store::ledger_delete_json("));
}

#[test]
fn file_format_value_usage_is_documented_as_wire_boundary() {
    let file_source = include_str!("file_format.rs");
    assert!(BOUNDARY_DOCS.contains("allowed to use `serde_json::Value` for file wire records"));
    assert!(BOUNDARY_DOCS.contains("semantic execution substrate"));
    assert!(BOUNDARY_DOCS.contains("File APIs that change Pact"));
    assert!(
        BOUNDARY_DOCS.contains("File commit-record projection hydrates `TransactionTrace` history")
    );
    assert!(file_source.contains("pub fn file_fork_json("));
    assert!(file_source.contains("match mode"));
    assert!(file_source.contains("PactForkMode::Soft =>"));
    assert!(file_source.contains("hard fork requires an output path for the child .pact"));
    assert!(file_source.contains("PactParentProvenance"));
    assert!(file_source.contains("crate::store::project_branch_component_tree("));
    assert!(file_source.contains("fork_tree.edit(|root|"));
    assert!(file_source.contains("hydrate_payload_refs(\n                &mut child_tree_value"));
    assert!(!file_source.contains("ledger_fork_at_revision_json("));
    assert!(!file_source.contains("source_patch_count_at_fork"));
    assert!(!file_source.contains("replayed_patch_count"));

    let projection_source = include_str!("file_format/projection_replay.rs");
    assert!(BOUNDARY_DOCS.contains("`CommitRecordOp` replay is a file-wire adapter"));
    assert!(BOUNDARY_DOCS.contains("does not call any patch-shaped import"));
    assert!(!projection_source.contains("patches_from_commit_records("));
}

#[test]
fn hard_fork_history_selection_resolves_parent_above_selector_stream() {
    let wire_source = include_str!("file_format/wire.rs");
    let corpus_source = include_str!("file_format/corpus.rs");
    let parent_aware_source = include_str!("file_format/parent_aware.rs");
    let main_source = include_str!("main.rs");

    assert!(!wire_source.contains("pub parent_path: Option<String>"));
    assert!(!corpus_source.contains("extend_result_with_parent_history("));
    assert!(corpus_source.contains("selector_reaches_prefork_history("));
    assert!(parent_aware_source.contains("pub fn file_select_with_parents("));
    assert!(
        parent_aware_source
            .contains("resolve_parent(Some(child_path), provenance, parent_config)?")
    );
    assert!(parent_aware_source.contains("CROSS_FORK_HISTORY_BLOCKER"));
    assert!(main_source.contains("parent_resolver_config_from_paths(parent_paths)"));
    assert!(main_source.contains("file_format::file_select_with_parents("));
}

#[test]
fn json_and_value_boundary_pockets_are_documented() {
    for (path, source, marker) in [
        (
            "components.rs",
            include_str!("components.rs"),
            "`Component` / `ComponentKind` model is the semantic interface",
        ),
        (
            "selector.rs",
            include_str!("selector.rs"),
            "native tree before selector semantics run",
        ),
        (
            "mutation.rs",
            include_str!("mutation.rs"),
            "Canonical mutation applies to `ComponentTree`",
        ),
        (
            "normalize.rs",
            include_str!("normalize.rs"),
            "serialized inspection/rendering view",
        ),
        (
            "store/api.rs",
            include_str!("store/api.rs"),
            "functions in this module are CLI/FFI/file-wire adapters",
        ),
        (
            "resolver/api.rs",
            include_str!("resolver/api.rs"),
            "`*_json` methods and `serde_json::Value` reports are FFI/CLI compatibility",
        ),
    ] {
        assert!(!source.is_empty(), "{path} source is missing");
        assert!(
            BOUNDARY_DOCS.contains(path),
            "{path} missing boundary section"
        );
        assert!(
            BOUNDARY_DOCS.contains(marker),
            "{path} missing boundary marker"
        );
    }
}

#[test]
fn public_help_uses_projection_vocabulary_for_read_surfaces() {
    let source = include_str!("main.rs");
    let help_source = source_section(source, "fn print_help() {", "fn default_blob_inline_limit");

    for required in [
        "pact view head",
        "pact view archived",
        "pact load components",
        "pact store-project",
    ] {
        assert!(
            help_source.contains(required),
            "public help must advertise projection/read surface `{required}`"
        );
    }

    for forbidden in [
        "file-materialize",
        "root-materialize",
        "store-materialize",
        " materialize ",
        " materialization",
    ] {
        assert!(
            !help_source.contains(forbidden),
            "public help must not advertise materialization vocabulary `{forbidden}`"
        );
    }
}

#[test]
fn production_sources_do_not_reintroduce_legacy_materialize_symbols() {
    let forbidden = concat!("legacy_", "materialize");
    for (path, source) in [
        ("src/context_tree.rs", include_str!("context_tree.rs")),
        ("src/file_format.rs", include_str!("file_format.rs")),
        (
            "src/file_format/corpus.rs",
            include_str!("file_format/corpus.rs"),
        ),
        (
            "src/file_format/text_search.rs",
            include_str!("file_format/text_search.rs"),
        ),
        ("src/provider_thread.rs", include_str!("provider_thread.rs")),
        ("src/render.rs", include_str!("render.rs")),
        ("src/resolver/api.rs", include_str!("resolver/api.rs")),
        ("src/selector.rs", include_str!("selector.rs")),
        ("src/store/api.rs", include_str!("store/api.rs")),
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must not introduce `{forbidden}` compatibility symbols"
        );
    }
}

#[test]
fn normal_production_sources_do_not_use_materialization_vocabulary() {
    let forbidden = concat!("material", "iz");
    for (path, source) in [
        ("src/context_tree.rs", include_str!("context_tree.rs")),
        ("src/file_format.rs", include_str!("file_format.rs")),
        (
            "src/file_format/corpus.rs",
            include_str!("file_format/corpus.rs"),
        ),
        (
            "src/file_format/history.rs",
            include_str!("file_format/history.rs"),
        ),
        (
            "src/file_format/inspect.rs",
            include_str!("file_format/inspect.rs"),
        ),
        (
            "src/file_format/projection_replay.rs",
            include_str!("file_format/projection_replay.rs"),
        ),
        (
            "src/file_format/query.rs",
            include_str!("file_format/query.rs"),
        ),
        (
            "src/file_format/text_search.rs",
            include_str!("file_format/text_search.rs"),
        ),
        ("src/main.rs", include_str!("main.rs")),
        ("src/provider_thread.rs", include_str!("provider_thread.rs")),
        ("src/render.rs", include_str!("render.rs")),
        ("src/resolver/api.rs", include_str!("resolver/api.rs")),
        ("src/selector.rs", include_str!("selector.rs")),
        ("src/store.rs", include_str!("store.rs")),
        (
            "src/store/branch_projection.rs",
            include_str!("store/branch_projection.rs"),
        ),
        ("src/store/api.rs", include_str!("store/api.rs")),
    ] {
        assert!(
            !source.contains(forbidden),
            "{path} must use projection/read vocabulary, not `{forbidden}` runtime vocabulary"
        );
    }
}

#[test]
fn single_store_projection_does_not_reintroduce_materialize_helper() {
    let source = include_str!("store/branch.rs");
    assert!(
        !source.contains("materialize_value"),
        "single-store projection must use projection vocabulary"
    );
    assert!(source.contains("fn project_value("));
}

#[test]
fn materialization_modules_stay_deleted_and_unexported() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "src/materialization.rs",
        "src/store/materialize.rs",
        "src/file_format/materialize.rs",
    ] {
        assert!(
            !repo_root.join(path).exists(),
            "{path} must stay deleted; projection is the read surface"
        );
    }

    let lib_source = include_str!("lib.rs");
    assert!(!lib_source.contains("mod materialization"));
    assert!(!lib_source.contains("pub mod materialization"));
}

#[test]
fn resolver_and_mutation_modules_are_not_public_user_modules() {
    let lib_source = include_str!("lib.rs");
    assert!(!lib_source.contains("pub mod resolver"));
    assert!(!lib_source.contains("pub mod mutation"));
    assert!(!lib_source.contains("pub mod store"));
    assert!(!lib_source.contains("pub mod tree"));
    assert!(!lib_source.contains("pub use tree::ComponentTree"));
    assert!(lib_source.contains("mod resolver"));
    assert!(lib_source.contains("mod mutation"));
    assert!(lib_source.contains("mod store"));
    assert!(lib_source.contains("mod tree"));
    assert!(lib_source.contains("pub use mutation::{"));
    assert!(lib_source.contains("MutationValidationError"));
    assert!(lib_source.contains("validate_mutation_request_json"));
}

#[test]
fn file_ledger_open_is_not_public_user_api() {
    let file_source = include_str!("file_format.rs");
    let history_source = include_str!("file_format/history.rs");
    let builder_source = include_str!("file_format/builder.rs");
    let parent_source = include_str!("file_format/parent.rs");
    assert!(file_source.contains("pub(crate) fn file_open_ledger("));
    assert!(!file_source.contains("pub fn file_open_ledger("));
    assert!(file_source.contains("pub fn file_root_id("));
    assert!(file_source.contains("pub fn file_load_child_head_json("));
    assert!(!file_source.contains("file_repack_with_options_json, ledger_from_records"));
    assert!(
        !file_source
            .contains("pub use parent::{ParentResolverConfig, VerifiedParent, resolve_parent}")
    );
    assert!(history_source.contains("pub(crate) fn ledger_from_records("));
    assert!(!history_source.contains("pub fn ledger_from_records("));
    assert!(builder_source.contains("pub(crate) fn from_ledger("));
    assert!(!builder_source.contains("pub fn from_ledger("));
    assert!(parent_source.contains("pub(crate) fn resolve_parent("));
    assert!(!parent_source.contains("pub fn resolve_parent("));
}

#[test]
fn file_size_and_rss_guard_tests_stay_present_with_expected_cadence() {
    let file_tests = include_str!("file_format/tests.rs");
    for test_name in [
        "bench_medium_agentic_stays_under_100kb",
        "pact_jsonl_gzip_comparison_gates",
        "repeated_output_pact_beats_jsonl_gzip",
        "repeated_large_output_growth_is_ref_bounded",
        "heavy_repeated_output_uses_refs_not_inline_bodies",
        "medium_head_projection_rss_delta_stays_bounded",
        "medium_provider_thread_projection_rss_delta_stays_bounded",
        "medium_selector_window_rss_delta_stays_bounded",
        "medium_component_id_hydration_rss_delta_stays_bounded",
    ] {
        assert!(
            file_tests.contains(&format!("#[test]\nfn {test_name}(")),
            "file-format guard test `{test_name}` must stay in normal test runs"
        );
    }

    assert!(file_tests.contains(
        "#[test]\n#[cfg(feature = \"compression-zstd\")]\nfn archive_profile_beats_full_state_jsonl_gzip("
    ));

    assert!(file_tests.contains(
        "#[test]\n#[ignore = \"manual RSS reporter for large projection/query/provider-thread reads\"]\nfn large_projection_rss_reporter("
    ));
    for field in [
        "projection_bytes={projection_bytes}",
        "provider_thread_bytes={provider_thread_bytes}",
        "query_result_bytes={query_result_bytes}",
        "hydration_result_bytes={hydration_result_bytes}",
        "projection_peak_rss={projection_peak_rss}",
        "projection_delta_rss={projection_delta_rss}",
        "provider_peak_rss={provider_peak_rss}",
        "provider_delta_rss={provider_delta_rss}",
        "query_peak_rss={query_peak_rss}",
        "query_delta_rss={query_delta_rss}",
        "hydration_peak_rss={hydration_peak_rss}",
        "hydration_delta_rss={hydration_delta_rss}",
    ] {
        assert!(
            file_tests.contains(field),
            "large RSS reporter must print `{field}`"
        );
    }

    let cutover_tests = include_str!("../tests/storage_cutover.rs");
    assert!(cutover_tests.contains("public_context_tree_persists_only_pactbin2"));
}

#[test]
fn batch3_component_hydration_uses_native_component_projection() {
    let source = include_str!("file_format.rs");
    let section = source_section(
        source,
        "pub fn file_hydrate_components_json(",
        "pub fn file_open_json(",
    );

    assert!(section.contains("project_branch_archive_component_tree("));
    assert!(section.contains("prune_component_tree("));
    assert!(section.contains("supplement_hydrated_component_records("));
    assert!(section.contains("hydrate_payload_refs(&mut tree, &root_store.content)"));
    assert!(!section.contains("ledger_project_branch_tree_json("));
    assert!(!section.contains("ComponentTree::from_value("));
    assert!(!section.contains("prune_hydrated_component_tree("));
    assert!(!section.contains("serde_json::to_string(&root_store)"));
}

#[test]
fn batch3_corpus_snapshot_windows_use_native_component_selection() {
    let source = include_str!("file_format/corpus.rs");
    let section = source_section(
        source,
        "fn iterate_snapshot_window<",
        "struct NodeStreamItem",
    );

    assert!(section.contains("project_branch_archive_component_tree("));
    assert!(section.contains("select_components_tree("));
    assert!(!section.contains("project_cycle_from_records_for_branch("));
    assert!(!section.contains("ComponentTree::from_value("));
    assert!(!section.contains("SerializedSelectorComponent::from_value("));
    assert!(!section.contains("selector_select("));
}

#[test]
fn archive_pact_view_projection_uses_typed_archive_components() {
    let source = include_str!("store/api.rs");
    let section = source_section(
        source,
        "pub fn project_branch_with_scope(",
        "pub fn project_branch(",
    );

    assert!(section.contains("ProjectionTreeScope::Head => {"));
    assert!(section.contains("project_head_snapshot("));
    assert!(section.contains("ProjectionTreeScope::Archive => {"));
    assert!(section.contains("project_branch_archive_component_tree("));
    assert!(section.contains("build_projection_from_component(tree.root()"));
    assert!(!section.contains("build_projection(&projected"));
}

#[test]
fn branch_tree_json_head_scope_uses_head_snapshot_projection() {
    let source = include_str!("store/api.rs");
    let section = source_section(
        source,
        "pub fn ledger_project_branch_tree_json_with_scope(",
        "pub fn ledger_project_branch_json(",
    );

    assert!(section.contains("ProjectionTreeScope::Head => {"));
    assert!(section.contains("project_head_snapshot("));
    assert!(!section.contains("project_branch_component_tree("));
    assert!(!section.contains("project_branch_tree_value("));
}

#[test]
fn typed_archive_component_projection_does_not_parse_archive_json_sink() {
    let source = include_str!("store/branch_projection.rs");
    let section = source_section(
        source,
        "pub(crate) fn project_branch_archive_component_tree(",
        "fn archive_base_from_transaction_traces(",
    );

    assert!(section.contains("project_branch_archive_component_tree_inner("));
    assert!(section.contains("replay_branch_archive_component_transaction_traces_from("));
    assert!(!section.contains("project_branch_archive_tree_value("));
    assert!(!section.contains("ComponentTree::from_value(&tree)"));
}

#[test]
fn phase4_query_head_selector_buckets_use_current_head_indexes() {
    let source = include_str!("file_format/query.rs");
    let section = source_section(
        source,
        "fn query_native_head_selector_bucket(",
        "fn native_head_id_entries(",
    );

    assert!(section.contains("branch.current.component_count()"));
    assert!(section.contains("branch.current.tree.index"));
    assert!(!section.contains("project_branch_component_tree("));
    assert!(!section.contains("project_branch_tree_value("));
}

#[test]
fn component_json_adapters_have_component_named_entry_points() {
    let source = include_str!("components.rs");
    for function in [
        "pub fn component_from_json(",
        "pub fn component_project_json(",
        "pub fn component_prepare_for_context_json(",
        "pub fn pact_canonical_component_kinds_json(",
        "pub fn pact_canonical_component_json(",
    ] {
        assert!(source.contains(function), "missing {function}");
    }

    for removed_alias in [
        "pub fn pact_node_from_component_json",
        "pub fn pact_node_project_json",
        "pub fn pact_node_prepare_for_context_json",
        "pub fn pact_canonical_node_json",
        "pub fn pact_canonical_node_kinds_json",
    ] {
        assert!(
            !source.contains(removed_alias),
            "deleted node-named adapter `{removed_alias}` must stay removed"
        );
    }

    assert!(source.contains("fn canonical_component_json(kind: &str, payload_json: &str)"));
    assert!(!source.contains(concat!("parse_", "leg", "acy_node_adapter(")));
    assert!(!source.contains("PACTNode"));
    assert!(source.contains("pub const PACT_COMPONENT_ESCAPE_HATCH: &str = \"PACTComponent\";"));
}

fn scanned_source_files() -> Vec<(String, String)> {
    fn is_excluded_corpus(rel: &str) -> bool {
        rel.ends_with("english_lexpack.rs") || rel.ends_with("shell_lexpack.rs")
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--", "src", "tests"])
        .output()
        .expect("architecture guards require `git` to enumerate tracked sources");
    assert!(
        output.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut out = Vec::new();
    for rel in output.stdout.split(|byte| *byte == 0) {
        if rel.is_empty() {
            continue;
        }
        let Ok(rel) = std::str::from_utf8(rel) else {
            continue;
        };
        if !rel.ends_with(".rs") || is_excluded_corpus(rel) {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(root.join(rel)) {
            out.push((rel.to_string(), contents));
        }
    }
    assert!(
        !out.is_empty(),
        "architecture guards found no tracked source files to scan"
    );
    out
}

fn contains_bare_word_ci(haystack: &str, word: &str) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    let word = word.to_ascii_lowercase();
    let bytes = haystack.as_bytes();
    let is_word_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(&word) {
        let start = from + rel;
        let end = start + word.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

#[test]
fn sources_do_not_reintroduce_removed_patch_import_or_old_ledger_vocabulary() {
    let forbidden: &[&str] = &[
        concat!("Leg", "acyPatchImport"),
        concat!("leg", "acy_patch_import"),
        concat!("store_import", "_patch"),
        concat!("ledger_import", "_patch"),
        concat!("store-import-", "leg", "acy-patch"),
        concat!("replayed_", "leg", "acy_import"),
        concat!("source_", "leg", "acy_import"),
        concat!("ledger", "_diff"),
        concat!("encode", "_ledger"),
        concat!("decode", "_ledger"),
        concat!("pack", "_ledgers"),
        concat!("unpack", "_ledgers"),
    ];
    for (path, source) in scanned_source_files() {
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{path} reintroduces removed vocabulary `{needle}`"
            );
        }
    }
}

#[test]
fn sources_do_not_reintroduce_bare_legacy_vocabulary() {
    let bare = concat!("leg", "acy");
    for (path, source) in scanned_source_files() {
        assert!(
            !contains_bare_word_ci(&source, bare),
            "{path} reintroduces bare `{bare}` architecture vocabulary; \
             rename to removed/unsupported or delete it"
        );
    }
}
