use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchError, SemanticBatchSession, SemanticEditBatch,
    SemanticEditUpdate, SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
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

fn one_update(
    session: &SemanticBatchSession,
    id: &SemanticUnitId,
    changed: Ueg,
    range: SemanticEditRange,
) -> SemanticEditUpdate {
    SemanticEditUpdate {
        unit: id.clone(),
        ueg: changed,
        manifest: session.manifest_for(id, vec![range]).unwrap(),
    }
}

#[test]
fn envelope_accepts_next_id_and_advances_only_after_success() {
    let base = parse(&fixture("value + 1"));
    let changed = parse(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let id = unit("unit.ueg");
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let update = one_update(&session, &id, changed, leaf_range(&base));
    let batch = SemanticEditBatch::new(vec![update]).unwrap();
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();

    session
        .refresh_envelope(&envelope, &profile)
        .expect("first envelope");
    assert_eq!(session.next_batch_id(), 2);
    assert!(session.is_valid());
}

#[test]
fn replay_and_sequence_gap_invalidate_the_batch_session() {
    let base = parse(&fixture("value + 1"));
    let changed = parse(&fixture("value + 2"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    let id = unit("unit.ueg");
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let update = one_update(&session, &id, changed.clone(), leaf_range(&base));
    let batch = SemanticEditBatch::new(vec![update]).unwrap();
    let first = SemanticBatchEnvelope::new(1, session.profile_key(), batch.clone()).unwrap();
    session
        .refresh_envelope(&first, &profile)
        .expect("first envelope");
    let replay = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    assert!(matches!(
        session.refresh_envelope(&replay, &profile),
        Err(SemanticBatchError::BatchSequenceMismatch { .. })
    ));
    assert!(!session.is_valid());

    let mut fresh = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let gap_update = one_update(&fresh, &id, changed, leaf_range(&base));
    let gap_batch = SemanticEditBatch::new(vec![gap_update]).unwrap();
    let gap = SemanticBatchEnvelope::new(2, fresh.profile_key(), gap_batch).unwrap();
    assert!(matches!(
        fresh.refresh_envelope(&gap, &profile),
        Err(SemanticBatchError::BatchSequenceMismatch {
            expected: 1,
            actual: 2
        })
    ));
    assert!(!fresh.is_valid());
}

#[test]
fn profile_key_mismatch_and_zero_id_fail_closed() {
    let base = parse(&fixture("value + 1"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Zig);
    let other_profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let id = unit("unit.ueg");
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: base.clone(),
            capacity: 8,
        }],
    )
    .unwrap();
    let update = one_update(
        &session,
        &id,
        base,
        leaf_range(&parse(&fixture("value + 1"))),
    );
    let batch = SemanticEditBatch::new(vec![update]).unwrap();
    assert!(matches!(
        SemanticBatchEnvelope::new(0, session.profile_key(), batch.clone()),
        Err(SemanticBatchError::InvalidBatchId)
    ));
    let foreign = SemanticBatchSession::start(
        other_profile,
        vec![SemanticUnitStart {
            unit: id.clone(),
            ueg: parse(&fixture("value + 1")),
            capacity: 8,
        }],
    )
    .unwrap();
    let envelope = SemanticBatchEnvelope::new(1, foreign.profile_key(), batch).unwrap();
    assert!(matches!(
        session.refresh_envelope(&envelope, &profile),
        Err(SemanticBatchError::BatchProfileMismatch)
    ));
    assert!(!session.is_valid());
}
