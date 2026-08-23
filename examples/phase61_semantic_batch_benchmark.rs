use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_batch::{
    SemanticBatchSession, SemanticEditBatch, SemanticEditUpdate, SemanticUnitId, SemanticUnitStart,
};
use un1c0::semantic_session::SemanticEditRange;
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
    changed_units: usize,
    refreshed_units: usize,
    batch_refresh_p50_ns: u128,
    batch_refresh_p95_ns: u128,
    sequential_refresh_p50_ns: u128,
    sequential_refresh_p95_ns: u128,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut rows = Vec::new();
    for &units in UNIT_LEVELS {
        let fixture = build_fixture(units);
        let batch_samples = measure(SAMPLES, || {
            let (mut session, batch) = create_batch(&fixture, profile.clone());
            session
                .refresh_batch(&batch, &profile)
                .expect("valid atomic batch refresh");
        });
        let sequential_samples = measure(SAMPLES, || {
            for unit in &fixture {
                let mut session = un1c0::semantic_session::SemanticSession::start(
                    &unit.base,
                    profile.clone(),
                    FUNCTIONS_PER_UNIT * 4,
                )
                .expect("valid sequential session");
                let manifest = session
                    .manifest_for_edits(vec![unit.leaf_range.clone()])
                    .expect("valid sequential manifest");
                session
                    .refresh_from_edit_manifest(&unit.changed, &profile, &manifest)
                    .expect("valid sequential refresh");
            }
        });
        rows.push(BenchmarkRow {
            units,
            functions_per_unit: FUNCTIONS_PER_UNIT,
            total_functions: units * FUNCTIONS_PER_UNIT,
            samples: SAMPLES,
            target: profile.target.label(),
            changed_units: units,
            refreshed_units: units,
            batch_refresh_p50_ns: percentile(&batch_samples, 0.50),
            batch_refresh_p95_ns: percentile(&batch_samples, 0.95),
            sequential_refresh_p50_ns: percentile(&sequential_samples, 0.50),
            sequential_refresh_p95_ns: percentile(&sequential_samples, 0.95),
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

struct UnitFixture {
    id: SemanticUnitId,
    base: Ueg,
    changed: Ueg,
    leaf_range: SemanticEditRange,
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

fn create_batch(
    fixture: &[UnitFixture],
    profile: TargetCapabilityProfile,
) -> (SemanticBatchSession, SemanticEditBatch) {
    let starts = fixture
        .iter()
        .map(|unit| SemanticUnitStart {
            unit: unit.id.clone(),
            ueg: unit.base.clone(),
            capacity: FUNCTIONS_PER_UNIT * 4,
        })
        .collect();
    let session = SemanticBatchSession::start(profile.clone(), starts).unwrap();
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
    (session, SemanticEditBatch::new(updates).unwrap())
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
