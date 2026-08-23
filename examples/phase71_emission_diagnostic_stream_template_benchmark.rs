use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::emission_diagnostic::EmissionDiagnosticReport;
use un1c0::emission_diagnostic_stream::{
    EmissionDiagnosticStream, EmissionDiagnosticStreamTemplate, MAX_STREAM_FRAMES,
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
const UNITS: usize = 4;
const FUNCTIONS_PER_UNIT: usize = 8;
const FRAME_COUNT: usize = MAX_STREAM_FRAMES;

#[derive(Debug, Serialize)]
struct BenchmarkArtifact {
    phase: u8,
    units: usize,
    functions_per_unit: usize,
    total_functions: usize,
    frames: usize,
    samples: usize,
    target: &'static str,
    baseline: TimingSummary,
    optimized_template: TimingSummary,
    current_report_list_builder: TimingSummary,
    optimized_p50_reduction_percent: f64,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

#[derive(Debug, Serialize)]
struct TimingSummary {
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
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
    let (snapshot, candidates) = prepared_state(profile.clone());
    let emitter = ReceiptBoundBatchEmitter::new(TargetBinding::Rust);
    let (receipt, _) = emitter
        .emit_with_receipt(&snapshot, 1, &profile, &candidates, |_, _| {
            Ok::<(), &'static str>(())
        })
        .expect("receipt-bound emission");
    let report = EmissionDiagnosticReport::from_receipts(
        std::slice::from_ref(&receipt),
        &snapshot,
        &profile,
        &candidates,
    )
    .expect("diagnostic report");
    let reports = vec![report.clone(); FRAME_COUNT];
    let template =
        EmissionDiagnosticStreamTemplate::from_report(&report, &snapshot, &profile, &candidates)
            .expect("verified stream template");

    let baseline = measure(SAMPLES, || {
        black_box(legacy_repeated_work(
            &reports,
            &snapshot,
            &profile,
            &candidates,
        ));
    });
    let optimized_template = measure(SAMPLES, || {
        black_box(
            template
                .build(71, FRAME_COUNT, &snapshot, &profile, &candidates)
                .expect("optimized template construction"),
        );
    });
    let current_report_list_builder = measure(SAMPLES, || {
        black_box(
            EmissionDiagnosticStream::from_verified_reports(
                71,
                &reports,
                &snapshot,
                &profile,
                &candidates,
            )
            .expect("current report-list construction"),
        );
    });

    let baseline_summary = summary(&baseline);
    let optimized_summary = summary(&optimized_template);
    let reduction = if baseline_summary.p50_ns == 0 {
        0.0
    } else {
        (baseline_summary
            .p50_ns
            .saturating_sub(optimized_summary.p50_ns) as f64
            / baseline_summary.p50_ns as f64)
            * 100.0
    };
    let artifact = BenchmarkArtifact {
        phase: 71,
        units: UNITS,
        functions_per_unit: FUNCTIONS_PER_UNIT,
        total_functions: UNITS * FUNCTIONS_PER_UNIT,
        frames: FRAME_COUNT,
        samples: SAMPLES,
        target: profile.target.label(),
        baseline: baseline_summary,
        optimized_template: optimized_summary,
        current_report_list_builder: summary(&current_report_list_builder),
        optimized_p50_reduction_percent: reduction,
        errors: 0,
        cluster_mutation_performed: false,
        secret_material_recorded: false,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark artifact")
    );
}

fn legacy_repeated_work(
    reports: &[EmissionDiagnosticReport],
    snapshot: &SemanticSnapshotEnvelope,
    profile: &TargetCapabilityProfile,
    candidates: &BTreeMap<SemanticUnitId, Ueg>,
) -> usize {
    let first = reports.first().expect("non-empty reports");
    let target = first.aggregate().target();
    let batch_id = first.aggregate().batch_id();
    let profile_key = first.aggregate().profile_key();
    let unit_roots = first.aggregate().unit_roots();
    let mut total = 0usize;
    for report in reports {
        report
            .verify_for(snapshot, profile, candidates)
            .expect("legacy current-state verification");
        assert_eq!(report.aggregate().target(), target);
        assert_eq!(report.aggregate().batch_id(), batch_id);
        assert_eq!(report.aggregate().profile_key(), profile_key);
        assert_eq!(report.aggregate().unit_roots(), unit_roots);
        total = total
            .checked_add(report.to_json().expect("legacy serialization").len())
            .expect("legacy byte total");
    }
    total
}

fn prepared_state(
    profile: TargetCapabilityProfile,
) -> (SemanticSnapshotEnvelope, BTreeMap<SemanticUnitId, Ueg>) {
    let fixtures = build_fixture();
    let starts = fixtures
        .iter()
        .map(|unit| SemanticUnitStart {
            unit: unit.id.clone(),
            ueg: unit.base.clone(),
            capacity: FUNCTIONS_PER_UNIT * 4,
        })
        .collect();
    let mut session = SemanticBatchSession::start(profile.clone(), starts).unwrap();
    let updates = fixtures
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
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session.refresh_envelope(&batch_envelope, &profile).unwrap();
    let snapshot = SemanticSnapshotEnvelope::capture(&session, 1).unwrap();
    let candidates = fixtures
        .into_iter()
        .map(|unit| (unit.id, unit.changed))
        .collect();
    (snapshot, candidates)
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
            leaf_body.to_owned()
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

fn summary(sorted: &[u128]) -> TimingSummary {
    TimingSummary {
        p50_ns: percentile(sorted, 0.50),
        p95_ns: percentile(sorted, 0.95),
        p99_ns: percentile(sorted, 0.99),
    }
}

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
