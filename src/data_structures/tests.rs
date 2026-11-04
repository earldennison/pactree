use super::*;

#[test]
fn core_offset_insert_zero_pushes_old_core_to_post() {
    let mut array = CoreOffsetArray::new();
    assert_eq!(array.insert(0, "first").actual_offset, 0);
    let report = array.insert(0, "second");
    assert_eq!(report.actual_offset, 0);
    assert_eq!(
        report.shifted_offsets,
        vec![OffsetShift {
            from_offset: 0,
            to_offset: 1,
        }]
    );

    assert_eq!(array.get(0), Some(&"second"));
    assert_eq!(array.get(1), Some(&"first"));
    assert_eq!(array.offsets(), vec![0, 1]);
    assert!(array.contains_offset(0));
}

#[test]
fn core_offset_first_insert_becomes_core_even_when_requested_elsewhere() {
    let mut array = CoreOffsetArray::new();

    assert_eq!(array.insert(1, "core").actual_offset, 0);

    assert_eq!(array.get(0), Some(&"core"));
    assert_eq!(array.offsets(), vec![0]);
}

#[test]
fn core_offset_positive_insert_pushes_positive_stack_outward() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.insert(1, "old-post");

    let report = array.insert(1, "new-post");
    assert_eq!(report.actual_offset, 1);
    assert_eq!(
        report.shifted_offsets,
        vec![OffsetShift {
            from_offset: 1,
            to_offset: 2,
        }]
    );

    assert_eq!(array.get(1), Some(&"new-post"));
    assert_eq!(array.get(2), Some(&"old-post"));
    let ordered: Vec<&str> = array
        .iter_ordered()
        .into_iter()
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(ordered, vec!["core", "new-post", "old-post"]);
}

#[test]
fn core_offset_negative_insert_pushes_negative_stack_outward() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.insert(-1, "old-pre");

    let report = array.insert(-1, "new-pre");
    assert_eq!(report.actual_offset, -1);
    assert_eq!(
        report.shifted_offsets,
        vec![OffsetShift {
            from_offset: -1,
            to_offset: -2,
        }]
    );

    assert_eq!(array.get(-1), Some(&"new-pre"));
    assert_eq!(array.get(-2), Some(&"old-pre"));
    let ordered: Vec<&str> = array
        .iter_ordered()
        .into_iter()
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(ordered, vec!["old-pre", "new-pre", "core"]);
}

#[test]
fn core_offset_append_direction_uses_requested_offset_sign() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");

    assert_eq!(array.append("post-a", 1).actual_offset, 1);
    assert_eq!(array.append("post-b", 1).actual_offset, 2);
    assert_eq!(array.append("pre-a", -1).actual_offset, -1);
    assert_eq!(array.append("pre-b", -1).actual_offset, -2);

    assert_eq!(array.offsets(), vec![-2, -1, 0, 1, 2]);
    let ordered: Vec<&str> = array
        .iter_ordered()
        .into_iter()
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(ordered, vec!["pre-b", "pre-a", "core", "post-a", "post-b"]);
}

#[test]
fn core_offset_set_replaces_without_demoting_or_shifting() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.insert(1, "post");

    let report = array.set(0, "replaced-core").expect("replace core");
    assert_eq!(report, CoreOffsetReplaceReport { offset: 0 });
    array.set(1, "replaced-post").expect("replace post");

    assert_eq!(array.offsets(), vec![0, 1]);
    assert_eq!(array.get(0), Some(&"replaced-core"));
    assert_eq!(array.get(1), Some(&"replaced-post"));
    assert_eq!(
        array.replace(8, "missing"),
        Err(PactArrayError::InvalidReplacement {
            coordinate: "8".to_string(),
        })
    );
}

#[test]
fn core_offset_negative_and_positive_stacks_keep_pact_order() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.insert(-1, "pre1");
    array.insert(-1, "pre2");
    array.insert(1, "post1");
    array.insert(1, "post2");

    let ordered: Vec<&str> = array
        .iter_ordered()
        .into_iter()
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(ordered, vec!["pre1", "pre2", "core", "post2", "post1"]);
    assert_eq!(array.offsets(), vec![-2, -1, 0, 1, 2]);
}

#[test]
fn core_offset_remove_zero_promotes_first_post() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.insert(1, "post");

    let (removed, report) = array.remove(0).expect("remove core");
    assert_eq!(removed, "core");
    assert_eq!(
        report.shifted_offsets,
        vec![OffsetShift {
            from_offset: 1,
            to_offset: 0,
        }]
    );
    assert_eq!(array.get(0), Some(&"post"));
}

#[test]
fn core_offset_remove_negative_and_positive_offsets_shift_compactly() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core");
    array.append("pre-a", -1);
    array.append("pre-b", -1);
    array.append("post-a", 1);
    array.append("post-b", 1);

    let (removed_pre, pre_report) = array.remove(-1).expect("remove pre");
    assert_eq!(removed_pre, "pre-a");
    assert_eq!(
        pre_report.shifted_offsets,
        vec![OffsetShift {
            from_offset: -2,
            to_offset: -1,
        }]
    );
    let (removed_post, post_report) = array.remove(1).expect("remove post");
    assert_eq!(removed_post, "post-a");
    assert_eq!(
        post_report.shifted_offsets,
        vec![OffsetShift {
            from_offset: 2,
            to_offset: 1,
        }]
    );

    assert_eq!(array.offsets(), vec![-1, 0, 1]);
    assert_eq!(array.get(-1), Some(&"pre-b"));
    assert_eq!(array.get(0), Some(&"core"));
    assert_eq!(array.get(1), Some(&"post-b"));
}

#[test]
fn core_offset_serializes_stably_and_can_clear() {
    let mut array = CoreOffsetArray::new();
    array.insert(0, "core".to_string());
    array.append("pre".to_string(), -1);
    array.append("post".to_string(), 1);

    let json = serde_json::to_string(&array).expect("serialize core offsets");
    assert_eq!(json, r#"{"core":"core","pre":["pre"],"post":["post"]}"#);

    let decoded: CoreOffsetArray<String> =
        serde_json::from_str(&json).expect("decode core offsets");
    assert_eq!(decoded.offsets(), vec![-1, 0, 1]);
    assert_eq!(
        decoded.ordered_node_ids(),
        vec![
            (-1, "pre".to_string()),
            (0, "core".to_string()),
            (1, "post".to_string()),
        ]
    );

    array.clear();
    assert!(array.is_empty());
    assert_eq!(array.remove(0), Err(PactArrayError::MissingCore));
}

#[test]
fn depth_array_maps_storage_to_pact_depth() {
    let mut array = DepthArray::new();
    assert_eq!(array.append("sys").depth, -1);
    assert_eq!(array.append("active").depth, 0);
    assert_eq!(array.append("oldest").depth, 1);

    let items: Vec<(i32, &str)> = array
        .iter_depth_items()
        .map(|(depth, value)| (depth, *value))
        .collect();
    assert_eq!(items, vec![(-1, "sys"), (0, "active"), (1, "oldest")]);
    assert_eq!(array.depths(), vec![-1, 0, 1]);
    assert!(array.contains_depth(0));
    assert_eq!(array.max_depth(), 1);
    assert_eq!(
        array.ordered_node_ids(),
        vec![(-1, "sys"), (0, "active"), (1, "oldest")]
    );
}

#[test]
fn depth_array_protects_system_and_active_depths() {
    let mut array = DepthArray::new();
    array.append("sys");
    array.append("active");
    array.append("history");

    assert!(array.remove(-1).is_err());
    assert!(array.remove(0).is_err());
    assert_eq!(
        array.remove(-1),
        Err(PactArrayError::ProtectedDepthRemoval { depth: -1 })
    );
    let (removed, report) = array.remove(1).expect("remove history");
    assert_eq!(removed, "history");
    assert_eq!(report.shifted_depths, Vec::<DepthShift>::new());
}

#[test]
fn depth_array_insert_historical_depth_shifts_later_turns() {
    let mut array = DepthArray::new();
    array.append("sys");
    array.append("active");
    array.append("old-d1");
    array.append("old-d2");

    let report = array.insert(1, "new-d1").expect("insert historical");
    assert_eq!(
        report.shifted_depths,
        vec![
            DepthShift {
                from_depth: 1,
                to_depth: 2,
            },
            DepthShift {
                from_depth: 2,
                to_depth: 3,
            },
        ]
    );

    let items: Vec<(i32, &str)> = array
        .iter_depth_items()
        .map(|(depth, value)| (depth, *value))
        .collect();
    assert_eq!(
        items,
        vec![
            (-1, "sys"),
            (0, "active"),
            (1, "new-d1"),
            (2, "old-d1"),
            (3, "old-d2"),
        ]
    );
}

#[test]
fn depth_array_delete_historical_depth_shifts_later_turns_down() {
    let mut array = DepthArray::new();
    array.append("sys");
    array.append("active");
    array.append("old-d1");
    array.append("old-d2");
    array.append("old-d3");

    let (removed, report) = array.remove(2).expect("remove d2");
    assert_eq!(removed, "old-d2");
    assert_eq!(
        report.shifted_depths,
        vec![DepthShift {
            from_depth: 3,
            to_depth: 2,
        }]
    );

    let items: Vec<(i32, &str)> = array
        .iter_depth_items()
        .map(|(depth, value)| (depth, *value))
        .collect();
    assert_eq!(
        items,
        vec![(-1, "sys"), (0, "active"), (1, "old-d1"), (2, "old-d3"),]
    );
    assert_eq!(array.get(3), None);
}

#[test]
fn depth_array_serializes_stably_and_reports_invalid_operations() {
    let mut array = DepthArray::new();
    array.append("sys".to_string());
    array.append("active".to_string());

    let json = serde_json::to_string(&array).expect("serialize depth array");
    assert_eq!(json, r#"{"depths":["sys","active"]}"#);

    let decoded: DepthArray<String> = serde_json::from_str(&json).expect("decode depth array");
    assert_eq!(decoded.depths(), vec![-1, 0]);
    assert_eq!(
        DepthArray::<String>::index_for_depth(-2),
        Err(PactArrayError::InvalidDepth { depth: -2 })
    );
    assert_eq!(
        array.insert(3, "sparse".to_string()),
        Err(PactArrayError::SparseDepthInsert {
            depth: 3,
            max_depth: 0,
        })
    );
    assert_eq!(
        array.replace(9, "missing".to_string()),
        Err(PactArrayError::InvalidReplacement {
            coordinate: "d9".to_string(),
        })
    );

    array.clear();
    assert!(array.is_empty());
}
