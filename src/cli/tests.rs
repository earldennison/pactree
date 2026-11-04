use super::{Cli, Command, parse_known};
use crate::PactErrorKind;
use std::ffi::OsString;
use std::path::PathBuf;

fn parse(args: &[&str]) -> Cli {
    parse_known(args.iter().map(OsString::from))
        .expect("parse")
        .expect("known command")
}

#[test]
fn validation_aliases_share_one_typed_command() {
    for command in ["file-validate", "check", "doctor"] {
        assert_eq!(
            parse(&["pactree", command, "session.pact"]).command,
            Command::Validate {
                path: PathBuf::from("session.pact")
            }
        );
    }
}

#[test]
fn rewind_disambiguates_optional_branch_from_revision() {
    assert_eq!(
        parse(&["pactree", "file-rewind-to", "session.pact", "12"]).command,
        Command::Rewind {
            path: PathBuf::from("session.pact"),
            branch_id: None,
            revision: 12,
        }
    );
    assert_eq!(
        parse(&["pactree", "file-rewind-to", "session.pact", "feature", "12",]).command,
        Command::Rewind {
            path: PathBuf::from("session.pact"),
            branch_id: Some("feature".to_string()),
            revision: 12,
        }
    );
}

#[test]
fn rewind_rejects_malformed_revision() {
    let error = parse_known(
        ["pactree", "file-rewind-to", "session.pact", "bad"]
            .into_iter()
            .map(OsString::from),
    )
    .expect_err("malformed revision");
    assert_eq!(error.kind(), PactErrorKind::Invalid);
    assert!(error.to_string().contains("invalid rewind revision"));
}

#[test]
fn repair_aliases_share_one_typed_command() {
    let expected = Command::Repair {
        source: PathBuf::from("source.pact"),
        output: PathBuf::from("repaired.pact"),
        truncate_tail: true,
    };
    assert_eq!(
        parse(&[
            "pactree",
            "file-repair",
            "source.pact",
            "repaired.pact",
            "--truncate-tail",
        ])
        .command,
        expected
    );
    assert_eq!(
        parse(&[
            "pactree",
            "heal",
            "source.pact",
            "--out",
            "repaired.pact",
            "--truncate-tail",
        ])
        .command,
        expected
    );
}

#[test]
fn version_is_a_typed_command() {
    assert_eq!(parse(&["pactree", "--version"]).command, Command::Version);
}
