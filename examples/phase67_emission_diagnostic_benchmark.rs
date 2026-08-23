use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
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
const OBSERVATIONS: &[usize] = &[1, 2, 4, 8];
const UNITS: usize = 4;
const FUNCTIONS_PER_UNIT: usize = 8;

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    units: usize,
    observations: usize,
    functions_per_unit: usize,
    total_functions: usize,
    samples: usize,
    target: &'static str,
    report_p50_ns: u128,
    report_p95_ns: u128,
    verification_p50_ns: u128,
    verification_p95_ns: u128,
    diagnostic_entries: usize,
    chunks_emitted: usize,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

#[derive(Clone)]
struct UnitFixture {
    id: SemanticUnitId,
    base: Ueg,
    changed: Ueg,
    leaf_range: SemanticEditRange,
}

fn main() {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let (session, snapshot, candidates) = prepared_state(profile.clone());
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, stats) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .expect("receipt-bound emission");
    assert!(session.is_valid());

    let mut rows = Vec::new();
    for &observations in OBSERVATIONS {
        let receipts = vec![receipt.clone(); observations];
        let report_samples = measure(SAMPLES, || {
            EmissionDiagnosticReport::from_receipts(&receipts, &snapshot, &profile, &candidates)
                .expect("diagnostic report generation");
        });
        let report =
            EmissionDiagnosticReport::from_receipts(&receipts, &snapshot, &profile, &candidates)
                .expect("diagnostic report generation");
        let verification_samples = measure(SAMPLES, || {
            report
                .verify_for(&snapshot, &profile, &candidates)
                .expect("diagnostic report verification");
        });
        rows.push(BenchmarkRow {
            units: UNITS,
            observations,
            functions_per_unit: FUNCTIONS_PER_UNIT,
            total_functions: UNITS * FUNCTIONS_PER_UNIT,
            samples: SAMPLES,
            target: profile.target.label(),
            report_p50_ns: percentile(&report_samples, 0.50),
            report_p95_ns: percentile(&report_samples, 0.95),
            verification_p50_ns: percentile(&verification_samples, 0.50),
            verification_p95_ns: percentile(&verification_samples, 0.95),
            diagnostic_entries: report.entries().len(),
            chunks_emitted: stats.chunks_emitted,
            errors: 0,
            cluster_mutation_performed: false,
            secret_material_recorded: false,
        });
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&rows).expect("serialize benchmark rows")
    );
}

fn prepared_state(
    profile: TargetCapabilityProfile,
) -> (
    un1c0::SemanticBatchSession,
    SemanticSnapshotEnvelope,
    BTreeMap<SemanticUnitId, Ueg>,
) {
    let fixture = build_fixture();
    let starts = fixture
        .iter()
        .map(|unit| SemanticUnitStart {
            unit: unit.id.clone(),
            ueg: unit.base.clone(),
            capacity: FUNCTIONS_PER_UNIT * 4,
        })
        .collect();
    let mut session = SemanticBatchSession::start(profile.clone(), starts).unwrap();
    let updates = fixture
        .iter()
        .map(|unit| SemanticEditUpdate {
            unit: unit.id.clone(),
            ueg: unit.changed.clone(),
            manifest: session
                .manifest_for(&unit.id, vec![unit.leaf_range.clone()])
                .unwrap(),
        })
        .collect();
    let batch = SemanticEditBatch::new(updates).unwrap();
    let envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = fixture
        .into_iter()
        .map(|unit| (unit.id, unit.changed))
        .collect();
    (session, snapshot, candidates)
}

fn build_fixture() -> Vec<UnitFixture> {
    (0..UNITS)
        .map(|index| {
            let base = parse(&source("value + 1"));
            let changed = parse(&source("value + 2"));
            let (start_byte, end_byte) = match &base.nodes[0] {
                NodeKind::Lambda(lambda) => {
                    (lambda.source_span.start_byte, lambda.source_span.end_byte)
                }
            };
            UnitFixture {
                id: SemanticUnitId::new(format!("workspace/unit_{index}.ueg")).unwrap(),
                base,
                changed,
                leaf_range: SemanticEditRange::new(start_byte, end_byte).unwrap(),
            }
        })
        .collect()
}

fn source(leaf_body: &str) -> String {
    let mut source = String::new();
    for index in 0..FUNCTIONS_PER_UNIT {
        let body = if index == 0 {
            leaf_body.to_string()
        } else {
            format!("function_{}(value)", index - 1)
        };
        source.push_str(&format!(
            "def function_{index}(value: int) -> int:\n    return {body}\n\n"
        ));
    }
    source
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
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
