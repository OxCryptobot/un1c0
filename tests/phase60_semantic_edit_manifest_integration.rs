use std::collections::BTreeSet;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_session::{
    SemanticEditManifestError, SemanticEditRange, SemanticSession, SemanticSessionError,
};
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

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

fn span(ueg: &Ueg, index: usize) -> (usize, usize) {
    let NodeKind::Lambda(lambda) = &ueg.nodes[index];
    (lambda.source_span.start_byte, lambda.source_span.end_byte)
}

#[test]
fn manifest_maps_leaf_edit_and_refreshes_conservative_callers() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let (start_byte, end_byte) = span(&base, 0);
    let manifest = session
        .manifest_for_edits(vec![SemanticEditRange::new(start_byte, end_byte).unwrap()])
        .expect("valid manifest");

    let resolution = session
        .derive_edit_resolution(&changed, &profile, &manifest)
        .expect("manifest resolution");
    assert_eq!(resolution.mapped_functions, BTreeSet::from([0]));
    assert_eq!(
        resolution.semantic_changes.changed_functions,
        BTreeSet::from([0])
    );

    let refresh = session
        .refresh_from_edit_manifest(&changed, &profile, &manifest)
        .expect("manifest-bound refresh");
    assert_eq!(
        refresh.validation.affected_functions,
        BTreeSet::from([0, 1, 2])
    );
    assert_eq!(
        refresh.validation.revalidated_functions,
        BTreeSet::from([0])
    );
    assert!(session.is_valid());
}

#[test]
fn manifest_base_and_profile_bindings_fail_closed() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let (start_byte, end_byte) = span(&base, 0);
    let manifest = session
        .manifest_for_edits(vec![SemanticEditRange::new(start_byte, end_byte).unwrap()])
        .expect("valid manifest");

    let mut wrong_profile = profile.clone();
    wrong_profile.supports_calls = false;
    assert!(matches!(
        session.derive_edit_resolution(&changed, &wrong_profile, &manifest),
        Err(SemanticSessionError::ProfileChanged)
    ));

    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let foreign = SemanticSession::start(&changed, profile.clone(), 16).expect("foreign session");
    let foreign_manifest = foreign
        .manifest_for_edits(vec![SemanticEditRange::new(start_byte, end_byte).unwrap()])
        .expect("foreign manifest");
    assert!(matches!(
        session.derive_edit_resolution(&changed, &profile, &foreign_manifest),
        Err(SemanticSessionError::EditManifestBaseMismatch)
    ));
    assert!(!session.is_valid());
}

#[test]
fn edit_ranges_must_map_to_one_function_and_cover_all_semantic_changes() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let (leaf_start, leaf_end) = span(&base, 0);
    let (middle_start, middle_end) = span(&base, 1);

    let overlapping = SemanticEditRange::new(leaf_start, middle_end).unwrap();
    let manifest = session
        .manifest_for_edits(vec![overlapping])
        .expect("manifest itself is syntactically valid");
    assert!(matches!(
        session.derive_edit_resolution(&changed, &profile, &manifest),
        Err(SemanticSessionError::EditRangeAmbiguous { .. })
    ));

    let mut session = SemanticSession::start(&base, profile.clone(), 16).expect("valid session");
    let unrelated_only = session
        .manifest_for_edits(vec![
            SemanticEditRange::new(middle_start, middle_end).unwrap()
        ])
        .expect("valid unrelated manifest");
    assert!(matches!(
        session.derive_edit_resolution(&changed, &profile, &unrelated_only),
        Err(SemanticSessionError::SemanticChangeOutsideManifest { .. })
    ));

    assert!(matches!(
        SemanticEditRange::new(10, 9),
        Err(SemanticEditManifestError::InvalidRange { .. })
    ));
}

#[test]
fn manifest_ranges_reject_overlap_before_session_use() {
    let first = SemanticEditRange::new(0, 4).unwrap();
    let second = SemanticEditRange::new(3, 8).unwrap();
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let base = parse_ueg(&fixture("value + 1"));
    let session = SemanticSession::start(&base, profile, 16).expect("valid session");
    let fingerprint = session.current_fingerprint().expect("fingerprint");
    assert!(matches!(
        un1c0::semantic_session::SemanticEditManifest::new(
            fingerprint.root_key(),
            fingerprint.profile_key(),
            vec![first, second],
        ),
        Err(SemanticEditManifestError::OverlappingRanges { .. })
    ));
}
