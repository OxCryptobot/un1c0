use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_receipt::{EmissionReceiptError, ReceiptBoundBatchEmitter};
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

fn prepared() -> (
    SemanticSnapshotEnvelope,
    TargetCapabilityProfile,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit = id("workspace/unit.ueg");
    let base = parse(&fixture("value + 1"));
    let changed = parse(&fixture("value + 2"));
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

#[test]
fn receipt_binds_exact_emission_state_and_stats() {
    let (snapshot, profile, candidates) = prepared();
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, stats) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .expect("receipt-bound emission");
    assert_eq!(stats.units_emitted, 1);
    assert_eq!(stats.chunks_emitted, 2);
    assert!(stats.bytes_emitted > 0);
    assert_eq!(receipt.target(), TargetBinding::Rust);
    assert_eq!(receipt.batch_id(), 1);
    assert_eq!(receipt.chunks_emitted(), 2);
    assert_eq!(receipt.bytes_emitted(), stats.bytes_emitted);
    assert_ne!(receipt.output_digest(), [0; 32]);
    receipt
        .verify_for(&snapshot, 1, &profile, &candidates)
        .expect("receipt verification");
}

#[test]
fn receipt_verification_rejects_batch_or_target_mismatch() {
    let (snapshot, profile, candidates) = prepared();
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    assert!(matches!(
        receipt.verify_for(&snapshot, 2, &profile, &candidates),
        Err(EmissionReceiptError::ReceiptBatchMismatch {
            expected: 1,
            actual: 2
        })
    ));
    let zig_profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    assert!(matches!(
        receipt.verify_for(&snapshot, 1, &zig_profile, &candidates),
        Err(EmissionReceiptError::ReceiptTargetMismatch {
            expected: TargetBinding::Zig,
            actual: TargetBinding::Rust
        })
    ));
}

#[test]
fn sink_failure_returns_no_receipt_and_retains_unit_context() {
    let (snapshot, profile, candidates) = prepared();
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let result = emitter.emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
        Err::<(), _>("sink unavailable")
    });
    assert!(matches!(result, Err(EmissionReceiptError::Unit { .. })));
}
