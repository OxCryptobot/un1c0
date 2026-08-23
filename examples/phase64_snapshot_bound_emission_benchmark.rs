use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchEnvelope, SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate,
    SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
use un1c0::semantic_snapshot_envelope::SemanticSnapshotEnvelope;
use un1c0::snapshot_emission::SnapshotBoundBatchEmitter;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

const SAMPLES: usize = 64;
const UNIT_LEVELS: &[usize] = &[1, 2, 4, 8];
const FUNCTIONS_PER_UNIT: usize = 8;

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    units: usize,
    functions_per_unit: usize,
    total_functions: usize,
    samples: usize,
    target: &'static str,
    batch_id: u64,
    verification_p50_ns: u128,
    verification_p95_ns: u128,
    emission_p50_ns: u128,
    emission_p95_ns: u128,
    chunks_emitted: usize,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

struct UnitFixture {
    id: SemanticUnitId,
    base: Ueg,
    changed: Ueg,
    leaf_range: SemanticEditRange,
}

fn main() {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let emitter = SnapshotBoundBatchEmitter::new(TargetBinding::Rust);
    let mut rows = Vec::new();
    for &units in UNIT_LEVELS {
        let (session, candidates, batch_envelope) = prepared_state(units, profile.clone());
        let snapshot =
            SemanticSnapshotEnvelope::capture(&session, batch_envelope.batch_id()).unwrap();
        let verification_samples = measure(SAMPLES, || {
            snapshot
                .verify_for(batch_envelope.batch_id(), &profile, &candidates)
                .expect("verify exact snapshot envelope");
        });
        let mut chunks_emitted = 0;
        let emission_samples = measure(SAMPLES, || {
            let stats = emitter
                .emit(
                    &snapshot,
                    batch_envelope.batch_id(),
                    &profile,
                    &candidates,
                    |_, _| Ok::<(), &'static str>(()),
                )
                .expect("snapshot-bound emission");
            chunks_emitted = stats.chunks_emitted;
        });
        rows.push(BenchmarkRow {
            units,
            functions_per_unit: FUNCTIONS_PER_UNIT,
            total_functions: units * FUNCTIONS_PER_UNIT,
            samples: SAMPLES,
            target: profile.target.label(),
            batch_id: batch_envelope.batch_id(),
            verification_p50_ns: percentile(&verification_samples, 0.50),
            verification_p95_ns: percentile(&verification_samples, 0.95),
            emission_p50_ns: percentile(&emission_samples, 0.50),
            emission_p95_ns: percentile(&emission_samples, 0.95),
            chunks_emitted,
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
    units: usize,
    profile: TargetCapabilityProfile,
) -> (
    SemanticBatchSession,
    BTreeMap<SemanticUnitId, Ueg>,
    SemanticBatchEnvelope,
) {
    let fixture = build_fixture(units);
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
    let batch_envelope = SemanticBatchEnvelope::new(1, session.profile_key(), batch).unwrap();
    session
        .refresh_envelope(&batch_envelope, &profile)
        .expect("apply semantic batch");
    let candidates = fixture
        .into_iter()
        .map(|unit| (unit.id, unit.changed))
        .collect();
    (session, candidates, batch_envelope)
}

fn build_fixture(units: usize) -> Vec<UnitFixture> {
    (0..units)
        .map(|index| {
            let base = parse(&function_fixture("value + 1"));
            let changed = parse(&function_fixture("value + 2"));
            let (start_byte, end_byte) = match &base.nodes[0] {
                NodeKind::Lambda(leaf) => (leaf.source_span.start_byte, leaf.source_span.end_byte),
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

fn function_fixture(leaf_body: &str) -> String {
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
