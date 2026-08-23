use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_comparison::{
    EmissionDiagnosticComparison, EmissionDiagnosticComparisonError,
};
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn source(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn unit() -> SemanticUnitId {
    SemanticUnitId::new("workspace/unit.ueg").expect("valid unit")
}

fn leaf_range(ueg: &Ueg) -> SemanticEditRange {
    let NodeKind::Lambda(lambda) = &ueg.nodes[0];
    SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap()
}

fn prepared() -> (
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let id = unit();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session.manifest_for(&id, vec![leaf_range(&base)]).unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: id.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    (snapshot, profile, BTreeMap::from([(id, changed)]))
}

fn reports() -> (
    EmissionDiagnosticReport,
    EmissionDiagnosticReport,
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let (snapshot, profile, candidates) = prepared();
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    let before = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    let after = EmissionDiagnosticReport::from_receipts(
        &[receipt.clone(), receipt.clone(), receipt.clone(), receipt],
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    (before, after, snapshot, profile, candidates)
}

#[test]
fn comparison_reverifies_both_reports_and_returns_typed_deltas() {
    let (before, after, snapshot, profile, candidates) = reports();
    let comparison =
        EmissionDiagnosticComparison::compare(&before, &after, &snapshot, &profile, &candidates)
            .unwrap();
    let delta = comparison.delta();
    assert_eq!(delta.observation_delta(), 3);
    assert_eq!(delta.chunk_delta(), 0);
    assert_eq!(delta.byte_delta(), 0);
    assert!(delta.digest_equal());
}

#[test]
fn comparison_rejects_stale_candidates_before_computing_deltas() {
    let (before, after, snapshot, profile, _) = reports();
    let stale = BTreeMap::from([(unit(), parse(&source("value + 99")))]);
    let error = EmissionDiagnosticComparison::compare(&before, &after, &snapshot, &profile, &stale)
        .unwrap_err();
    assert!(matches!(
        error,
        EmissionDiagnosticComparisonError::Before(_)
    ));
}

#[test]
fn comparison_rejects_profile_drift_without_partial_result() {
    let (before, after, snapshot, _, candidates) = reports();
    let zig = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let error =
        EmissionDiagnosticComparison::compare(&before, &after, &snapshot, &zig, &candidates)
            .unwrap_err();
    assert!(matches!(
        error,
        EmissionDiagnosticComparisonError::Before(_)
    ));
}
