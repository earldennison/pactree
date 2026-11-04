use pactree::{
    PactErrorKind, PactSearchChangeCursor, PactSearchSnapshotInfo, SearchFreshnessPolicy,
    SearchMode, SearchSurface,
};

#[test]
fn v01_search_scope_rejects_composed_and_historical_surfaces() {
    assert_eq!(SearchSurface::parse("head").unwrap(), SearchSurface::Head);
    for value in ["history", "revision:12", "composed"] {
        let error = SearchSurface::parse(value).unwrap_err();
        assert_eq!(error.kind(), PactErrorKind::Invalid);
    }
}

#[test]
fn v01_search_surface_supports_head_and_explicit_branch() {
    assert_eq!(SearchSurface::parse("head").unwrap(), SearchSurface::Head);
    assert_eq!(
        SearchSurface::parse("branch:main").unwrap(),
        SearchSurface::Branch("main".to_string())
    );

    let error = SearchSurface::parse("branch:").unwrap_err();
    assert_eq!(error.kind(), PactErrorKind::Invalid);
    let error = SearchSurface::parse("branch").unwrap_err();
    assert_eq!(error.kind(), PactErrorKind::Invalid);
}

#[test]
fn v01_search_surface_rejects_ledger_wide_and_overlay_surfaces() {
    for value in [
        "ledger",
        "ledger-wide",
        "composed-delta-overlay",
        "revision:0",
        "history:main",
        "",
        "HEAD",
    ] {
        let error = SearchSurface::parse(value).unwrap_err();
        assert_eq!(
            error.kind(),
            PactErrorKind::Invalid,
            "surface {value:?} must be a typed Invalid rejection"
        );

        assert!(
            error.to_string().contains("search surface"),
            "error for {value:?} should name the search surface contract: {error}"
        );
    }
}

#[test]
fn v01_search_surface_round_trips_canonical_tokens() {
    assert_eq!(SearchSurface::Head.as_token(), "head");
    assert_eq!(
        SearchSurface::Branch("main".to_string()).as_token(),
        "branch:main"
    );
    for token in ["head", "branch:feature-x"] {
        let surface = SearchSurface::parse(token).unwrap();
        assert_eq!(surface.as_token(), token);
    }
}

#[test]
fn v01_search_modes_are_the_five_frozen_modes() {
    let cases = [
        ("exact", SearchMode::Exact),
        ("regex", SearchMode::Regex),
        ("lexical", SearchMode::Lexical),
        ("dense", SearchMode::Dense),
        ("hybrid", SearchMode::Hybrid),
    ];
    for (token, mode) in cases {
        assert_eq!(SearchMode::parse(token).unwrap(), mode);
        assert_eq!(mode.as_str(), token);
    }
    for bad in ["fuzzy", "semantic", "EXACT", ""] {
        let error = SearchMode::parse(bad).unwrap_err();
        assert_eq!(error.kind(), PactErrorKind::Invalid);
    }
}

#[test]
fn v01_freshness_policy_parses_and_defaults_to_require_current() {
    assert_eq!(
        SearchFreshnessPolicy::default(),
        SearchFreshnessPolicy::RequireCurrent
    );
    assert_eq!(
        SearchFreshnessPolicy::parse("require_current").unwrap(),
        SearchFreshnessPolicy::RequireCurrent
    );
    assert_eq!(
        SearchFreshnessPolicy::parse("allow_stale").unwrap(),
        SearchFreshnessPolicy::AllowStale
    );
    let error = SearchFreshnessPolicy::parse("eventual").unwrap_err();
    assert_eq!(error.kind(), PactErrorKind::Invalid);
}

#[test]
fn v01_snapshot_info_separates_file_identity_from_root_identity() {
    let cursor = PactSearchChangeCursor {
        source_file_identity: "file-identity-a".to_string(),
        root_id: "root-a".to_string(),
        branch_id: "main".to_string(),
        revision: 7,
        ordinal: 42,
    };
    let info = PactSearchSnapshotInfo {
        source_file_identity: "file-identity-a".to_string(),
        root_id: "root-a".to_string(),
        branch_id: "main".to_string(),
        head_revision: 7,
        search_cursor: cursor.clone(),
        physical_generation: 3,
    };

    assert_ne!(info.source_file_identity, info.root_id);
    assert_eq!(info.search_cursor, cursor);

    let recompacted = PactSearchSnapshotInfo {
        physical_generation: 4,
        ..info.clone()
    };
    assert_eq!(recompacted.search_cursor, info.search_cursor);
    assert_eq!(recompacted.head_revision, info.head_revision);
}
