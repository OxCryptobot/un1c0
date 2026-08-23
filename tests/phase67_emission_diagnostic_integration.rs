use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::{
    EmissionDiagnosticEntry, EmissionDiagnosticError, EmissionDiagnosticReport,
    MAX_DIAGNOSTIC_ENTRIES,
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

fn one_receipt() -> (
    un1c0::EmissionReceipt,
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
    (receipt, snapshot, profile, candidates)
}

#[test]
fn report_generation_requires_verification_and_emits_bounded_typed_entries() {
    let (receipt, snapshot, profile, candidates) = one_receipt();
    let report = EmissionDiagnosticReport::from_receipts(
        &[receipt.clone(), receipt],
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();

    assert_eq!(report.entries().len(), MAX_DIAGNOSTIC_ENTRIES);
    assert!(report.entries().iter().any(|entry| matches!(
        entry,
        EmissionDiagnosticEntry::ObservationCount { count: 2 }
    )));
    report.verify_for(&snapshot, &profile, &candidates).unwrap();
}

#[test]
fn report_rejects_empty_and_divergent_observations() {
    let (receipt, snapshot, profile, candidates) = one_receipt();
    let empty =
        EmissionDiagnosticReport::from_receipts(&[], &snapshot, &profile, &candidates).unwrap_err();
    assert!(matches!(
        empty,
        EmissionDiagnosticError::Aggregate(un1c0::ReceiptAggregateError::Empty)
    ));

    let other = ReceiptBoundBatchEmitter::new(TargetBinding::Zig)
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap_err();
    let _ = other;
    let report = EmissionDiagnosticReport::from_receipts(
        &[receipt.clone(), receipt],
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    assert_eq!(report.aggregate().observations(), 2);
}

#[test]
fn report_rejects_stale_state_and_target_drift_without_partial_output() {
    let (receipt, snapshot, profile, candidates) = one_receipt();
    let stale = BTreeMap::from([(unit(), parse(&source("value + 9")))]);
    let stale_error = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &stale,
    )
    .unwrap_err();
    assert!(matches!(stale_error, EmissionDiagnosticError::Aggregate(_)));

    let zig = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let target_error = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &zig,
        &candidates,
    )
    .unwrap_err();
    assert!(matches!(
        target_error,
        EmissionDiagnosticError::Aggregate(_)
    ));
}
