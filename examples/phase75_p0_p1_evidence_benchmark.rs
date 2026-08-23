use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use ed25519_dalek::SigningKey;
use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_attestation::{
    DiagnosticAttestationKey, DiagnosticAttestationVerifier,
};
use un1c0::emission_diagnostic_instrumentation::DiagnosticInstrumentation;
use un1c0::emission_diagnostic_network::MultiNodeDiagnosticReceiver;
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

const FRAME_COUNTS: [usize; 4] = [1, 2, 4, 8];
const SAMPLES: usize = 24;
const REUSED_ADMISSIONS_PER_SAMPLE: usize = 8;

#[derive(Serialize)]
struct Artifact {
    phase: u8,
    samples: usize,
    frame_counts: Vec<usize>,
    rows: Vec<Row>,
    errors: usize,
    instrumentation_enabled: bool,
    secret_material_recorded: bool,
}

#[derive(Serialize)]
struct Row {
    frames_per_evidence: usize,
    stream_bytes: usize,
    baseline_verify_p50_ns: u64,
    baseline_verify_p95_ns: u64,
    baseline_verify_p99_ns: u64,
    evidence_build_p50_ns: u64,
    evidence_admission_p50_ns: u64,
    evidence_admission_p95_ns: u64,
    evidence_admission_p99_ns: u64,
    reuse_speedup_pct: f64,
    sampled_ed25519_verify_ns: u64,
    sampled_signing_payload_serialize_ns: u64,
    sampled_trust_lookup_ns: u64,
    sampled_canonical_stream_serialize_ns: u64,
    sampled_content_hash_ns: u64,
    sampled_snapshot_fingerprint_ns: u64,
    sampled_nested_report_verify_ns: u64,
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

fn key() -> DiagnosticAttestationKey {
    DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[17; 32]))
}

fn attestation(
    key: &DiagnosticAttestationKey,
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
) -> un1c0::EmissionDiagnosticAttestation {
    key.attest_stream(
        1,
        stream,
        &fixture.snapshot,
        &fixture.profile,
        &fixture.candidates,
        BTreeMap::from([("environment".into(), "benchmark".into())]),
    )
    .expect("attestation")
}

fn main() {
    let fixture = fixture();
    let key = key();
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier
        .register_public_key(key.public_key())
        .expect("trusted benchmark key");
    let instrumentation = DiagnosticInstrumentation::enabled(256);
    let mut rows = Vec::new();
    let mut errors = 0usize;

    for frames in FRAME_COUNTS {
        let diagnostic_stream = stream(&fixture, frames);
        let diagnostic_attestation = attestation(&key, &fixture, &diagnostic_stream);
        let mut baseline_times = Vec::new();
        let mut build_times = Vec::new();
        let mut admission_times = Vec::new();
        let mut stage_sample = None;

        for _ in 0..SAMPLES {
            for _ in 0..REUSED_ADMISSIONS_PER_SAMPLE {
                let started = Instant::now();
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
                baseline_times.push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
            }

            let started = Instant::now();
            let evidence = match verifier.verify_stream_evidence(
                &diagnostic_attestation,
                &diagnostic_stream,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                &instrumentation,
            ) {
                Ok(evidence) => evidence,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            build_times.push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
            if stage_sample.is_none() {
                stage_sample = instrumentation.snapshot().samples.last().cloned();
            }

            let mut receiver = MultiNodeDiagnosticReceiver::new();
            receiver
                .register_node(7, Arc::new(verifier.clone()))
                .expect("register benchmark node");
            for sequence in 1..=REUSED_ADMISSIONS_PER_SAMPLE as u64 {
                let started = Instant::now();
                if receiver
                    .ingest_verified(
                        7,
                        9,
                        sequence,
                        black_box(evidence.clone()),
                        &fixture.snapshot,
                        &fixture.profile,
                        &fixture.candidates,
                    )
                    .is_err()
                {
                    errors += 1;
                }
                admission_times.push(started.elapsed().as_nanos().min(u64::MAX as u128) as u64);
            }
        }

        let baseline_p50 = percentile(&mut baseline_times, 0.50);
        let admission_p50 = percentile(&mut admission_times, 0.50);
        let Some(stage) = stage_sample else {
            errors += 1;
            continue;
        };
        rows.push(Row {
            frames_per_evidence: frames,
            stream_bytes: diagnostic_stream.total_frame_bytes(),
            baseline_verify_p50_ns: baseline_p50,
            baseline_verify_p95_ns: percentile(&mut baseline_times, 0.95),
            baseline_verify_p99_ns: percentile(&mut baseline_times, 0.99),
            evidence_build_p50_ns: percentile(&mut build_times, 0.50),
            evidence_admission_p50_ns: admission_p50,
            evidence_admission_p95_ns: percentile(&mut admission_times, 0.95),
            evidence_admission_p99_ns: percentile(&mut admission_times, 0.99),
            reuse_speedup_pct: if admission_p50 == 0 {
                0.0
            } else {
                (1.0 - admission_p50 as f64 / baseline_p50 as f64) * 100.0
            },
            sampled_ed25519_verify_ns: stage.stages.ed25519_verify_ns,
            sampled_signing_payload_serialize_ns: stage.stages.signing_payload_serialize_ns,
            sampled_trust_lookup_ns: stage.stages.trust_lookup_ns,
            sampled_canonical_stream_serialize_ns: stage.stages.canonical_stream_serialize_ns,
            sampled_content_hash_ns: stage.stages.content_hash_ns,
            sampled_snapshot_fingerprint_ns: stage.stages.snapshot_fingerprint_ns,
            sampled_nested_report_verify_ns: stage.stages.nested_report_verify_ns,
        });
    }

    let artifact = Artifact {
        phase: 75,
        samples: SAMPLES,
        frame_counts: FRAME_COUNTS.to_vec(),
        rows,
        errors,
        instrumentation_enabled: true,
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark artifact")
    );
}
