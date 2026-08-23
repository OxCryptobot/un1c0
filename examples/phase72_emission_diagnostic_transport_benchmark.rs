use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_stream::EmissionDiagnosticStream;
use un1c0::emission_diagnostic_transport::{
    AsyncDiagnosticTransport, DistributedEmissionAggregator, MAX_AGGREGATE_FRAMES,
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
const FRAMES_PER_SOURCE: usize = 4;
const SOURCE_COUNTS: [usize; 4] = [1, 2, 4, 8];

#[derive(Debug, Serialize)]
struct BenchmarkArtifact {
    phase: u8,
    samples: usize,
    target: &'static str,
    units: usize,
    total_functions: usize,
    frames_per_source: usize,
    max_sources: usize,
    max_aggregate_frames: usize,
    rows: Vec<Row>,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

#[derive(Debug, Serialize)]
struct Row {
    source_count: usize,
    total_frames: usize,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
}

struct Fixture {
    snapshot: SemanticSnapshotEnvelope,
    profile: TargetCapabilityProfile,
    candidates: BTreeMap<SemanticUnitId, Ueg>,
    stream: EmissionDiagnosticStream,
}

fn main() {
    let fixture = prepared();
    let mut rows = Vec::new();
    for source_count in SOURCE_COUNTS {
        let total_frames = source_count * FRAMES_PER_SOURCE;
        let values = measure(SAMPLES, || {
            let transport =
                AsyncDiagnosticTransport::new(MAX_AGGREGATE_FRAMES).expect("transport capacity");
            for source_id in 1..=source_count as u64 {
                transport
                    .send(source_id, 1, &fixture.stream)
                    .expect("send diagnostic stream");
            }
            let mut aggregate = DistributedEmissionAggregator::new();
            for _ in 0..source_count {
                let observation = transport
                    .try_receive_for(&fixture.snapshot, &fixture.profile, &fixture.candidates)
                    .expect("receive diagnostic stream")
                    .expect("queued observation");
                aggregate
                    .ingest(
                        observation,
                        &fixture.snapshot,
                        &fixture.profile,
                        &fixture.candidates,
                    )
                    .expect("aggregate diagnostic stream");
            }
            assert_eq!(aggregate.source_count(), source_count);
            assert_eq!(aggregate.total_frames(), total_frames);
            black_box(aggregate.summary());
        });
        rows.push(Row {
            source_count,
            total_frames,
            p50_ns: percentile(&values, 0.50),
            p95_ns: percentile(&values, 0.95),
            p99_ns: percentile(&values, 0.99),
        });
    }

    let artifact = BenchmarkArtifact {
        phase: 72,
        samples: SAMPLES,
        target: fixture.profile.target.label(),
        units: 1,
        total_functions: 2,
        frames_per_source: FRAMES_PER_SOURCE,
        max_sources: un1c0::emission_diagnostic_transport::MAX_DISTRIBUTED_SOURCES,
        max_aggregate_frames: MAX_AGGREGATE_FRAMES,
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
    let stream = EmissionDiagnosticStream::from_repeated_report(
        72,
        &report,
        FRAMES_PER_SOURCE,
        &snapshot,
        &profile,
        &candidates,
    )
    .unwrap();
    Fixture {
        snapshot,
        profile,
        candidates,
        stream,
    }
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
