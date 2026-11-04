use super::{MembershipSink, stream_branch_membership};
use crate::error::PactResult;

#[derive(Default)]
struct Collect {
    first_seen: Vec<(String, u64)>,
    lifecycle_seen: Vec<(String, u64)>,
}

impl MembershipSink for Collect {
    fn first_seen(&mut self, component_id: &str, revision: u64) -> PactResult<()> {
        self.first_seen.push((component_id.to_string(), revision));
        Ok(())
    }

    fn lifecycle_seen(&mut self, component_id: &str, tick: u64) -> PactResult<()> {
        self.lifecycle_seen.push((component_id.to_string(), tick));
        Ok(())
    }
}

fn ledger_json(branch: &str) -> String {
    format!(
        r#"{{"schema":"pactree.root_store.v1","root_id":"root","current_branch_id":"{branch}",
        "branches":{{
          "other":{{"head_revision":9,"lifecycle_tick":3,"base_ref":{{"root_id":"root","branch_id":"other","revision":null,"transaction_id":null,"component_id":null,"content_ref":null}},
                    "current":{{"tree":{{"first_seen":{{"zzz":9}}}},"lifecycle_seen":{{"zzz":9}}}}}},
          "{branch}":{{"parent_branch_id":null,"fork_ref":null,"head_revision":48,"lifecycle_tick":7,
                       "base_ref":{{"root_id":"root","branch_id":"{branch}","revision":0,"transaction_id":null,"component_id":null,"content_ref":null}},
                       "transaction_traces":[{{"huge":[1,2,3]}}],
                       "current":{{"branch_id":"{branch}","tree":{{"root_id":"root","root":{{"id":"root","children":[{{"id":"a","content":"x"}}]}},"first_seen":{{"a":1,"b":2,"c":3}},"tool_pairs":{{}}}},
                                   "lifecycle_seen":{{"a":5,"c":6}},"message_order":["a"],"content_refs":{{}}}},
                       "aux":{{}}}}
        }},
        "event_index":{{"transaction_traces":[{{"ops":[1,2,3]}}]}}}}"#
    )
}

#[test]
fn full_ledger_and_compact_envelope_yield_the_same_membership() {
    for (label, json) in [
        ("full", ledger_json("main")),
        (
            "compact",
            format!(
                r#"{{"schema":"pact.compact_head_ledger.v1","ledger":{}}}"#,
                ledger_json("main")
            ),
        ),
    ] {
        let mut sink = Collect::default();
        let scalars = stream_branch_membership(json.as_bytes(), false, "main", &mut sink)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert!(scalars.found, "{label}");
        assert_eq!(scalars.head_revision, Some(48), "{label}");
        assert_eq!(scalars.lifecycle_tick, 7, "{label}");
        assert_eq!(
            scalars.base_ref.as_ref().and_then(|r| r.revision),
            Some(0),
            "{label}"
        );
        assert!(
            !scalars.has_parent_branch && !scalars.has_fork_ref,
            "{label}"
        );
        assert_eq!(
            sink.first_seen,
            vec![("a".into(), 1), ("b".into(), 2), ("c".into(), 3)],
            "{label}"
        );
        assert_eq!(
            sink.lifecycle_seen,
            vec![("a".into(), 5), ("c".into(), 6)],
            "{label}"
        );
    }
}

#[test]
fn sections_wrapper_missing_branch_and_sink_errors_are_reported() {
    let sections = format!(
        r#"{{"public_trace":{{"events":[1,2]}},"new_content":[],"head_checkpoint":{},"ledger_delta":null,"head_metadata":{{"x":1}}}}"#,
        ledger_json("main")
    );
    let mut sink = Collect::default();
    let scalars =
        stream_branch_membership(sections.as_bytes(), true, "main", &mut sink).expect("sections");
    assert_eq!(scalars.head_revision, Some(48));
    assert_eq!(sink.first_seen.len(), 3);

    let error = stream_branch_membership(
        ledger_json("main").as_bytes(),
        false,
        "absent",
        &mut Collect::default(),
    )
    .expect_err("missing branch");
    assert!(
        error.to_string().contains("no entry for the branch"),
        "{error}"
    );

    struct Failing;
    impl MembershipSink for Failing {
        fn first_seen(&mut self, _: &str, _: u64) -> PactResult<()> {
            Err(crate::PactError::storage("scratch is full"))
        }
        fn lifecycle_seen(&mut self, _: &str, _: u64) -> PactResult<()> {
            Ok(())
        }
    }
    let error =
        stream_branch_membership(ledger_json("main").as_bytes(), false, "main", &mut Failing)
            .expect_err("sink failure");
    assert_eq!(error.kind(), crate::PactErrorKind::Storage, "{error}");

    let error = stream_branch_membership(
        b"{\"branches\":[1,2]".as_slice(),
        false,
        "main",
        &mut Collect::default(),
    )
    .expect_err("truncated");
    assert!(
        error
            .to_string()
            .contains("invalid Pact baseline checkpoint"),
        "{error}"
    );
}
