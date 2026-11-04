use super::*;

#[test]
fn parses_history_and_star() {
    let parsed = parse_corpus_selector("@history .text_content").expect("parse");
    assert!(matches!(parsed.temporal.start, TemporalPoint::History));
    assert_eq!(parsed.structural_selector, ".text_content");

    let parsed = parse_corpus_selector("@* .text_content").expect("parse");
    assert!(matches!(parsed.temporal.start, TemporalPoint::History));
}

#[test]
fn parses_relative_and_absolute_points() {
    let parsed = parse_corpus_selector("@t0 .text_content").expect("parse");
    assert!(matches!(parsed.temporal.start, TemporalPoint::Current));

    let parsed = parse_corpus_selector("@t-3 .message_turn").expect("parse");
    assert_eq!(parsed.temporal.start, TemporalPoint::Relative(-3));

    let parsed = parse_corpus_selector("@c12 .tool_result").expect("parse");
    assert_eq!(parsed.temporal.start, TemporalPoint::Cycle(12));
}

#[test]
fn parses_ranges_with_dotdot_and_colon() {
    let parsed = parse_corpus_selector("@c3..@c7 .text_content").expect("parse");
    assert_eq!(parsed.temporal.start, TemporalPoint::Cycle(3));
    assert_eq!(parsed.temporal.end, Some(TemporalPoint::Cycle(7)));

    let parsed = parse_corpus_selector("@t-5:@t-1 .text_content").expect("parse");
    assert_eq!(parsed.temporal.start, TemporalPoint::Relative(-5));
    assert_eq!(parsed.temporal.end, Some(TemporalPoint::Relative(-1)));
}

#[test]
fn defaults_to_current_when_no_prefix() {
    let parsed = parse_corpus_selector(".text_content").expect("parse");
    assert!(matches!(parsed.temporal.start, TemporalPoint::Current));
    assert_eq!(parsed.structural_selector, ".text_content");
}

#[test]
fn rejects_positive_t_aliases() {
    let err = parse_corpus_selector("@t1 .text_content").expect_err("must reject");
    assert!(
        err.to_string()
            .contains("positive relative temporal aliases")
    );
}

#[test]
fn rejects_mixed_axes_ranges() {
    let err = parse_corpus_selector("@c1..@t-1 .text_content").expect_err("must reject");
    assert!(err.to_string().contains("mixes axes"));
}

#[test]
fn requires_structural_selector_after_prefix() {
    let err = parse_corpus_selector("@history").expect_err("must reject");
    assert!(err.to_string().contains("structural selector"));
}

#[test]
fn plan_classifies_node_local_versus_window() {
    assert_eq!(
        compile_corpus_selector_plan(".text_content").expect("plan"),
        CorpusSelectorPlan::NodeLocal
    );
    assert_eq!(
        compile_corpus_selector_plan(".tool_result[success=false]").expect("plan"),
        CorpusSelectorPlan::NodeLocal
    );
    assert_eq!(
        compile_corpus_selector_plan(".message_turn > .text_content").expect("plan"),
        CorpusSelectorPlan::SnapshotWindow
    );
    assert_eq!(
        compile_corpus_selector_plan(".tool_result:has(.text_content)").expect("plan"),
        CorpusSelectorPlan::SnapshotWindow
    );
}

#[test]
fn indexed_selector_hit_ignores_structured_payload_field_shapes() {
    let hit = indexed_selector_hit(
        serde_json::json!({
            "id": "result_1",
            "componentType": "tool_result",
            "parent_id": "message_1",
            "tool_call_id": "call_1",
            "result": {"rows": [{"value": 7}]},
            "content_ref": {"id": "cnt_1", "hash": "abc"},
        }),
        "main",
    )
    .expect("selector hit");

    assert_eq!(hit.component_id, "result_1");
    assert_eq!(hit.element_kind, "tool_result");
    assert_eq!(hit.parent_id.as_deref(), Some("message_1"));
}
