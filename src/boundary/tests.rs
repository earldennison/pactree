use super::*;

fn req(
    key: &str,
    kind: ExternalMutationKind,
    boundary: BoundaryKind,
) -> ExternalMutationRequirement {
    ExternalMutationRequirement {
        schema: default_requirement_schema(),
        requirement_id: format!("req_{key}"),
        idempotency_key: key.to_string(),
        kind,
        boundary,
        branch_id: "main".to_string(),
        subject_id: None,
        payload: BTreeMap::new(),
        aux: BTreeMap::new(),
    }
}

#[test]
fn empty_pending_commits_immediately() {
    let pending = PendingExternalMutations::new();
    let policy = BoundaryPolicy::for_kind(BoundaryKind::PreRequest);
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::Committed);
}

#[test]
fn provider_prep_waits_on_pending_requirements() {
    let mut pending = PendingExternalMutations::new();
    pending.record(req(
        "k1",
        ExternalMutationKind::RenderComponent,
        BoundaryKind::PreRequest,
    ));
    let policy = BoundaryPolicy::for_kind(BoundaryKind::PreRequest);
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::WaitingExternal);
}

#[test]
fn fulfilled_requirement_allows_commit() {
    let mut pending = PendingExternalMutations::new();
    pending.record(req(
        "k1",
        ExternalMutationKind::RenderComponent,
        BoundaryKind::PreRequest,
    ));
    let policy = BoundaryPolicy::for_kind(BoundaryKind::PreRequest);
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::WaitingExternal);

    let fulfilled = pending.fulfill("k1");
    assert!(fulfilled.is_some());
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::Committed);
}

#[test]
fn failure_blocks_commit_when_policy_requires() {
    let mut pending = PendingExternalMutations::new();
    pending.record(req(
        "k1",
        ExternalMutationKind::RenderComponent,
        BoundaryKind::PreRequest,
    ));
    pending.record_failure("k1", "render exploded".to_string());
    let policy = BoundaryPolicy::for_kind(BoundaryKind::PreRequest);
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::Failed);
}

#[test]
fn adhoc_boundary_treats_pending_as_deferred() {
    let mut pending = PendingExternalMutations::new();
    pending.record(req(
        "k1",
        ExternalMutationKind::EstimateTokens,
        BoundaryKind::Adhoc,
    ));
    let policy = BoundaryPolicy::for_kind(BoundaryKind::Adhoc);
    assert_eq!(pending.evaluate(&policy), BoundaryOutcome::Deferred);
}

#[test]
fn requirement_round_trips_json() {
    let r = req(
        "k1",
        ExternalMutationKind::ExecuteTool,
        BoundaryKind::PostResponseAlteration,
    );
    let s = serde_json::to_string(&r).unwrap();
    let back: ExternalMutationRequirement = serde_json::from_str(&s).unwrap();
    assert_eq!(r, back);
}
