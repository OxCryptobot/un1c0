use std::collections::BTreeSet;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::{GenerationError, IncrementalCodeGenerator, TargetBinding};
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_session::{SemanticSession, SemanticSessionError};
use un1c0::walker::{python_to_ueg, Ueg};

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
fn warm_leaf_refresh_reuses_unchanged_reverse_callers() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    let refresh = session
        .refresh(&changed, &BTreeSet::from([0]), &profile)
        .expect("valid changed leaf");
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
    assert!(refresh.validation.report.is_valid());
    assert!(session.is_valid());
    assert_eq!(
        session
            .snapshot_for(TargetBinding::Rust)
            .expect("rust snapshot")
            .fingerprint(),
        refresh.snapshot.fingerprint()
    );
}

#[test]
fn semantic_error_clears_snapshot_and_blocks_emission() {
    let base = parse_ueg(&fixture("value + 1"));
    let invalid = parse_ueg(&fixture("value + missing_name"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let old_snapshot = session.snapshot().cloned().expect("initial snapshot");

    let result = session.refresh(&invalid, &BTreeSet::from([0]), &profile);
    assert!(result.is_err());
    assert!(!session.is_valid());
    assert!(matches!(
        session.snapshot_for(TargetBinding::Rust),
        Err(SemanticSessionError::Invalidated)
    ));

    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);
    let error = generator
        .emit_remaining_with_snapshot(&invalid, &old_snapshot, |_| Ok::<(), &'static str>(()))
        .expect_err("no absent snapshot can be emitted");
    assert!(matches!(error, GenerationError::ValidationSnapshot { .. }));
}

#[test]
fn structural_change_invalidates_session_and_rejects_target_or_profile_changes() {
    let base = parse_ueg(&fixture("value + 1"));
    let structural = parse_ueg(&format!(
        "{}\ndef extra(value: int) -> int:\n    return value\n",
        fixture("value + 1")
    ));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");

    assert!(matches!(
        session.refresh(&structural, &BTreeSet::from([0]), &profile),
        Err(SemanticSessionError::StructuralChange)
    ));
    assert!(!session.is_valid());

    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    assert!(matches!(
        session.snapshot_for(TargetBinding::Rust),
        Err(SemanticSessionError::TargetChanged { .. })
    ));
    let mut incompatible = profile.clone();
    incompatible.supports_calls = false;
    assert!(matches!(
        session.refresh(&base, &BTreeSet::from([0]), &incompatible),
        Err(SemanticSessionError::ProfileChanged)
    ));
    assert!(!session.is_valid());
}

#[test]
fn current_snapshot_emits_after_valid_refresh_and_rejects_stale_source() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 3"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let refresh = session
        .refresh(&changed, &BTreeSet::from([0]), &profile)
        .expect("valid refresh");

    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Go);
    let mut chunks = 0;
    generator
        .emit_remaining_with_snapshot(&changed, &refresh.snapshot, |chunk| {
            chunks += 1;
            assert_eq!(chunk.target, TargetBinding::Go);
            Ok::<(), &'static str>(())
        })
        .expect("fresh snapshot permits emission");
    assert_eq!(chunks, changed.nodes.len());

    let stale = parse_ueg(&fixture("value + 4"));
    let mut stale_generator = IncrementalCodeGenerator::new(TargetBinding::Go);
    let error = stale_generator
        .emit_remaining_with_snapshot(
            &stale,
            session
                .snapshot_for(TargetBinding::Go)
                .expect("current snapshot"),
            |_| Ok::<(), &'static str>(()),
        )
        .expect_err("stale source must be rejected");
    assert!(matches!(error, GenerationError::ValidationSnapshot { .. }));
}
