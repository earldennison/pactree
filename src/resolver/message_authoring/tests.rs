use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::{
    AuthoringIdentity, DeterministicIdentity, author_components, insert_subtree_trace_ops,
    lower_message, stamp_insert_identity, trace_identity,
};
use crate::components::Component;
use crate::ops::PactOperation;
use crate::transaction_trace::{TraceOp, TraceOpKind};

fn identity() -> DeterministicIdentity {
    DeterministicIdentity::new(
        crate::DeterministicAuthoringOptions::new("t_".to_string(), 5_000, 10).expect("options"),
    )
    .expect("identity")
}

fn text(content: &str) -> Component {
    Component::text_authoring(content.to_string(), Default::default())
}

#[test]
fn deterministic_identity_matches_the_resolver_state_sequence() {
    let mut identity = identity();
    assert_eq!(identity.next_local_id(), "t_00000001");
    assert_eq!(identity.next_node_id("msg"), "msg_t_00000002");
    assert_eq!(identity.next_timestamp_ns(), 5_000);
    assert_eq!(identity.next_timestamp_ns(), 5_010);
}

#[test]
fn authoring_rehomes_ids_and_mints_around_collisions() {
    let mut identity = identity();
    let mut authored = BTreeSet::new();
    let mut used = |id: &str| Ok(id == "ctx:t_00000001" || id == "ctx:taken");
    let mut parent = text("p");
    parent.id = "other:taken".to_string();
    let mut c1 = text("c1");
    c1.id = String::new();
    let mut c2 = text("c2");
    c2.id = String::new();
    parent.children = vec![c1, c2];
    let mut dup = text("d");
    dup.id = "x:dup".to_string();
    let mut dup2 = text("d2");
    dup2.id = "dup".to_string();
    let parts = author_components(
        "ctx",
        vec![parent, dup, dup2],
        &mut identity,
        &mut used,
        &mut authored,
    )
    .expect("authored");

    assert_eq!(parts[0].id, "ctx:t_00000002");
    assert_eq!(parts[0].children[0].id, "ctx:t_00000003");
    assert_eq!(
        parts[0].children[0].parent_id.as_deref(),
        Some("ctx:t_00000002")
    );
    assert_eq!(parts[0].children[1].id, "ctx:t_00000004");
    assert_eq!(parts[1].id, "ctx:dup");
    assert_eq!(
        parts[2].id, "ctx:t_00000005",
        "a duplicate within the batch mints fresh"
    );
    assert_eq!(authored.len(), 5);
}

#[test]
fn lowering_draws_timestamp_then_container_then_part_ids() {
    let mut identity = identity();
    let mut part = text("hello");
    part.id = String::new();
    let mut aux = BTreeMap::new();
    aux.insert("k".to_string(), json!(1));
    let op = lower_message(
        "root",
        "main",
        "assistant",
        vec![part],
        aux.clone(),
        &mut identity,
    )
    .expect("lowered")
    .expect("an op");
    let PactOperation::Insert {
        envelope,
        parent_id,
        component,
        ..
    } = &op
    else {
        panic!("insert expected");
    };
    assert_eq!(envelope.host_timestamp_ns, Some(5_000));
    assert_eq!(envelope.aux, aux);
    assert_eq!(parent_id.as_deref(), Some("root"));
    assert_eq!(component.id, "msg_t_00000001");
    assert_eq!(
        component.pact_depth,
        Some(1),
        "assistant turns author at depth 1"
    );
    assert_eq!(component.children[0].id, "node_t_00000002");
    assert_eq!(
        component.children[0].parent_id.as_deref(),
        Some("msg_t_00000001")
    );
    assert_eq!(component.children[0].pact_depth, Some(1));
    assert_eq!(component.aux, aux);

    let system = lower_message(
        "root",
        "main",
        "system",
        vec![text("a"), text("b")],
        BTreeMap::new(),
        &mut identity,
    )
    .expect("lowered")
    .expect("an op");
    let PactOperation::Insert {
        parent_id,
        component,
        ..
    } = &system
    else {
        panic!("insert expected");
    };
    assert_eq!(parent_id.as_deref(), Some("system"));
    assert_eq!(component.id, "sys_t_00000003");
    assert_eq!(component.content(), Some("a\nb"));
    assert_eq!(component.pact_depth, Some(-1));

    assert!(
        lower_message(
            "root",
            "main",
            "system",
            vec![],
            BTreeMap::new(),
            &mut identity
        )
        .expect("ok")
        .is_none(),
        "an empty system message is a no-op"
    );
    assert!(
        lower_message(
            "root",
            "main",
            "tool",
            vec![text("x")],
            BTreeMap::new(),
            &mut identity
        )
        .is_err()
    );
    assert!(
        lower_message(
            "root",
            "main",
            "user",
            vec![],
            BTreeMap::new(),
            &mut identity
        )
        .is_err()
    );
    let mut reasoning = Component::reasoning_authoring("r".to_string(), Default::default());
    reasoning.id = String::new();
    assert!(
        lower_message(
            "root",
            "main",
            "system",
            vec![reasoning],
            BTreeMap::new(),
            &mut identity
        )
        .is_err()
    );
}

#[test]
fn identity_stamp_fills_only_missing_creation_times() {
    let mut identity = identity();
    let mut stamped = text("s");
    stamped.created_at_ns = 77;
    let mut fresh = text("f");
    fresh.created_at_ns = 0;
    let mut op = lower_message(
        "root",
        "main",
        "user",
        vec![stamped, fresh],
        BTreeMap::new(),
        &mut identity,
    )
    .expect("lowered")
    .expect("op");
    stamp_insert_identity(&mut op, &mut identity);
    let PactOperation::Insert {
        component,
        envelope,
        ..
    } = &op
    else {
        panic!("insert expected");
    };
    assert_eq!(component.created_at_ns, 5_000);
    assert_eq!(component.children[0].created_at_ns, 77);
    assert_eq!(component.children[1].created_at_ns, 5_000);
    assert_eq!(envelope.host_timestamp_ns, Some(5_000));
    let first = trace_identity(&op, 5_000, 1);
    assert!(first.starts_with("trace_") && first.len() == 22);
    assert_ne!(
        first,
        trace_identity(&op, 5_000, 2),
        "the session counter is part of the id"
    );
}

#[test]
fn insert_subtree_ops_follow_the_resolver_order() {
    let mut turn = text("t");
    turn.id = "m".to_string();
    let mut a = text("a");
    a.id = "a".to_string();
    let mut a1 = text("a1");
    a1.id = "a1".to_string();
    a.children = vec![a1];
    let mut b = text("b");
    b.id = "b".to_string();
    turn.children = vec![a, b];
    let mut ops: Vec<TraceOp> = Vec::new();
    insert_subtree_trace_ops(
        &turn,
        "root",
        3,
        Some(&["sys".to_string(), "m".to_string()]),
        &mut |op| {
            ops.push(op);
            Ok(())
        },
    )
    .expect("ops");
    let shape = ops
        .iter()
        .map(|op| match &op.kind {
            TraceOpKind::InsertComponent {
                parent_id,
                index,
                component,
            } => {
                assert_eq!(component["children"], json!([]));
                format!("insert {} under {parent_id}@{index}", op.component_id)
            }
            TraceOpKind::SetField { key, value } => format!("{} {key}={value}", op.component_id),
            other => panic!("unexpected {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        vec![
            "insert m under root@3",
            "root child_ids=[\"sys\",\"m\"]",
            "insert a under m@0",
            "m child_ids=[\"a\",\"b\"]",
            "insert a1 under a@0",
            "a child_ids=[\"a1\"]",
            "insert b under m@1",
            "m child_ids=[\"a\",\"b\"]",
        ]
    );
}
