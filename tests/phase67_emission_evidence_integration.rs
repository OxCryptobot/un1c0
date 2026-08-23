use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_evidence::EmissionEvidenceBundle;
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
    let unit = unit();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
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
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(unit, changed)]);
    (snapshot, profile, candidates)
}

fn bundle() -> (
    EmissionEvidenceBundle,
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
    let bundle = EmissionEvidenceBundle::from_receipts(&[receipt.clone(), receipt]).unwrap();
    (bundle, snapshot, profile, candidates)
}

#[test]
fn bundle_verifies_exact_current_state_and_preserves_observation_count() {
    let (bundle, snapshot, profile, candidates) = bundle();
    assert_eq!(bundle.aggregate().observations(), 2);
    assert!(bundle.evidence_digest() != [0; 32]);
    bundle.verify_for(&snapshot, &profile, &candidates).unwrap();
}

#[test]
fn divergent_receipts_are_rejected_before_bundle_creation() {
    let (snapshot, profile, candidates) = prepared();
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (first, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    let changed_candidates = BTreeMap::from([(unit(), parse(&source("value + 3")))]);
    let second = emitter.emit_with_receipt(&snapshot, 1, &profile, &changed_candidates, |_, _| {
        Ok::<(), &'static str>(())
    });
    assert!(second.is_err());
    assert!(EmissionEvidenceBundle::from_receipts(&[first.clone(), first]).is_ok());
}

#[test]
fn bundle_rejects_stale_candidate_state() {
    let (bundle, snapshot, profile, _) = bundle();
    let stale = BTreeMap::from([(unit(), parse(&source("value + 9")))]);
    assert!(bundle.verify_for(&snapshot, &profile, &stale).is_err());
}
