use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::emission_receipt_aggregate::{EmissionReceiptAggregate, ReceiptAggregateError};
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

fn fixture(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn id(value: &str) -> SemanticUnitId {
    SemanticUnitId::new(value).expect("valid unit identity")
}

fn leaf_range(ueg: &Ueg) -> SemanticEditRange {
    let NodeKind::Lambda(lambda) = &ueg.nodes[0];
    SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap()
}

fn prepared(
    target: TargetBinding,
    body: &str,
) -> (
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let profile = TargetCapabilityProfile::for_target(target);
    let unit = id("workspace/unit.ueg");
    let base = parse(&fixture("value + 1"));
    let changed = parse(&fixture(body));
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session
        .manifest_for(&unit, vec![leaf_range(&base)])
        .unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: unit.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session
        .refresh_envelope(&batch_envelope, &profile)
        .expect("apply semantic batch");
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(unit, changed)]);
    (snapshot, profile, candidates)
}

fn receipt(
    target: TargetBinding,
    body: &str,
) -> (
    un1c0::EmissionReceipt,
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let (snapshot, profile, candidates) = prepared(target, body);
    let emitter = ReceiptBoundBatchEmitter::new(target);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .expect("receipt-bound emission");
    (receipt, snapshot, profile, candidates)
}

#[test]
fn aggregates_equivalent_receipts_and_verifies_current_state() {
    let (receipt, snapshot, profile, candidates) = receipt(TargetBinding::Rust, "value + 2");
    let aggregate = EmissionReceiptAggregate::from_receipts(&[receipt.clone(), receipt]).unwrap();
    assert_eq!(aggregate.observations(), 2);
    assert_eq!(aggregate.chunks_emitted(), 2);
    assert_eq!(aggregate.bytes_emitted() > 0, true);
    aggregate
        .verify_for(&snapshot, &profile, &candidates)
        .expect("aggregate verification");
}

#[test]
fn rejects_empty_and_divergent_observations() {
    assert!(matches!(
        EmissionReceiptAggregate::from_receipts(&[]),
        Err(ReceiptAggregateError::Empty)
    ));
    let (first, _, _, _) = receipt(TargetBinding::Rust, "value + 2");
    let (second, _, _, _) = receipt(TargetBinding::Rust, "value + 3");
    assert!(matches!(
        EmissionReceiptAggregate::from_receipts(&[first, second]),
        Err(ReceiptAggregateError::UnitRootsMismatch)
    ));
}

#[test]
fn rejects_target_divergence_before_aggregate_creation() {
    let (rust, _, _, _) = receipt(TargetBinding::Rust, "value + 2");
    let (zig, _, _, _) = receipt(TargetBinding::Zig, "value + 2");
    assert!(matches!(
        EmissionReceiptAggregate::from_receipts(&[rust, zig]),
        Err(ReceiptAggregateError::TargetMismatch {
            expected: TargetBinding::Rust,
            actual: TargetBinding::Zig
        })
    ));
}
