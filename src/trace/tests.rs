use super::*;

#[test]
fn boundary_kind_round_trips_snake_case() {
    let k = BoundaryKind::PostRequestAlteration;
    let s = serde_json::to_string(&k).unwrap();
    assert_eq!(s, "\"post_request_alteration\"");
    let back: BoundaryKind = serde_json::from_str(&s).unwrap();
    assert_eq!(back, k);
}

#[test]
fn resolve_outcome_default_schema() {
    let outcome = ResolveOutcome {
        schema: default_resolve_outcome_schema(),
        branch_id: "main".to_string(),
        boundary: BoundaryKind::Authored,
        accepted: 0,
        deduped: 0,
        applied: vec![],
        rejected: vec![],
        produced_component_ids: vec![],
        produced_message_ids: vec![],
        produced_message_child_ids: vec![],
        replaced_component_ids: vec![],
        deleted_component_ids: vec![],
        affected_component_ids: vec![],
        pending_external_mutations: vec![],
        dirty_projections: vec![],
        traces: vec![],
        outcome: crate::boundary::BoundaryOutcome::Committed,
        commit_candidate: None,
    };
    assert_eq!(outcome.schema, "pact.resolve_outcome.v1");
}
