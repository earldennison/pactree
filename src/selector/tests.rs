use super::*;

#[path = "id_tests.rs"]
mod id;

fn sample_tree_json() -> &'static str {
    r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {
              "id": "sys",
              "componentType": "system_header",
              "pact_depth": -1,
              "children": [
                {"id":"shell","componentType":"shell","type":"shell","offset":0}
              ]
            },
            {
              "id": "history",
              "componentType": "conversation_history",
              "children": [
                {
                  "id": "assistant_turn",
                  "componentType": "message_turn",
                  "type": "message_turn",
                  "role": "assistant",
                  "author": "Earl Dennison",
                  "priority": 3,
                  "pact_depth": 1,
                  "offset": 0,
                  "children": [
                    {"id":"assistant_text","componentType":"text_content","type":"text_content","offset":0,"ttl":2,"tags":["tool_call","urgent"],"content":"hello"},
                    {"id":"sticky_note","componentType":"note","type":"note","offset":0,"ttl":1,"cad":1,"content":"remember"},
                    {"id":"assistant_tool","componentType":"tool_result","type":"tool_result","offset":1,"content":"done"}
                  ]
                }
              ]
            },
            {
              "id": "active",
              "componentType": "active_message",
              "type": "message_turn",
              "role": "user",
              "pact_depth": 0,
              "offset": 1,
              "children": [
                {"id":"active_text","componentType":"text_content","type":"text_content","offset":0,"content":"question"}
              ]
            }
          ]
        }"#
}

fn expansion_tree_json() -> &'static str {
    r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {
              "id": "active",
              "componentType": "active_message",
              "type": "message_turn",
              "role": "user",
              "pact_depth": 0,
              "offset": 0,
              "children": [
                {
                  "id": "container",
                  "componentType": "message_container",
                  "type": "message_container",
                  "offset": 0,
                  "children": [
                    {"id":"am_a","componentType":"text_content","type":"text_content","offset":0,"ttl":1,"cad":1,"tags":["urgent","alpha"],"author":"Earl Grey","pinned":true,"score":3.25,"content":"am-a"},
                    {"id":"am_b","componentType":"text_content","type":"text_content","offset":1,"ttl":1,"cad":2,"tags":["beta"],"author":"Tea Time","pinned":false,"score":2.75,"content":"am-b"},
                    {"id":"am_c","componentType":"text_content","type":"text_content","offset":2,"ttl":2,"cad":1,"tags":["urgent","alpha"],"author":"Earl Grey II","pinned":true,"score":4.0,"content":"am-c"}
                  ]
                }
              ]
            }
          ]
        }"#
}

fn selected_ids(tree: &str, selector: &str) -> Vec<String> {
    let selected = select_json(tree, selector).expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    matches.into_iter().map(|item| item.id).collect()
}

#[test]
fn simple_type_selector_uses_component_tree_buckets_without_full_reindex() {
    let tree = crate::tree::ComponentTree::from_value(
        &serde_json::from_str(sample_tree_json()).expect("sample tree JSON"),
    )
    .expect("component tree");
    crate::tests::instrumentation::reset_selector_full_index_count();

    let selected =
        super::select_components_tree(&tree, ".text_content", false).expect("type selection");

    assert_eq!(selected.len(), 2);
    assert_eq!(
        crate::tests::instrumentation::selector_full_index_count(),
        0,
        "simple indexed selectors must not recursively reindex the whole tree"
    );
}

#[test]
fn selects_depth_regions() {
    let selected = select_json(sample_tree_json(), "d0").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "active");
}

#[test]
fn selects_recursive_type_and_attribute() {
    let selected = select_json(sample_tree_json(), "d1{role='assistant'}").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_turn");
}

#[test]
fn selects_nested_attrs_by_attribute() {
    let tree = r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {
              "id": "call",
              "componentType": "tool_call",
              "type": "tool_call",
              "attrs": {
                "tool_call_id": "toolu_nested",
                "tool_name": "shell_execute"
              }
            }
          ]
        }"#;

    let selected = select_json(tree, "{tool_call_id=toolu_nested}").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "call");
}

#[test]
fn selects_by_position() {
    let selected = select_json(sample_tree_json(), "(d0,0)").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "active_text");
}

#[test]
fn selects_depth_range_positions() {
    let selected = select_json(sample_tree_json(), "(d0..d*,0)").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    let ids: Vec<String> = matches.into_iter().map(|item| item.id).collect();
    assert_eq!(ids, vec!["assistant_text", "active_text"]);

    let selected = select_json(sample_tree_json(), "(d1..d1,1)").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_tool");
}

#[test]
fn selects_coordinate_sets() {
    let selected = select_json(sample_tree_json(), "((d0,0), (d1,1))").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    let ids: Vec<String> = matches.into_iter().map(|item| item.id).collect();
    assert_eq!(ids, vec!["active_text", "assistant_tool"]);

    let selected = select_json(sample_tree_json(), "((d1..d1,0), (d0,0))").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    let ids: Vec<String> = matches.into_iter().map(|item| item.id).collect();
    assert_eq!(ids, vec!["assistant_text", "active_text"]);
}

#[test]
fn selects_by_id_union() {
    let selected = select_json(sample_tree_json(), "#active_text, #shell").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    let ids: Vec<String> = matches.into_iter().map(|item| item.id).collect();
    assert_eq!(ids, vec!["active_text", "shell"]);
}

#[test]
fn selects_children_and_descendants_from_depth() {
    let selected = select_json(sample_tree_json(), "d0 > .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches[0].id, "active_text");
}

#[test]
fn selects_descendants_with_whitespace_combinator() {
    let selected = select_json(sample_tree_json(), "#history .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");
}

#[test]
fn selects_descendants_with_tilde_combinator() {
    let selected = select_json(sample_tree_json(), "#history ~ .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");
}

#[test]
fn selects_descendants_with_within_limit() {
    let selected =
        select_json(sample_tree_json(), "#history ??within(1) .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert!(matches.is_empty());

    let selected =
        select_json(sample_tree_json(), "#history ??within(2) .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");
}

#[test]
fn rejects_deprecated_spatial_anchors() {
    let err = select_json(sample_tree_json(), "^seq .text_content").expect_err("deprecated");
    assert!(
        err.to_string()
            .contains("deprecated symbolic PACT region selectors")
    );

    let err = select_json(sample_tree_json(), "^ah .text_content").expect_err("deprecated");
    assert!(
        err.to_string()
            .contains("deprecated symbolic PACT region selectors")
    );
}

#[test]
fn selects_canonical_depth_regions_instead_of_spatial_anchors() {
    let selected = select_json(sample_tree_json(), "d1 .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");

    let selected = select_json(sample_tree_json(), "d0 .text_content").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "active_text");
}

#[test]
fn selects_bracket_attributes_with_numeric_comparisons() {
    let selected = select_json(sample_tree_json(), ".message_turn[priority>=3]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_turn");

    let selected = select_json(sample_tree_json(), ".text_content[ttl<3]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");
}

#[test]
fn selects_lifecycle_behavior_filters() {
    let selected = select_json(sample_tree_json(), ".text_content[ttl<3]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");

    let selected = select_json(sample_tree_json(), ".note[ephemeral]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "sticky_note");

    let selected = select_json(sample_tree_json(), ".note[sticky cadence=1]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "sticky_note");

    let selected = select_json(sample_tree_json(), ".note[sticky=false]").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert!(matches.is_empty());
}

#[test]
fn selects_attributes_with_not_equal() {
    let selected = select_json(sample_tree_json(), ".message_turn{role!='user'}").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_turn");
}

#[test]
fn selects_attributes_with_regex_operator() {
    let selected =
        select_json(sample_tree_json(), ".message_turn{author~='^Earl'}").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_turn");

    let selected = select_json(sample_tree_json(), ".message_turn{author~='['}").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert!(matches.is_empty());
}

#[test]
fn selects_structural_pseudos() {
    let selected = select_json(sample_tree_json(), "#assistant_text:first-child").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");

    let selected = select_json(sample_tree_json(), "#assistant_tool:last-child").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_tool");

    let selected = select_json(sample_tree_json(), "#sticky_note:nth(2)").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "sticky_note");

    let selected =
        select_json(sample_tree_json(), ".message_turn:has(.tool_result)").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_turn");
}

#[test]
fn selects_tag_filters() {
    let selected = select_json(sample_tree_json(), "+tool_call+urgent").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_text");
}

#[test]
fn selects_namespaced_tag_prefixes_and_wildcards_by_segment() {
    let tree = r#"{
          "id": "root",
          "componentType": "root",
          "children": [
            {"id":"shell_body","componentType":"text_content","type":"text_content","tags":["shell:body"]},
            {"id":"shell_execute","componentType":"tool_call","type":"tool_call","tags":["shell:op_execute"]},
            {"id":"shell_stdout","componentType":"tool_result","type":"tool_result","tags":["shell:op_execute:stdout"]},
            {"id":"shell_op","componentType":"tool_call","type":"tool_call","tags":["shell:op"]},
            {"id":"shell_op_stdout","componentType":"tool_result","type":"tool_result","tags":["shell:op:stdout"]},
            {"id":"shell_op_execute_prefix","componentType":"tool_call","type":"tool_call","tags":["shell:op_execute_prefix"]},
            {"id":"file_body","componentType":"text_content","type":"text_content","tags":["file:body"]}
          ]
        }"#;

    assert_eq!(
        selected_ids(tree, "+shell"),
        vec![
            "shell_body",
            "shell_execute",
            "shell_op",
            "shell_op_execute_prefix",
            "shell_op_stdout",
            "shell_stdout",
        ]
    );
    assert_eq!(
        selected_ids(tree, "+shell:*"),
        vec![
            "shell_body",
            "shell_execute",
            "shell_op",
            "shell_op_execute_prefix",
        ]
    );
    assert_eq!(
        selected_ids(tree, "+*:body"),
        vec!["file_body", "shell_body"]
    );
    assert_eq!(
        selected_ids(tree, "+shell:op"),
        vec!["shell_op", "shell_op_stdout"]
    );
    assert_eq!(
        selected_ids(tree, "+shell:op_execute"),
        vec!["shell_execute", "shell_stdout"]
    );
    assert_eq!(
        selected_ids(tree, "+shell:op_execute:*"),
        vec!["shell_stdout"]
    );
}

#[test]
fn selects_canonical_type_tags() {
    let selected = select_json(sample_tree_json(), ".block:tool_result").expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_tool");
}

#[test]
fn applies_selector_controls() {
    let (cleaned, controls) =
        extract_controls(".block @offset=1 @limit=1 @mode=latest").expect("controls");
    assert_eq!(cleaned, ".block");
    assert_eq!(controls.offset, 1);
    assert_eq!(controls.limit, Some(1));
    assert_eq!(controls.mode.as_deref(), Some("latest"));

    let selected = select_json(
        sample_tree_json(),
        "#history .block @offset=1 @limit=1 @mode=latest",
    )
    .expect("select");
    let matches: Vec<SelectorMatch> = serde_json::from_str(&selected).expect("json");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "assistant_tool");
}

#[test]
fn expands_coordinate_all_aliases_and_ranges() {
    let tree = expansion_tree_json();

    assert_eq!(
        selected_ids(tree, "(d0,0, all)"),
        vec!["am_a", "am_b", "am_c"]
    );
    assert_eq!(selected_ids(tree, "(d0,0, min)"), vec!["am_a"]);
    assert_eq!(selected_ids(tree, "(d0,0, first)"), vec!["am_a"]);
    assert_eq!(selected_ids(tree, "(d0,0, -last)"), vec!["am_a"]);
    assert_eq!(selected_ids(tree, "(d0,0, max)"), vec!["am_c"]);
    assert_eq!(selected_ids(tree, "(d0,0, last)"), vec!["am_c"]);
    assert_eq!(selected_ids(tree, "(d0,0, +last)"), vec!["am_c"]);
    assert_eq!(
        selected_ids(tree, "(d0,0, 0..max)"),
        vec!["am_a", "am_b", "am_c"]
    );
    assert_eq!(selected_ids(tree, "(d0,0, 1..0)"), vec!["am_a", "am_b"]);
    assert_eq!(
        selected_ids(tree, "(d0,0, max..0)"),
        vec!["am_a", "am_b", "am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, +all)"),
        vec!["am_a", "am_b", "am_c"]
    );
}

#[test]
fn keeps_coordinate_parser_strict_for_depth_and_invalid_ranges() {
    let tree = expansion_tree_json();

    assert!(select_json(tree, "(all,0)").is_err());
    assert!(select_json(tree, "(d0,0, all..+last)").is_err());
    assert!(select_json(tree, "()").is_err());
    assert!(select_json(tree, "(d0, 'invalid')").is_err());
}

#[test]
fn handles_selector_parser_robustness_cases() {
    let tree = expansion_tree_json();

    assert_eq!(
        selected_ids(tree, "(d0,\n  0,   all)"),
        vec!["am_a", "am_b", "am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all){ author~='^Earl\\s+Grey$' }"),
        vec!["am_a"]
    );
    assert!(selected_ids(tree, "(d0,0, all){ author~='[unterminated' }").is_empty());
    assert!(select_json(tree, "(d0,0, all) @mode=unknown").is_err());
    assert!(select_json(tree, "(d0,0, all) @limit=").is_err());
}

#[test]
fn applies_boolean_float_lifecycle_and_tag_filters() {
    let tree = expansion_tree_json();

    assert_eq!(
        selected_ids(tree, "(d0,0, all){ pinned=true }"),
        vec!["am_a", "am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all){ pinned=false }"),
        vec!["am_b"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all){ score>=3.0 }"),
        vec!["am_a", "am_c"]
    );
    assert_eq!(selected_ids(tree, "(d0,0, all){ score<3.0 }"), vec!["am_b"]);
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[ttl=1]"),
        vec!["am_a", "am_b"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[cad=1]"),
        vec!["am_a", "am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[cadence=1]"),
        vec!["am_a", "am_c"]
    );
    assert_eq!(selected_ids(tree, "(d0,0, all)[sticky=true]"), vec!["am_a"]);
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[sticky=false]"),
        vec!["am_b", "am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[ephemeral=true]"),
        vec!["am_a", "am_b"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all)[ephemeral=false]"),
        vec!["am_c"]
    );
    assert_eq!(
        selected_ids(tree, "(d0,0, all)+urgent+alpha"),
        vec!["am_a", "am_c"]
    );
    assert_eq!(selected_ids(tree, "(d0,0, all)+beta"), vec!["am_b"]);
}

#[test]
fn select_components_populates_rich_canonical_fields() {
    let tree: Value = serde_json::from_str(
        r#"{
              "id": "root",
              "componentType": "root",
              "children": [
                {
                  "id": "turn",
                  "componentType": "message_turn",
                  "type": "message_turn",
                  "role": "assistant",
                  "offset": 0,
                  "pact_depth": 1,
                  "children": [
                    {
                      "id": "call",
                      "componentType": "tool_call",
                      "type": "tool_call",
                      "parent_id": "turn",
                      "offset": 0,
                      "tool_call_id": "tc_1",
                      "tool_name": "shell",
                      "parameters": {"cmd": "ls"},
                      "attrs": {"vendor": "acme"}
                    },
                    {
                      "id": "text",
                      "componentType": "text_content",
                      "type": "text_content",
                      "parent_id": "turn",
                      "offset": 1,
                      "content_ref": {"id": "blob_text"},
                      "source_ref": {"kind": "file", "path": "/x"}
                    },
                    {
                      "id": "res",
                      "componentType": "tool_result",
                      "type": "tool_result",
                      "parent_id": "turn",
                      "offset": 2,
                      "tool_call_id": "tc_1",
                      "result": {"stdout": "ok"},
                      "visibility": "hidden"
                    }
                  ]
                }
              ]
            }"#,
    )
    .unwrap();

    let calls = select_components(&tree, ".tool_call", false).expect("select tool_call");
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.id, "call");
    assert_eq!(call.parent_id.as_deref(), Some("turn"));
    assert_eq!(call.tool_call_id.as_deref(), Some("tc_1"));
    assert_eq!(call.tool_name.as_deref(), Some("shell"));
    assert_eq!(call.parameters, Some(serde_json::json!({"cmd": "ls"})));
    assert_eq!(call.aux.get("vendor").and_then(Value::as_str), Some("acme"));
    assert!(!call.archived && !call.deleted);

    let text = select_components(&tree, ".text_content", false).expect("select text");
    assert_eq!(text.len(), 1);
    assert_eq!(
        text[0].content_ref,
        Some(serde_json::json!({"id": "blob_text"}))
    );
    assert_eq!(
        text[0].source_ref,
        Some(serde_json::json!({"kind": "file", "path": "/x"}))
    );

    let results = select_components(&tree, ".tool_result", false).expect("select tool_result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].result, Some(serde_json::json!({"stdout": "ok"})));
    assert_eq!(results[0].visibility.as_deref(), Some("hidden"));
}

#[test]
fn select_components_tree_populates_rich_canonical_fields() {
    let value: Value = serde_json::from_str(
        r#"{
              "id": "root",
              "componentType": "root",
              "children": [
                {
                  "id": "turn",
                  "componentType": "message_turn",
                  "role": "assistant",
                  "children": [
                    {
                      "id": "call",
                      "componentType": "tool_call",
                      "offset": 0,
                      "tool_call_id": "tc_1",
                      "tool_name": "shell",
                      "parameters": {"cmd": "ls"},
                      "attrs": {"vendor": "acme"}
                    },
                    {
                      "id": "text",
                      "componentType": "text_content",
                      "offset": 1,
                      "content_ref": {"id": "blob_text"},
                      "source_ref": {"kind": "file", "path": "/x"}
                    }
                  ]
                }
              ]
            }"#,
    )
    .unwrap();
    let tree = ComponentTree::from_value(&value).expect("component tree");

    let calls = select_components_tree(&tree, ".tool_call", false).expect("select tool_call");

    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.id, "call");
    assert_eq!(call.parent_id.as_deref(), Some("turn"));
    assert_eq!(call.tool_call_id.as_deref(), Some("tc_1"));
    assert_eq!(call.tool_name.as_deref(), Some("shell"));
    assert_eq!(call.parameters, Some(serde_json::json!({"cmd": "ls"})));
    assert_eq!(call.aux.get("vendor").and_then(Value::as_str), Some("acme"));

    let text = select_components_tree(&tree, ".text_content", false).expect("select text");
    assert_eq!(text.len(), 1);
    assert_eq!(
        text[0].content_ref,
        Some(serde_json::json!({"id": "blob_text"}))
    );
    assert_eq!(
        text[0].source_ref,
        Some(serde_json::json!({"kind": "file", "path": "/x"}))
    );
}

#[test]
fn native_selector_does_not_use_removed_attr_fallbacks() {
    let mut root = Component::from_component_value(&serde_json::json!({
        "id": "root",
        "componentType": "root",
        "children": [{
            "id": "text",
            "componentType": "text_content",
            "offset": 0,
            "content": "native",
            "aux": {
                "node_type": "text_content",
                "cadence": 1
            }
        }]
    }))
    .expect("component");
    let child = root.children.get_mut(0).expect("child");
    child.lifecycle.policy.ttl = Some(1);
    child.lifecycle.policy.cad = Some(2);
    let tree = ComponentTree::from_root(root).expect("tree");

    assert!(
        select_components_tree(&tree, "{node_type=text_content}", false)
            .expect("native selector")
            .is_empty(),
        "native selectors must not treat removed node_type as canonical"
    );
    assert!(
        select_components_tree(&tree, ".text_content[cadence=1]", false)
            .expect("native selector")
            .is_empty(),
        "native selectors must read cadence from lifecycle policy, not aux"
    );
    assert_eq!(
        select_components_tree(&tree, ".text_content[cadence=2]", false)
            .expect("native selector")
            .len(),
        1
    );
}

#[test]
fn json_selector_rejects_removed_projection_fallback() {
    let json = r#"{
            "id": "root",
            "componentType": "root",
            "children": [{
                "id": "text",
                "componentType": "text_content",
                "attrs": {
                    "node_type": "text_content",
                    "cadence": 1
                }
            }]
        }"#;

    assert!(
        select_json(json, "{node_type=text_content}")
            .expect_err("removed node_type is rejected by native JSON selector")
            .to_string()
            .contains("cad without ttl is invalid")
    );
}

#[test]
fn select_components_include_archived_controls_visibility() {
    let tree: Value = serde_json::from_str(
            r#"{
              "id": "root",
              "componentType": "root",
              "children": [
                {"id": "live", "componentType": "text_content", "type": "text_content", "offset": 0, "content": "live"},
                {"id": "gone", "componentType": "text_content", "type": "text_content", "offset": 1, "content": "gone", "deleted": true},
                {"id": "old", "componentType": "text_content", "type": "text_content", "offset": 2, "content": "old", "attrs": {"archived": true}}
              ]
            }"#,
        )
        .unwrap();

    let visible = select_components(&tree, ".text_content", false).expect("select default");
    let visible_ids: Vec<&str> = visible.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(visible_ids, vec!["live"]);

    let all = select_components(&tree, ".text_content", true).expect("select all");
    let all_ids: Vec<&str> = all.iter().map(|node| node.id.as_str()).collect();
    assert_eq!(all_ids, vec!["live", "gone", "old"]);
    assert!(all.iter().find(|node| node.id == "gone").unwrap().deleted);
    assert!(all.iter().find(|node| node.id == "old").unwrap().archived);
}

#[test]
fn select_components_json_round_trips() {
    let json = r#"{
            "id": "root",
            "componentType": "root",
            "children": [
                {"id": "t", "componentType": "text_content", "type": "text_content", "offset": 0, "content": "hi"}
            ]
        }"#;
    let out = select_components_json(json, ".text_content", false).expect("select_components_json");
    let parsed: Vec<SelectedComponent> = serde_json::from_str(&out).expect("decode");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "t");
    assert_eq!(parsed[0].content.as_deref(), Some("hi"));
}
