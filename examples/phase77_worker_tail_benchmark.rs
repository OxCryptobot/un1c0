use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier, EmissionDiagnosticAttestation,
};
use un1c0::emission_diagnostic_cache::DiagnosticEvidenceCache;
use un1c0::emission_diagnostic_instrumentation::DiagnosticInstrumentation;
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_workers::{
    DiagnosticVerificationJob, DiagnosticVerificationWorkerPool,
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

const SAMPLES: usize = 17;

#[derive(Serialize)]
struct BenchmarkArtifact {
    schema_version: u8,
    phase: u8,
    artifact: &'static str,
    rows: Vec<Row>,
    errors: usize,
    secret_material_recorded: bool,
}

#[derive(Serialize)]
struct Row {
    worker_count: usize,
    job_count: usize,
    submitted_jobs: u64,
    completed_jobs: u64,
    failed_jobs: u64,
    cancelled_jobs: u64,
    queue_full_rejections: u64,
    fairness_rejections: u64,
    out_of_order_buffered: u64,
    queue_wait_p50_us: u64,
    queue_wait_p95_us: u64,
    queue_wait_max_us: u64,
    verification_service_p50_us: u64,
    verification_service_p95_us: u64,
    verification_service_max_us: u64,
    end_to_end_p50_us: u64,
    end_to_end_p95_us: u64,
    end_to_end_p99_us: u64,
    end_to_end_max_us: u64,
    throughput_jobs_per_sec: f64,
    errors: usize,
    sample_count: usize,
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
    let unit = SemanticUnitId::new("workspace/phase77.ueg").unwrap();
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

fn signing_key() -> DiagnosticAttestationKey {
    DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[23; 32]))
}

fn verifier() -> Arc<DiagnosticAttestationVerifier> {
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier
        .register_public_key(signing_key().public_key())
        .unwrap();
    Arc::new(verifier)
}

fn stream(fixture: &Fixture) -> EmissionDiagnosticStream {
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
    let stream = EmissionDiagnosticStream::from_repeated_report(
        77,
        &report,
        8,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
    )
    .unwrap();
    stream
}

fn attestation(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
    index: usize,
) -> EmissionDiagnosticAttestation {
    signing_key()
        .attest_stream(
            stream.stream_id(),
            stream,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
            BTreeMap::from([("environment".into(), format!("benchmark-{index}"))]),
        )
        .unwrap()
}

fn job(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
    attestation: &EmissionDiagnosticAttestation,
    verifier: &Arc<DiagnosticAttestationVerifier>,
    cache: &DiagnosticEvidenceCache,
    index: usize,
) -> DiagnosticVerificationJob {
    DiagnosticVerificationJob {
        node_id: (index % 8 + 1) as u64,
        connection_id: 100 + index as u64,
        sequence: index as u64 + 1,
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

fn percentile(values: &[u64], percentile: usize) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    ordered[((ordered.len() - 1) * percentile / 100).min(ordered.len() - 1)]
}

fn percentile_f64(values: &[f64], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    ordered[((ordered.len() - 1) * percentile / 100).min(ordered.len() - 1)]
}

fn aggregate(samples: &[Row]) -> Row {
    let first = samples.first().expect("at least one benchmark sample");
    let exact_counters = |field: fn(&Row) -> u64| {
        let expected = field(first);
        assert!(samples.iter().all(|sample| field(sample) == expected));
        expected
    };
    assert!(samples.iter().all(|sample| sample.errors == 0));
    let aggregate_u64 = |field: fn(&Row) -> u64, quantile: usize| {
        let values: Vec<_> = samples.iter().map(field).collect();
        percentile(&values, quantile)
    };
    let aggregate_max =
        |field: fn(&Row) -> u64| samples.iter().map(field).max().expect("sample values");
    let aggregate_f64 = |percentile: usize| {
        let values: Vec<_> = samples
            .iter()
            .map(|sample| sample.throughput_jobs_per_sec)
            .collect();
        percentile_f64(&values, percentile)
    };
    Row {
        worker_count: first.worker_count,
        job_count: first.job_count,
        submitted_jobs: exact_counters(|row| row.submitted_jobs),
        completed_jobs: exact_counters(|row| row.completed_jobs),
        failed_jobs: exact_counters(|row| row.failed_jobs),
        cancelled_jobs: exact_counters(|row| row.cancelled_jobs),
        queue_full_rejections: exact_counters(|row| row.queue_full_rejections),
        fairness_rejections: exact_counters(|row| row.fairness_rejections),
        out_of_order_buffered: aggregate_max(|row| row.out_of_order_buffered),
        queue_wait_p50_us: aggregate_u64(|row| row.queue_wait_p50_us, 50),
        queue_wait_p95_us: aggregate_u64(|row| row.queue_wait_p95_us, 95),
        queue_wait_max_us: aggregate_max(|row| row.queue_wait_max_us),
        verification_service_p50_us: aggregate_u64(|row| row.verification_service_p50_us, 50),
        verification_service_p95_us: aggregate_u64(|row| row.verification_service_p95_us, 95),
        verification_service_max_us: aggregate_max(|row| row.verification_service_max_us),
        end_to_end_p50_us: aggregate_u64(|row| row.end_to_end_p50_us, 50),
        end_to_end_p95_us: aggregate_u64(|row| row.end_to_end_p95_us, 95),
        end_to_end_p99_us: aggregate_u64(|row| row.end_to_end_p99_us, 99),
        end_to_end_max_us: aggregate_max(|row| row.end_to_end_max_us),
        throughput_jobs_per_sec: aggregate_f64(50),
        errors: 0,
        sample_count: samples.len(),
    }
}

fn measure(fixture: &Fixture, workers: usize, jobs: usize) -> Row {
    let stream = stream(fixture);
    let attestations: Vec<_> = (0..jobs)
        .map(|index| attestation(fixture, &stream, index))
        .collect();
    let verifier = verifier();
    let cache = DiagnosticEvidenceCache::new(64, 2 * 1024 * 1024).unwrap();
    let queue_capacity = jobs.max(16);
    let per_node_limit = jobs.min(8).max(1);
    let mut pool =
        DiagnosticVerificationWorkerPool::new_with_limits(workers, queue_capacity, per_node_limit)
            .unwrap();
    let mut submitted_at = BTreeMap::new();
    let start = Instant::now();
    let mut errors = 0;
    for index in 0..jobs {
        let input = job(
            fixture,
            &stream,
            &attestations[index],
            &verifier,
            &cache,
            index,
        );
        match pool.submit(input) {
            Ok(job_id) => {
                submitted_at.insert(job_id, Instant::now());
            }
            Err(_) => errors += 1,
        }
    }
    let mut e2e_us = Vec::with_capacity(jobs);
    for _ in 0..jobs {
        match pool.next_ordered() {
            Ok(Some(result)) if result.evidence.is_ok() && !result.is_cancelled() => {
                if let Some(submitted) = submitted_at.get(&result.job_id) {
                    e2e_us.push(
                        Instant::now()
                            .saturating_duration_since(*submitted)
                            .as_micros()
                            .min(u64::MAX as u128) as u64,
                    );
                } else {
                    errors += 1;
                }
            }
            Ok(Some(_)) | Ok(None) | Err(_) => errors += 1,
        }
    }
    let wall_us = start.elapsed().as_micros().max(1) as f64;
    let metrics = pool.metrics();
    let completed = e2e_us.len() as u64;
    let _ = pool.close();
    Row {
        worker_count: workers,
        job_count: jobs,
        submitted_jobs: metrics.submitted_jobs,
        completed_jobs: metrics.completed_jobs,
        failed_jobs: metrics.failed_jobs,
        cancelled_jobs: metrics.cancelled_jobs,
        queue_full_rejections: metrics.queue_full_rejections,
        fairness_rejections: metrics.fairness_rejections,
        out_of_order_buffered: metrics.out_of_order_buffered,
        queue_wait_p50_us: metrics.queue_wait_p50_us,
        queue_wait_p95_us: metrics.queue_wait_p95_us,
        queue_wait_max_us: metrics.queue_wait_max_us,
        verification_service_p50_us: metrics.verification_service_p50_us,
        verification_service_p95_us: metrics.verification_service_p95_us,
        verification_service_max_us: metrics.verification_service_max_us,
        end_to_end_p50_us: percentile(&e2e_us, 50),
        end_to_end_p95_us: percentile(&e2e_us, 95),
        end_to_end_p99_us: percentile(&e2e_us, 99),
        end_to_end_max_us: e2e_us.iter().copied().max().unwrap_or(0),
        throughput_jobs_per_sec: completed as f64 / (wall_us / 1_000_000.0),
        errors,
        sample_count: 1,
    }
}

fn main() {
    let fixture = fixture();
    let mut rows = Vec::new();
    let mut errors = 0;
    for workers in [1, 2, 4, 8] {
        for jobs in [1, 4, 8, 16] {
            let samples: Vec<_> = (0..SAMPLES)
                .map(|_| measure(&fixture, workers, jobs))
                .collect();
            let row = aggregate(&samples);
            errors += row.errors;
            rows.push(row);
        }
    }
    let artifact = BenchmarkArtifact {
        schema_version: 1,
        phase: 77,
        artifact: "diagnostic_worker_tail_latency",
        rows,
        errors,
        secret_material_recorded: false,
    };
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
}
