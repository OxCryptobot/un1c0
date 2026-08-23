use std::collections::BTreeMap;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::snapshot_emission::{SnapshotBoundBatchEmitter, SnapshotEmissionError};
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
fn emits_only_after_exact_snapshot_verification() {
    let (snapshot, profile, candidates) = prepared();
    let emitter = SnapshotBoundBatchEmitter::new(TargetBinding::Rust);
    let mut names = Vec::new();
    let stats = emitter
        .emit(&snapshot, 1, &profile, &candidates, |unit, chunk| {
            names.push((unit.clone(), chunk.function_name));
            Ok::<(), &'static str>(())
        })
        .expect("snapshot-bound emission");
    assert_eq!(stats.units_emitted, 1);
    assert_eq!(stats.chunks_emitted, 2);
    assert!(stats.bytes_emitted > 0);
    assert_eq!(names.len(), 2);
}

#[test]
fn stale_candidate_is_rejected_before_sink_invocation() {
    let (snapshot, profile, mut candidates) = prepared();
    let unit = candidates.keys().next().unwrap().clone();
    candidates.insert(unit, parse(&fixture("value + 99")));
    let emitter = SnapshotBoundBatchEmitter::new(TargetBinding::Rust);
    let mut sink_calls = 0;
    let result = emitter.emit(&snapshot, 1, &profile, &candidates, |_, _| {
        sink_calls += 1;
        Ok::<(), &'static str>(())
    });
    assert!(matches!(result, Err(SnapshotEmissionError::Envelope(_))));
    assert_eq!(sink_calls, 0);
}

#[test]
fn target_mismatch_and_sink_failures_are_typed() {
    let (snapshot, profile, candidates) = prepared();
    let zig_emitter = SnapshotBoundBatchEmitter::new(TargetBinding::Zig);
    assert!(matches!(
        zig_emitter.emit(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        }),
        Err(SnapshotEmissionError::TargetMismatch {
            expected: TargetBinding::Zig,
            actual: TargetBinding::Rust
        })
    ));

    let emitter = SnapshotBoundBatchEmitter::new(TargetBinding::Rust);
    let result = emitter.emit(&snapshot, 1, &profile, &candidates, |_, _| {
        Err::<(), _>("sink unavailable")
    });
    assert!(matches!(result, Err(SnapshotEmissionError::Unit { .. })));
}
