use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};

use ed25519_dalek::SigningKey;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
    EmissionDiagnosticAttestationError,
};
use un1c0::emission_diagnostic_cache::{
    DiagnosticEvidenceCache, DiagnosticEvidenceCacheConfigError,
};
use un1c0::emission_diagnostic_instrumentation::DiagnosticInstrumentation;
use un1c0::emission_diagnostic_network::{
    EmissionDiagnosticNetworkError, MultiNodeDiagnosticReceiver,
};
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_workers::{
    DiagnosticVerificationJob, DiagnosticVerificationWorkerError, DiagnosticVerificationWorkerPool,
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

fn fixture() -> Fixture {
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
    Fixture {
        snapshot: SemanticSnapshotEnvelope::capture(&session, 1).unwrap(),
        profile,
        candidates: BTreeMap::from([(unit, changed)]),
    }
}

fn stream(fixture: &Fixture, stream_id: u64, frame_count: usize) -> EmissionDiagnosticStream {
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
        stream_id,
        &report,
        frame_count,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap()
}

fn key() -> DiagnosticAttestationKey {
    DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[17; 32]))
}

fn attestation(
    key: &DiagnosticAttestationKey,
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
    metadata_value: &str,
) -> EmissionDiagnosticAttestation {
    key.attest_stream(
        stream.stream_id(),
        stream,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
        BTreeMap::from([("environment".into(), metadata_value.into())]),
    )
    .unwrap()
}

fn verifier() -> DiagnosticAttestationVerifier {
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier.register_public_key(key().public_key()).unwrap();
    verifier
}

fn job(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
    attestation: &EmissionDiagnosticAttestation,
    verifier: &Arc<DiagnosticAttestationVerifier>,
    cache: &DiagnosticEvidenceCache,
) -> DiagnosticVerificationJob {
    DiagnosticVerificationJob {
        node_id: 7,
        connection_id: 9,
        sequence: 1,
        attestation: attestation.clone(),
        stream: stream.clone(),
        envelope: fixture.snapshot.clone(),
        profile: fixture.profile.clone(),
        units: fixture.candidates.clone(),
        verifier: Arc::clone(verifier),
        cache: cache.clone(),
        instrumentation: DiagnosticInstrumentation::disabled(),
    }
}

fn node_job(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
    attestation: &EmissionDiagnosticAttestation,
    verifier: &Arc<DiagnosticAttestationVerifier>,
    cache: &DiagnosticEvidenceCache,
    node_id: u64,
    sequence: u64,
) -> DiagnosticVerificationJob {
    let mut input = job(fixture, stream, attestation, verifier, cache);
    input.node_id = node_id;
    input.sequence = sequence;
    input
}

#[test]
fn cache_hits_reuse_immutable_evidence_and_emit_redacted_cache_counters() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 4);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = verifier();
    let cache = DiagnosticEvidenceCache::new(4, 64 * 1024).unwrap();
    let instrumentation = DiagnosticInstrumentation::enabled(8);

    let first = verifier
        .verify_stream_evidence_with_cache(
            &diagnostic_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &cache,
            &instrumentation,
        )
        .unwrap();
    let second = verifier
        .verify_stream_evidence_with_cache(
            &diagnostic_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &cache,
            &instrumentation,
        )
        .unwrap();

    assert_eq!(
        first.canonical().canonical_stream_bytes(),
        second.canonical().canonical_stream_bytes()
    );
    let metrics = cache.metrics();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.insertions, 1);
    assert_eq!(metrics.entries, 1);
    let snapshot = instrumentation.snapshot();
    assert_eq!(snapshot.counters.evidence_cache_misses, 1);
    assert_eq!(snapshot.counters.evidence_cache_hits, 1);
    assert_eq!(snapshot.samples.len(), 2);
    assert!(!snapshot.samples[1].contains_sensitive_material());
}

#[test]
fn trust_epoch_change_invalidates_cached_evidence_before_reuse() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let mut verifier = verifier();
    let cache = DiagnosticEvidenceCache::new(4, 64 * 1024).unwrap();
    verifier
        .verify_stream_evidence_with_cache(
            &diagnostic_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &cache,
            &DiagnosticInstrumentation::disabled(),
        )
        .unwrap();
    assert_eq!(cache.metrics().entries, 1);
    assert!(verifier.revoke_public_key(&key().public_key()));
    assert!(matches!(
        verifier.verify_stream_evidence_with_cache(
            &diagnostic_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &cache,
            &DiagnosticInstrumentation::disabled(),
        ),
        Err(EmissionDiagnosticAttestationError::UnknownPublicKey)
    ));
    let metrics = cache.metrics();
    assert_eq!(metrics.entries, 0);
    assert_eq!(metrics.invalidations, 1);
}

#[test]
fn cache_key_binds_attestation_metadata_and_full_context() {
    let fixture = fixture();
    let first_stream = stream(&fixture, 75, 1);
    let second_stream = stream(&fixture, 76, 1);
    let key = key();
    let first = attestation(&key, &fixture, &first_stream, "one");
    let different_metadata = attestation(&key, &fixture, &first_stream, "two");
    let verifier = verifier();
    let cache = DiagnosticEvidenceCache::new(4, 64 * 1024).unwrap();

    let first_key = cache
        .key_for(
            &first,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    let metadata_key = cache
        .key_for(
            &different_metadata,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    let stream_key = cache
        .key_for(
            &attestation(&key, &fixture, &second_stream, "one"),
            &second_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    assert_ne!(first_key, metadata_key);
    assert_ne!(first_key, stream_key);
}

#[test]
fn cache_enforces_entry_and_byte_bounds_with_deterministic_eviction() {
    let fixture = fixture();
    let first_stream = stream(&fixture, 75, 1);
    let second_stream = stream(&fixture, 76, 1);
    let key = key();
    let first_attestation = attestation(&key, &fixture, &first_stream, "one");
    let second_attestation = attestation(&key, &fixture, &second_stream, "two");
    let verifier = verifier();
    let first_evidence = verifier
        .verify_stream_evidence(
            &first_attestation,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &DiagnosticInstrumentation::disabled(),
        )
        .unwrap();
    let second_evidence = verifier
        .verify_stream_evidence(
            &second_attestation,
            &second_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &DiagnosticInstrumentation::disabled(),
        )
        .unwrap();
    let first_bytes = first_evidence.canonical().canonical_stream_bytes().len();
    let second_bytes = second_evidence.canonical().canonical_stream_bytes().len();
    let max_bytes = first_bytes.max(second_bytes);
    let cache = DiagnosticEvidenceCache::new(1, max_bytes).unwrap();
    let first_key = cache
        .key_for(
            &first_attestation,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    let second_key = cache
        .key_for(
            &second_attestation,
            &second_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    assert!(cache.insert(first_key, Arc::new(first_evidence)));
    assert!(cache.insert(second_key, Arc::new(second_evidence)));
    let metrics = cache.metrics();
    assert_eq!(metrics.entries, 1);
    assert_eq!(metrics.evictions, 1);
    assert_eq!(metrics.bytes, second_bytes);

    let too_small = DiagnosticEvidenceCache::new(1, first_bytes.saturating_sub(1)).unwrap();
    let key = too_small
        .key_for(
            &first_attestation,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            verifier.trust_epoch(),
        )
        .unwrap();
    let evidence = verifier
        .verify_stream_evidence(
            &first_attestation,
            &first_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &DiagnosticInstrumentation::disabled(),
        )
        .unwrap();
    assert!(!too_small.insert(key, Arc::new(evidence)));
    assert_eq!(too_small.metrics().entries, 0);
}

#[test]
fn malformed_cache_configuration_fails_closed() {
    assert_eq!(
        DiagnosticEvidenceCache::new(0, 1).unwrap_err(),
        DiagnosticEvidenceCacheConfigError::ZeroCapacity
    );
    assert!(matches!(
        DiagnosticEvidenceCache::new(1, 0),
        Err(DiagnosticEvidenceCacheConfigError::ZeroByteBudget)
    ));
}

#[test]
fn stale_candidates_fail_before_cache_reuse_or_aggregate_mutation() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = verifier();
    let cache = DiagnosticEvidenceCache::new(4, 64 * 1024).unwrap();
    let evidence = verifier
        .verify_stream_evidence_with_cache(
            &diagnostic_attestation,
            &diagnostic_stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            &cache,
            &DiagnosticInstrumentation::disabled(),
        )
        .unwrap();
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
fn bounded_workers_return_results_in_submission_order_and_close_cleanly() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = Arc::new(verifier());
    let cache = DiagnosticEvidenceCache::new(16, 256 * 1024).unwrap();
    let mut pool = DiagnosticVerificationWorkerPool::new(2, 8).unwrap();
    for _ in 0..4 {
        pool.submit(job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
        ))
        .unwrap();
    }
    let mut ids = Vec::new();
    for _ in 0..4 {
        let result = pool.next_ordered().unwrap().unwrap();
        ids.push(result.job_id);
        assert!(result.evidence.is_ok());
    }
    assert_eq!(ids, vec![1, 2, 3, 4]);
    let metrics = pool.metrics();
    assert_eq!(metrics.submitted_jobs, 4);
    assert_eq!(metrics.completed_jobs, 4);
    assert_eq!(metrics.failed_jobs, 0);
    assert_eq!(metrics.ordered_dispatches, 4);
    pool.close().unwrap();
}

#[test]
fn cancelled_worker_results_cannot_verify_or_mutate_aggregation() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = Arc::new(verifier());
    let cache = DiagnosticEvidenceCache::new(16, 256 * 1024).unwrap();
    let gate = Arc::new(Barrier::new(2));
    let mut pool = DiagnosticVerificationWorkerPool::new_with_start_gate_and_limits(
        1,
        2,
        2,
        Arc::clone(&gate),
    )
    .unwrap();
    let ticket = pool
        .submit_with_cancellation(node_job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
            7,
            1,
        ))
        .unwrap();
    ticket.cancel();
    gate.wait();
    let result = pool.next_ordered().unwrap().unwrap();
    assert!(result.is_cancelled());
    assert!(matches!(
        result.evidence,
        Err(un1c0::emission_diagnostic_workers::EmissionDiagnosticWorkerError::Cancelled)
    ));
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver.register_node(7, verifier).unwrap();
    assert_eq!(
        receiver
            .ingest_worker_result(
                result,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates
            )
            .unwrap_err(),
        EmissionDiagnosticNetworkError::VerificationCancelled
    );
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 0);
    let metrics = pool.metrics();
    assert_eq!(metrics.cancelled_jobs, 1);
    assert_eq!(metrics.failed_jobs, 0);
    pool.close().unwrap();
}

#[test]
fn per_node_fairness_rejects_hot_node_but_preserves_other_nodes() {
    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = Arc::new(verifier());
    let cache = DiagnosticEvidenceCache::new(16, 256 * 1024).unwrap();
    let gate = Arc::new(Barrier::new(2));
    let mut pool = DiagnosticVerificationWorkerPool::new_with_start_gate_and_limits(
        1,
        8,
        2,
        Arc::clone(&gate),
    )
    .unwrap();
    for sequence in 1..=2 {
        pool.submit(node_job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
            7,
            sequence,
        ))
        .unwrap();
    }
    assert!(matches!(
        pool.submit(node_job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
            7,
            3,
        )),
        Err(DiagnosticVerificationWorkerError::FairnessLimit {
            node_id: 7,
            limit: 2
        })
    ));
    for sequence in 1..=2 {
        pool.submit(node_job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
            8,
            sequence,
        ))
        .unwrap();
    }
    assert!(matches!(
        pool.submit(node_job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
            8,
            3,
        )),
        Err(DiagnosticVerificationWorkerError::FairnessLimit {
            node_id: 8,
            limit: 2
        })
    ));
    gate.wait();
    for _ in 0..4 {
        assert!(pool.next_ordered().unwrap().unwrap().evidence.is_ok());
    }
    let metrics = pool.metrics();
    assert_eq!(metrics.submitted_jobs, 4);
    assert_eq!(metrics.fairness_rejections, 2);
    assert_eq!(metrics.queue_full_rejections, 0);
    pool.close().unwrap();
}

#[test]
fn worker_queue_bounds_and_receiver_boundary_remain_fail_closed() {
    assert!(matches!(
        DiagnosticVerificationWorkerPool::new(0, 1),
        Err(DiagnosticVerificationWorkerError::ZeroWorkers)
    ));
    assert!(matches!(
        DiagnosticVerificationWorkerPool::new(1, 0),
        Err(DiagnosticVerificationWorkerError::ZeroQueueCapacity)
    ));
    assert!(matches!(
        DiagnosticVerificationWorkerPool::new_with_limits(1, 1, 0),
        Err(DiagnosticVerificationWorkerError::ZeroPerNodeLimit)
    ));
    assert!(matches!(
        DiagnosticVerificationWorkerPool::new_with_limits(
            1,
            1,
            un1c0::emission_diagnostic_workers::MAX_DIAGNOSTIC_NODE_IN_FLIGHT + 1,
        ),
        Err(DiagnosticVerificationWorkerError::PerNodeLimitTooLarge { .. })
    ));

    let fixture = fixture();
    let diagnostic_stream = stream(&fixture, 75, 1);
    let diagnostic_attestation = attestation(&key(), &fixture, &diagnostic_stream, "one");
    let verifier = Arc::new(verifier());
    let cache = DiagnosticEvidenceCache::new(16, 256 * 1024).unwrap();
    let gate = Arc::new(Barrier::new(2));
    let worker_gate = Arc::clone(&gate);
    let mut pool =
        DiagnosticVerificationWorkerPool::new_with_start_gate_and_limits(1, 2, 2, worker_gate)
            .unwrap();
    for _ in 0..2 {
        pool.submit(job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
        ))
        .unwrap();
    }
    assert!(matches!(
        pool.submit(job(
            &fixture,
            &diagnostic_stream,
            &diagnostic_attestation,
            &verifier,
            &cache,
        )),
        Err(DiagnosticVerificationWorkerError::QueueFull)
    ));
    gate.wait();
    let result = pool.next_ordered().unwrap().unwrap();
    let mut receiver = MultiNodeDiagnosticReceiver::new();
    receiver.register_node(7, verifier).unwrap();
    let stale_unit = fixture.candidates.keys().next().unwrap().clone();
    let stale = BTreeMap::from([(
        stale_unit,
        parse("def leaf(value: int) -> int:\n    return value + 100\n"),
    )]);
    assert!(matches!(
        receiver.ingest_worker_result(result, &fixture.snapshot, &fixture.profile, &stale),
        Err(EmissionDiagnosticNetworkError::Attestation(
            EmissionDiagnosticAttestationError::ContentMismatch
        ))
    ));
    assert_eq!(receiver.aggregator(7).unwrap().source_count(), 0);
    pool.close().unwrap();
}
