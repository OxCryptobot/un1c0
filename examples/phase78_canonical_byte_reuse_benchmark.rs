use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier,
};
use un1c0::emission_diagnostic_cache::DiagnosticEvidenceCache;
use un1c0::emission_diagnostic_instrumentation::DiagnosticInstrumentation;
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_receipt::ReceiptBoundBatchEmitter;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_cache::SemanticFingerprint;
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

const FRAME_COUNTS: [usize; 6] = [1, 2, 4, 8, 16, 32];
const SAMPLES: usize = 32;

#[derive(Serialize)]
struct Artifact {
    schema_version: u8,
    phase: u8,
    artifact: &'static str,
    samples: usize,
    frame_counts: Vec<usize>,
    rows: Vec<Row>,
    errors: usize,
    secret_material_recorded: bool,
}

#[derive(Serialize)]
struct Row {
    frames: usize,
    stream_bytes: usize,
    payload_bytes: usize,
    canonical_payload_reuse_p50_ns: u64,
    canonical_payload_reuse_p95_ns: u64,
    canonical_payload_reuse_p99_ns: u64,
    sha256_integrity_p50_ns: u64,
    sha256_integrity_p95_ns: u64,
    sha256_integrity_p99_ns: u64,
    canonical_json_reuse_p50_ns: u64,
    canonical_json_reuse_p95_ns: u64,
    canonical_json_reuse_p99_ns: u64,
    semantic_fingerprint_p50_ns: u64,
    semantic_fingerprint_p95_ns: u64,
    semantic_fingerprint_p99_ns: u64,
    full_verification_p50_ns: u64,
    full_verification_p95_ns: u64,
    full_verification_p99_ns: u64,
    warm_cache_admission_p50_ns: u64,
    warm_cache_admission_p95_ns: u64,
    warm_cache_admission_p99_ns: u64,
    sampled_ed25519_verify_ns: u64,
    sampled_canonical_stream_serialize_ns: u64,
    sampled_canonical_bytes_reuse_ns: u64,
    sampled_content_hash_ns: u64,
    sampled_snapshot_fingerprint_ns: u64,
    sampled_nested_report_verify_ns: u64,
    sampled_unattributed_ns: u64,
}

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
    let unit = SemanticUnitId::new("workspace/unit.ueg").expect("unit");
    let base = parse("def leaf(value: int) -> int:\n    return value + 1\n");
    let changed = parse("def leaf(value: int) -> int:\n    return value + 2\n");
    let NodeKind::Lambda(lambda) = &base.nodes[0];
    let range = SemanticEditRange::new(lambda.source_span.start_byte, lambda.source_span.end_byte)
        .expect("range");
    let mut session = SemanticBatchSession::start(
        profile.clone(),
        vec![SemanticUnitStart {
            unit: unit.clone(),
            ueg: base,
            capacity: 8,
        }],
    )
    .expect("session");
    let manifest = session.manifest_for(&unit, vec![range]).expect("manifest");
    let batch = SemanticEditBatch::new(vec![SemanticEditUpdate {
        unit: unit.clone(),
        ueg: changed.clone(),
        manifest,
    }])
    .expect("batch");
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).expect("envelope");
    session
        .refresh_envelope(&envelope, &profile)
        .expect("refresh");
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).expect("snapshot");
    Fixture {
        snapshot,
        profile,
        candidates: BTreeMap::from([(unit, changed)]),
    }
}

fn stream(fixture: &Fixture, frames: usize) -> EmissionDiagnosticStream {
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(
            &fixture.snapshot,
            1,
            &fixture.profile,
            &fixture.candidates,
            |_, _| Ok::<(), &'static str>(()),
        )
        .expect("receipt");
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .expect("report");
    EmissionDiagnosticStream::from_repeated_report(
        75,
        &report,
        frames,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .expect("stream")
}

fn percentile(values: &mut [u64], percentile: f64) -> u64 {
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index.min(values.len() - 1)]
}

fn measure<F>(samples: usize, mut operation: F) -> Vec<u64>
where
    F: FnMut(),
{
    let mut values = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation();
        values.push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    }
    values
}

fn main() {
    let fixture = fixture();
    let key = DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[17; 32]));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier
        .register_public_key(key.public_key())
        .expect("trusted benchmark key");
    let cache = DiagnosticEvidenceCache::new(64, 8 * 1024 * 1024).expect("cache");
    let instrumentation = DiagnosticInstrumentation::enabled(512);
    let mut rows = Vec::new();
    let mut errors = 0usize;

    for frames in FRAME_COUNTS {
        let diagnostic_stream = stream(&fixture, frames);
        let diagnostic_attestation = key
            .attest_stream(
                75,
                &diagnostic_stream,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                BTreeMap::from([("environment".into(), "benchmark".into())]),
            )
            .expect("attestation");
        let payload = diagnostic_stream
            .canonical_payload_bytes()
            .expect("canonical payload");
        let canonical_times = measure(SAMPLES, || {
            black_box(
                diagnostic_stream
                    .canonical_payload_bytes()
                    .expect("payload"),
            );
        });
        let hash_times = measure(SAMPLES, || {
            black_box(EmissionDiagnosticStream::canonical_payload_digest(
                black_box(&payload),
            ));
        });
        let canonical_json_times = measure(SAMPLES, || {
            black_box(diagnostic_stream.to_json().expect("canonical JSON"));
        });
        let fingerprint_times = measure(SAMPLES, || {
            for candidate in fixture.candidates.values() {
                black_box(SemanticFingerprint::from_ueg(candidate, &fixture.profile));
            }
        });
        let full_times = measure(SAMPLES, || {
            if verifier
                .verify_stream(
                    black_box(&diagnostic_attestation),
                    black_box(&diagnostic_stream),
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                )
                .is_err()
            {
                errors += 1;
            }
        });
        let _ = verifier
            .verify_stream_evidence_with_cache(
                &diagnostic_attestation,
                &diagnostic_stream,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                &cache,
                &instrumentation,
            )
            .expect("populate cache");
        let warm_times = measure(SAMPLES, || {
            if verifier
                .verify_stream_evidence_with_cache(
                    &diagnostic_attestation,
                    &diagnostic_stream,
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                    &cache,
                    &instrumentation,
                )
                .is_err()
            {
                errors += 1;
            }
        });
        let stage = instrumentation
            .snapshot()
            .samples
            .iter()
            .rev()
            .find(|sample| {
                sample.frame_count as usize == frames && sample.counters.signature_verifications > 0
            })
            .cloned();
        let Some(stage) = stage else {
            errors += 1;
            continue;
        };
        rows.push(Row {
            frames,
            stream_bytes: diagnostic_stream.total_frame_bytes(),
            payload_bytes: payload.len(),
            canonical_payload_reuse_p50_ns: percentile(&mut canonical_times.clone(), 0.50),
            canonical_payload_reuse_p95_ns: percentile(&mut canonical_times.clone(), 0.95),
            canonical_payload_reuse_p99_ns: percentile(&mut canonical_times.clone(), 0.99),
            sha256_integrity_p50_ns: percentile(&mut hash_times.clone(), 0.50),
            sha256_integrity_p95_ns: percentile(&mut hash_times.clone(), 0.95),
            sha256_integrity_p99_ns: percentile(&mut hash_times.clone(), 0.99),
            canonical_json_reuse_p50_ns: percentile(&mut canonical_json_times.clone(), 0.50),
            canonical_json_reuse_p95_ns: percentile(&mut canonical_json_times.clone(), 0.95),
            canonical_json_reuse_p99_ns: percentile(&mut canonical_json_times.clone(), 0.99),
            semantic_fingerprint_p50_ns: percentile(&mut fingerprint_times.clone(), 0.50),
            semantic_fingerprint_p95_ns: percentile(&mut fingerprint_times.clone(), 0.95),
            semantic_fingerprint_p99_ns: percentile(&mut fingerprint_times.clone(), 0.99),
            full_verification_p50_ns: percentile(&mut full_times.clone(), 0.50),
            full_verification_p95_ns: percentile(&mut full_times.clone(), 0.95),
            full_verification_p99_ns: percentile(&mut full_times.clone(), 0.99),
            warm_cache_admission_p50_ns: percentile(&mut warm_times.clone(), 0.50),
            warm_cache_admission_p95_ns: percentile(&mut warm_times.clone(), 0.95),
            warm_cache_admission_p99_ns: percentile(&mut warm_times.clone(), 0.99),
            sampled_ed25519_verify_ns: stage.stages.ed25519_verify_ns,
            sampled_canonical_stream_serialize_ns: stage.stages.canonical_stream_serialize_ns,
            sampled_canonical_bytes_reuse_ns: stage.stages.canonical_bytes_reuse_ns,
            sampled_content_hash_ns: stage.stages.content_hash_ns,
            sampled_snapshot_fingerprint_ns: stage.stages.snapshot_fingerprint_ns,
            sampled_nested_report_verify_ns: stage.stages.nested_report_verify_ns,
            sampled_unattributed_ns: stage.unattributed_ns,
        });
    }

    let artifact = Artifact {
        schema_version: 1,
        phase: 78,
        artifact: "diagnostic_canonical_byte_reuse",
        samples: SAMPLES,
        frame_counts: FRAME_COUNTS.to_vec(),
        rows,
        errors,
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize artifact")
    );
}
