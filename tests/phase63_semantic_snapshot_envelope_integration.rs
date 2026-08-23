use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::{SemanticSnapshotEnvelope, SemanticSnapshotEnvelopeError};
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

fn unit(value: &str) -> SemanticUnitId {
    SemanticUnitId::new(value).expect("valid unit")
}

fn leaf_range(ueg: &Ueg) -> SemanticEditRange {
    let NodeKind::Lambda(lambda) = &ueg.nodes[0];
    SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap()
}

fn prepared_session() -> (
    SemanticBatchSession,
    TargetCapabilityProfile,
    SemanticUnitId,
    Ueg,
) {
    let base = parse(&fixture("value + 1"));
    let changed = parse(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let id = unit("workspace/unit.ueg");
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session
        .manifest_for(&id, vec![leaf_range(&base)])
        .expect("manifest");
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: id.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&envelope, &profile).unwrap();
    (session, profile, id, changed)
}

#[test]
fn captures_and_verifies_exact_multi_unit_snapshot_state() {
    let (session, profile, id, changed) = prepared_session();
    let envelope = SemanticSnapshotEnvelope::capture(&session, 1).expect("capture envelope");
    assert_eq!(envelope.batch_id(), 1);
    assert_eq!(envelope.units().len(), 1);

    let candidates = BTreeMap::from([(id, changed)]);
    envelope
        .verify_for(1, &profile, &candidates)
        .expect("exact candidate state");
}

#[test]
fn verification_rejects_batch_identity_unit_set_and_root_drift() {
    let (session, profile, id, changed) = prepared_session();
    let envelope = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();

    let exact = BTreeMap::from([(id.clone(), changed.clone())]);
    assert!(matches!(
        envelope.verify_for(2, &profile, &exact),
        Err(SemanticSnapshotEnvelopeError::BatchIdMismatch {
            expected: 1,
            actual: 2
        })
    ));
    assert!(matches!(
        envelope.verify_for(1, &profile, &BTreeMap::new()),
        Err(SemanticSnapshotEnvelopeError::EmptyUnitSet)
    ));

    let unexpected = BTreeMap::from([
        (id.clone(), changed.clone()),
        (unit("unexpected.ueg"), parse(&fixture("value + 3"))),
    ]);
    assert!(matches!(
        envelope.verify_for(1, &profile, &unexpected),
        Err(SemanticSnapshotEnvelopeError::UnexpectedUnit(_))
    ));

    let drifted = BTreeMap::from([(id, parse(&fixture("value + 99")))]);
    assert!(matches!(
        envelope.verify_for(1, &profile, &drifted),
        Err(SemanticSnapshotEnvelopeError::UegChanged { .. })
    ));
}

#[test]
fn capture_rejects_unapplied_or_invalidated_batch_state() {
    let base = parse(&fixture("value + 1"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let id = unit("unit.ueg");
    let mut session = SemanticBatchSession::start(
        profile,
        vec![SemanticUnitStart {
            unit: id,
            ueg: base,
            capacity: 8,
        }],
    )
    .unwrap();
    assert!(matches!(
        SemanticSnapshotEnvelope::capture(&session, 1),
        Err(SemanticSnapshotEnvelopeError::BatchNotApplied {
            batch_id: 1,
            next_batch_id: 1
        })
    ));
    session.invalidate();
    assert!(matches!(
        SemanticSnapshotEnvelope::capture(&session, 1),
        Err(SemanticSnapshotEnvelopeError::SessionInvalidated)
    ));
    assert!(matches!(
        SemanticSnapshotEnvelope::capture(&session, 0),
        Err(SemanticSnapshotEnvelopeError::InvalidBatchId)
    ));
}
