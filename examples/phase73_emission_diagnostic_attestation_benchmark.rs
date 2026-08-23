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
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_transport::{
    AsyncDiagnosticTransport, DistributedEmissionAggregator,
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

const SAMPLES: usize = 64;
const OBSERVATION_COUNTS: [usize; 4] = [1, 2, 4, 8];

#[derive(Debug, Serialize)]
struct BenchmarkArtifact {
    phase: u8,
    samples: usize,
    target: &'static str,
    rows: Vec<Row>,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

#[derive(Debug, Serialize)]
struct Row {
    observations: usize,
    stream_bytes: usize,
    aggregate_sources: usize,
    stream_attest_p50_ns: u128,
    stream_attest_p95_ns: u128,
    stream_attest_p99_ns: u128,
    stream_verify_p50_ns: u128,
    stream_verify_p95_ns: u128,
    stream_verify_p99_ns: u128,
    aggregate_attest_p50_ns: u128,
    aggregate_attest_p95_ns: u128,
    aggregate_attest_p99_ns: u128,
    aggregate_verify_p50_ns: u128,
    aggregate_verify_p95_ns: u128,
    aggregate_verify_p99_ns: u128,
}

struct Fixture {
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
    report: EmissionDiagnosticReport,
}

fn main() {
    let fixture = prepared();
    let key = DiagnosticAttestationKey::from_signing_key(SigningKey::from_bytes(&[73; 32]));
    let mut verifier = DiagnosticAttestationVerifier::new();
    verifier
        .register_public_key(key.public_key())
        .expect("register benchmark key");
    let mut rows = Vec::new();

    for observations in OBSERVATION_COUNTS {
        let stream = EmissionDiagnosticStream::from_repeated_report(
            73,
            &fixture.report,
            observations,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .expect("build benchmark stream");
        let aggregate = aggregate(&fixture, &stream);
        let stream_bytes = stream.to_json().expect("serialize benchmark stream").len();

        let stream_attest = measure(SAMPLES, || {
            let attestation = key
                .attest_stream(
                    observations as u64,
                    &stream,
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                    BTreeMap::new(),
                )
                .expect("attest stream");
            black_box(attestation.content_hash());
        });
        let stream_attestation = key
            .attest_stream(
                observations as u64,
                &stream,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                BTreeMap::new(),
            )
            .expect("prepare stream attestation");
        let stream_verify = measure(SAMPLES, || {
            verifier
                .verify_stream(
                    &stream_attestation,
                    &stream,
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                )
                .expect("verify stream");
            black_box(stream_attestation.content_hash());
        });

        let aggregate_attest = measure(SAMPLES, || {
            let attestation = key
                .attest_aggregate(
                    observations as u64 + 100,
                    &aggregate,
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                    BTreeMap::new(),
                )
                .expect("attest aggregate");
            black_box(attestation.content_hash());
        });
        let aggregate_attestation = key
            .attest_aggregate(
                observations as u64 + 100,
                &aggregate,
                &fixture.snapshot,
                &fixture.profile,
                &fixture.candidates,
                BTreeMap::new(),
            )
            .expect("prepare aggregate attestation");
        let aggregate_verify = measure(SAMPLES, || {
            verifier
                .verify_aggregate_for(
                    &aggregate_attestation,
                    &aggregate,
                    &fixture.snapshot,
                    &fixture.profile,
                    &fixture.candidates,
                )
                .expect("verify aggregate");
            black_box(aggregate_attestation.content_hash());
        });

        rows.push(Row {
            observations,
            stream_bytes,
            aggregate_sources: aggregate.source_count(),
            stream_attest_p50_ns: percentile(&stream_attest, 0.50),
            stream_attest_p95_ns: percentile(&stream_attest, 0.95),
            stream_attest_p99_ns: percentile(&stream_attest, 0.99),
            stream_verify_p50_ns: percentile(&stream_verify, 0.50),
            stream_verify_p95_ns: percentile(&stream_verify, 0.95),
            stream_verify_p99_ns: percentile(&stream_verify, 0.99),
            aggregate_attest_p50_ns: percentile(&aggregate_attest, 0.50),
            aggregate_attest_p95_ns: percentile(&aggregate_attest, 0.95),
            aggregate_attest_p99_ns: percentile(&aggregate_attest, 0.99),
            aggregate_verify_p50_ns: percentile(&aggregate_verify, 0.50),
            aggregate_verify_p95_ns: percentile(&aggregate_verify, 0.95),
            aggregate_verify_p99_ns: percentile(&aggregate_verify, 0.99),
        });
    }

    let artifact = BenchmarkArtifact {
        phase: 73,
        samples: SAMPLES,
        target: fixture.profile.target.label(),
        rows,
        errors: 0,
        cluster_mutation_performed: false,
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark artifact")
    );
}

fn prepared() -> Fixture {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let unit = SemanticUnitId::new("workspace/unit.ueg").unwrap();
    let base = parse(&source("value + 1"));
    let changed = parse(&source("value + 2"));
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
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&batch_envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = BTreeMap::from([(unit, changed)]);
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .unwrap();
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    Fixture {
        snapshot,
        profile,
        candidates,
        report,
    }
}

fn aggregate(
    fixture: &Fixture,
    stream: &EmissionDiagnosticStream,
) -> DistributedEmissionAggregator {
    let transport = AsyncDiagnosticTransport::new(1).unwrap();
    transport.send(73, 1, stream).unwrap();
    let observation = transport
        .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
        .unwrap()
        .unwrap();
    let mut aggregate = DistributedEmissionAggregator::new();
    aggregate
        .ingest(
            observation,
            &fixture.snapshot,
            &fixture.profile,
            &fixture.candidates,
        )
        .unwrap();
    aggregate
}

fn source(body: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {body}\n\ndef caller(value: int) -> int:\n    return leaf(value)\n"
    )
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn measure<F>(samples: usize, mut operation: F) -> Vec<u128>
where
    F: FnMut(),
{
    let mut values = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation();
        values.push(started.elapsed().as_nanos());
    }
    values.sort_unstable();
    values
}

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
