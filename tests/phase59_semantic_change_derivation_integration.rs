use std::collections::BTreeSet;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_session::{SemanticSession, SemanticSessionError};
use un1c0::walker::{python_to_ueg, DiagnosticSeverity, SourceSpan, Ueg, UegDiagnostic};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn fixture(leaf_expression: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {leaf_expression}\n\ndef middle(value: int) -> int:\n    return leaf(value)\n\ndef root(value: int) -> int:\n    return middle(value)\n\ndef unrelated(value: int) -> int:\n    return value + 7\n"
    )
}

#[test]
fn derives_exact_function_changes_and_refreshes_transitive_callers() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    let changes = session
        .derive_change_set(&changed, &profile)
        .expect("derive changes");
    assert_eq!(changes.changed_functions, BTreeSet::from([0]));
    assert_eq!(changes.unchanged_functions, BTreeSet::from([1, 2, 3]));
    assert_eq!(changes.previous_function_count, 4);
    assert_eq!(changes.current_function_count, 4);
    assert_ne!(changes.previous_root, changes.current_root);

    let refresh = session
        .refresh_auto(&changed, &profile)
        .expect("auto refresh");
    assert_eq!(
        refresh.validation.affected_functions,
        BTreeSet::from([0, 1, 2])
    );
    assert_eq!(
        refresh.validation.revalidated_functions,
        BTreeSet::from([0])
    );
    assert_eq!(refresh.validation.cache_hits, 2);
    assert_eq!(refresh.validation.cache_misses, 1);
}

#[test]
fn declared_change_set_must_match_fingerprint_derivation() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    let result = session.refresh(&changed, &BTreeSet::from([1]), &profile);
    assert!(matches!(
        result,
        Err(SemanticSessionError::ChangedSetMismatch { .. })
    ));
    assert!(!session.is_valid());
}

#[test]
fn unchanged_ueg_is_a_zero_work_refresh_and_preserves_snapshot() {
    let base = parse_ueg(&fixture("value + 1"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let before = session.snapshot().cloned().expect("initial snapshot");

    let refresh = session
        .refresh_auto(&base, &profile)
        .expect("no-op refresh");
    assert!(refresh.validation.affected_functions.is_empty());
    assert!(refresh.validation.revalidated_functions.is_empty());
    assert_eq!(refresh.validation.cache_hits, 0);
    assert_eq!(refresh.validation.cache_misses, 0);
    assert_eq!(refresh.snapshot, before);
}

#[test]
fn blocking_diagnostic_cannot_hide_behind_a_no_op_fingerprint() {
    let base = parse_ueg(&fixture("value + 1"));
    let mut invalid = base.clone();
    invalid.diagnostics.push(UegDiagnostic {
        code: "UEG-PARSER-ERROR".to_string(),
        message: "synthetic blocking parser error".to_string(),
        severity: DiagnosticSeverity::Error,
        span: SourceSpan::default(),
    });
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    let result = session.refresh_auto(&invalid, &profile);
    assert!(matches!(result, Err(SemanticSessionError::Incremental(_))));
    assert!(!session.is_valid());
}

#[test]
fn structural_change_invalidates_before_change_derivation() {
    let base = parse_ueg(&fixture("value + 1"));
    let structural = parse_ueg(&format!(
        "{}\ndef extra(value: int) -> int:\n    return value\n",
        fixture("value + 1")
    ));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    assert!(matches!(
        session.derive_change_set(&structural, &profile),
        Err(SemanticSessionError::StructuralChange)
    ));
    assert!(matches!(
        session.snapshot_for(TargetBinding::Rust),
        Err(SemanticSessionError::Invalidated)
    ));
}
