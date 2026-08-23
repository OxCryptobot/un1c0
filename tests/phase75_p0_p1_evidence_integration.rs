use std::collections::BTreeMap;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    CanonicalDiagnosticEvidence, DiagnosticAttestationKey, DiagnosticAttestationVerifier,
    EmissionDiagnosticAttestationError, VerifiedDiagnosticEvidence,
};
use un1c0::emission_diagnostic_instrumentation::{
    redacted_numeric_fields, DiagnosticInstrumentation, DiagnosticTelemetryCollectError,
    DiagnosticTelemetryCollector, DiagnosticTelemetryError, VerificationOutcome,
};
use un1c0::emission_diagnostic_journal::{DiagnosticJournalError, DiagnosticObservationJournal};
use un1c0::emission_diagnostic_network::{
    EmissionDiagnosticNetworkError, MultiNodeDiagnosticReceiver,
};
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

struct Fixture {
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn prepared() -> Fixture {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit = SemanticUnitId::new("workspace/unit.ueg").unwrap();
    let base = parse("def leaf(value: int) -> int:\n    return value + 1\n");
    let changed = parse("def leaf(value: int) -> int:\n    return value + 2\n");
    let NodeKind::Lambda(lambda) = &base.nodes[0];
    let range =
        SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte).unwrap();
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit.clone(),
            ueg: base,
            capacity: 8,
        }],
    )
    .unwrap();
    let manifest = session.manifest_for(&unit, vec![range]).unwrap();
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: unit.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .unwrap();
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    Fixture {
        snapshot,
        profile,
        candidates: BTreeMap::from([(unit, changed)]),
    }
}

fn stream(fixture: &Fixture, frame_count: usize) -> EmissionDiagnosticStream {
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(
            &fixture.snapshot,
            1,
            &fixture.profile,
            &fixture.candidates,
            |_, _| Ok::<(), &'static str>(()),
        )
        .unwrap();
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    EmissionDiagnosticStream::from_repeated_report(
        75,
        &report,
        frame_count,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn evidence(
    fixture: &Fixture,
    frame_count: usize,
    verifier: &DiagnosticAttestationVerifier,
    instrumentation: &DiagnosticInstrumentation,
) -> (VerifiedDiagnosticEvidence, EmissionDiagnosticStream) {
    let stream = stream(fixture, frame_count);
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(17));
    let attestation = key
        .attest_stream(
            1,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::from([("environment".into(), "test".into())]),
        )
        .unwrap();
    let evidence = verifier
        .verify_stream_evidence(
            &attestation,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            instrumentation,
        )
        .unwrap();
    (evidence, stream)
}

fn trusted_verifier() -> DiagnosticAttestationVerifier {
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(17));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();
    verifier
}

#[test]
fn p0_instrumentation_emits_redacted_stage_sample() {
    let fixture = prepared();
    let verifier = trusted_verifier();
    let instrumentation = DiagnosticInstrumentation::enabled(4);
    let (evidence, _) = evidence(&fixture, 4, &verifier, &instrumentation);
    let snapshot = instrumentation.snapshot();

    assert!(snapshot.enabled);
    assert_eq!(snapshot.completed_operations, 1);
    assert_eq!(snapshot.accepted_operations, 1);
    assert_eq!(snapshot.rejected_operations, 0);
    assert_eq!(snapshot.samples.len(), 1);
    let sample = &snapshot.samples[0];
    assert_eq!(sample.outcome, VerificationOutcome::Accepted);
    assert_eq!(sample.frame_count, 4);
    assert_eq!(sample.counters.signature_verifications, 1);
    assert_eq!(sample.counters.public_key_parses, 0);
    assert!(sample.stages.snapshot_fingerprint_ns > 0);
    assert!(sample.stages.nested_report_verify_ns > 0);
    assert_eq!(sample.stages.canonical_report_serialize_ns, 0);
    assert!(sample.stages.canonical_stream_serialize_ns > 0);
    assert!(sample.stages.canonical_bytes_reuse_ns > 0);
    assert!(sample.stages.content_hash_ns > 0);
    assert!(sample.stages.signing_payload_serialize_ns > 0);
    assert!(sample.stages.trust_lookup_ns > 0);
    assert!(sample.stages.ed25519_verify_ns > 0);
    assert!(!sample.contains_sensitive_material());
    assert_eq!(redacted_numeric_fields(sample).len(), 4);
    assert_eq!(evidence.canonical().frame_count(), 4);
}

#[test]
fn disabled_instrumentation_preserves_verification_without_samples() {
    let fixture = prepared();
    let verifier = trusted_verifier();
    let instrumentation = DiagnosticInstrumentation::disabled();
    let _ = evidence(&fixture, 1, &verifier, &instrumentation);
    let snapshot = instrumentation.snapshot();

    assert!(!snapshot.enabled);
    assert_eq!(snapshot.completed_operations, 0);
    assert!(snapshot.samples.is_empty());
}

#[test]
fn immutable_evidence_is_current_state_bound_and_aggregates_once() {
    let fixture = prepared();
    let verifier = trusted_verifier();
    let instrumentation = DiagnosticInstrumentation::disabled();
    let (evidence, stream) = evidence(&fixture, 2, &verifier, &instrumentation);
    let canonical = evidence.canonical();

    assert_eq!(canonical.stream(), &stream);
    assert_eq!(
        canonical.canonical_stream_bytes(),
        stream.to_json().unwrap()
    );
    assert!(canonical.matches_context(&fixture.snapshot, &fixture.profile));
    assert!(canonical.matches_current_candidates(
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates
    ));

    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver
        .register_node(7, Arc::new(verifier.clone()))
        .unwrap();
    receiver
        .ingest_verified(
            7,
            9,
            1,
            evidence.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 1);
    assert!(matches!(
        receiver.ingest_verified(
            7,
            9,
            1,
            evidence,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticNetworkError::Transport(
            un1c0::EmissionDiagnosticTransportError::Replay { .. }
        ))
    ));
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 1);
}

#[test]
fn stale_candidates_are_rejected_before_verified_aggregation_mutation() {
    let fixture = prepared();
    let verifier = trusted_verifier();
    let instrumentation = DiagnosticInstrumentation::disabled();
    let (evidence, _) = evidence(&fixture, 1, &verifier, &instrumentation);
    let stale_unit = fixture.candidates.keys().next().unwrap().clone();
    let stale = BTreeMap::from([(
        stale_unit,
        parse("def leaf(value: int) -> int:\n    return value + 99\n"),
    )]);

    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver.register_node(7, Arc::new(verifier)).unwrap();
    assert!(matches!(
        receiver.ingest_verified(
            7,
            9,
            1,
            evidence,
            &fixture.snapshot,
            &fixture.profile,
            &stale,
        ),
        Err(EmissionDiagnosticNetworkError::Attestation(
            EmissionDiagnosticAttestationError::ContentMismatch
        ))
    ));
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 0);
}

#[test]
fn trust_epoch_revocation_invalidates_previously_verified_evidence() {
    let fixture = prepared();
    let key = DiagnosticAttestationKey::from_signing_key(signing_key(17));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key.public_key()).unwrap();
    let instrumentation = DiagnosticInstrumentation::disabled();
    let (evidence, _) = evidence(&fixture, 1, &verifier, &instrumentation);
    let old_epoch = evidence.trust_epoch();
    assert!(verifier.revoke_public_key(&key.public_key()));
    assert!(verifier.trust_epoch() > old_epoch);

    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver.register_node(7, Arc::new(verifier)).unwrap();
    assert!(matches!(
        receiver.ingest_verified(
            7,
            9,
            1,
            evidence,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        ),
        Err(EmissionDiagnosticNetworkError::Attestation(
            EmissionDiagnosticAttestationError::TrustEpochMismatch { .. }
        ))
    ));
}

#[test]
fn invalid_public_keys_fail_at_registration_without_partial_trust_state() {
    let mut verifier = DiagnosticAttestationVerifier::new();
    assert!(matches!(
        verifier.register_public_key([2; 32]),
        Err(EmissionDiagnosticAttestationError::InvalidPublicKey)
    ));
    assert_eq!(verifier.trusted_key_count(), 0);
    assert_eq!(verifier.trust_epoch(), 0);
}

#[test]
fn canonical_evidence_constructor_is_fallible_for_stale_stream_context() {
    let fixture = prepared();
    let stream = stream(&fixture, 1);
    let other_profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let instrumentation = DiagnosticInstrumentation::disabled();
    assert!(matches!(
        CanonicalDiagnosticEvidence::from_stream(
            &stream,
            &fixture.snapshot,
            &other_profile,
            &fixture.candidates,
            &instrumentation,
        ),
        Err(EmissionDiagnosticAttestationError::Stream(_))
    ));
}

#[test]
fn phase78_reuses_immutable_canonical_bytes_across_attestation_and_evidence() {
    let fixture = prepared();
    let stream = stream(&fixture, 4);
    let canonical_json = stream.canonical_json_bytes().to_vec();
    let canonical_payload = stream.canonical_payload_bytes().unwrap();
    assert_eq!(stream.to_json().unwrap(), canonical_json);
    assert_eq!(
        EmissionDiagnosticStream::canonical_payload_digest(&canonical_payload),
        stream.stream_digest()
    );

    let restored = EmissionDiagnosticStream::from_json_for(
        &canonical_json,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    assert_eq!(restored.canonical_json_bytes(), canonical_json.as_slice());
    assert_eq!(restored.to_json().unwrap(), canonical_json);

    let key = DiagnosticAttestationKey::from_signing_key(signing_key(17));
    let attestation = key
        .attest_stream(
            77,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::from([("environment".into(), "phase78".into())]),
        )
        .unwrap();
    let instrumentation = DiagnosticInstrumentation::enabled(2);
    let evidence = trusted_verifier()
        .verify_stream_evidence(
            &attestation,
            &stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &instrumentation,
        )
        .unwrap();
    assert_eq!(
        evidence.canonical().canonical_stream_bytes(),
        canonical_json.as_slice()
    );
    assert_eq!(
        evidence.canonical().content_hash(),
        attestation.content_hash()
    );
    let sample = &instrumentation.snapshot().samples[0];
    assert_eq!(sample.stages.canonical_report_serialize_ns, 0);
    assert!(sample.stages.canonical_bytes_reuse_ns > 0);
}

#[test]
fn f78_3_collector_queue_overflow_is_non_authoritative() {
    assert!(matches!(
        DiagnosticTelemetryCollector::new(0),
        Err(DiagnosticTelemetryCollectError::InvalidCapacity)
    ));
    let fixture = prepared();
    let verifier = trusted_verifier();
    let (verified, _) = evidence(
        &fixture,
        2,
        &verifier,
        &DiagnosticInstrumentation::disabled(),
    );
    let collector = DiagnosticTelemetryCollector::new(1).unwrap();
    let instrumentation = DiagnosticInstrumentation::disabled();
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver.register_node(7, Arc::new(verifier)).unwrap();

    receiver
        .ingest_verified_with_telemetry(
            7,
            11,
            1,
            verified.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &instrumentation,
            &collector,
        )
        .unwrap();
    receiver
        .ingest_verified_with_telemetry(
            7,
            11,
            2,
            verified,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &instrumentation,
            &collector,
        )
        .unwrap();
    assert_eq!(collector.len(), 1);
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 1);
    assert_eq!(receiver.aggregator(7).unwrap().total_frames(), 4);
    assert!(matches!(
        collector.collect(&instrumentation.snapshot()),
        Err(DiagnosticTelemetryCollectError::QueueFull {
            entries: 1,
            maximum: 1
        })
    ));

    let mut malformed_snapshot = instrumentation.snapshot();
    malformed_snapshot.samples =
        vec![malformed_snapshot
            .samples
            .first()
            .cloned()
            .unwrap_or_else(|| {
                let fresh = DiagnosticInstrumentation::enabled(1);
                fresh.recorder(1, 1).finish(VerificationOutcome::Accepted);
                fresh.snapshot().samples[0].clone()
            })];
    malformed_snapshot.samples[0].frame_count = 0;
    assert!(matches!(
        collector.collect(&malformed_snapshot),
        Err(DiagnosticTelemetryCollectError::Schema(
            DiagnosticTelemetryError::InvalidFrameCount { .. }
        ))
    ));
    assert_eq!(collector.len(), 1);
}

#[test]
fn f78_4_journal_is_ordered_before_authorized_aggregate_mutation() {
    let fixture = prepared();
    let verifier = trusted_verifier();
    let (verified, _) = evidence(
        &fixture,
        1,
        &verifier,
        &DiagnosticInstrumentation::disabled(),
    );
    let journal = DiagnosticObservationJournal::new(2).unwrap();
    let mut receiver = MultiNodeDiagnosticReceiver::new().with_journal(journal);
    receiver.register_node(7, Arc::new(verifier)).unwrap();

    receiver
        .ingest_verified(
            7,
            11,
            1,
            verified.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    receiver
        .ingest_verified(
            7,
            11,
            2,
            verified.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    let journal = receiver.journal().unwrap();
    assert_eq!(journal.len(), 2);
    assert!(journal.verify_integrity());
    assert_eq!(journal.entries()[0].sequence(), 1);
    assert_eq!(journal.entries()[1].sequence(), 2);
    assert_eq!(journal.entries()[0].node_id(), 7);
    assert_eq!(journal.entries()[1].connection_id(), 11);
    assert_eq!(journal.entries()[0].source_sequence(), 1);
    assert_eq!(journal.entries()[1].source_sequence(), 2);
    assert_eq!(journal.entries()[0].previous_digest(), [0; 32]);
    assert_eq!(
        journal.entries()[1].previous_digest(),
        journal.entries()[0].event_digest()
    );
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 1);
    assert_eq!(receiver.aggregator(7).unwrap().total_frames(), 2);

    let before = receiver.aggregator(7).unwrap().summary();
    let error = receiver
        .ingest_verified(
            7,
            11,
            3,
            verified.clone(),
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        EmissionDiagnosticNetworkError::Journal(DiagnosticJournalError::Full { .. })
    ));
    assert_eq!(receiver.journal().unwrap().len(), 2);
    assert_eq!(receiver.aggregator(7).unwrap().summary(), before);

    let other_profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let rejected = receiver
        .ingest_verified(
            7,
            11,
            3,
            verified.clone(),
            &fixture.snapshot,
            &other_profile,
            &fixture.candidates,
        )
        .unwrap_err();
    assert!(matches!(
        rejected,
        EmissionDiagnosticNetworkError::Attestation(
            EmissionDiagnosticAttestationError::ContentMismatch
        )
    ));
    assert_eq!(receiver.journal().unwrap().len(), 2);
    assert_eq!(receiver.aggregator(7).unwrap().summary(), before);
}
