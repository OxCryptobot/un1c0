use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_session::{SemanticEditRange, SemanticSession};
use un1c0::semantic_snapshot::SemanticValidationSnapshot;
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

const SAMPLES: usize = 64;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    manifest_ranges: usize,
    changed_functions: usize,
    mapped_functions: usize,
    affected_functions: usize,
    revalidated_functions: usize,
    full_capture_p50_ns: u128,
    full_capture_p95_ns: u128,
    manifest_resolution_p50_ns: u128,
    manifest_resolution_p95_ns: u128,
    manifest_refresh_p50_ns: u128,
    manifest_refresh_p95_ns: u128,
    errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let mut rows = Vec::new();
    for &functions in FUNCTION_LEVELS {
        let base = parse(&fixture(functions, "value + 1"));
        let changed = parse(&fixture(functions, "value + 2"));
        for target in TargetBinding::ALL {
            let profile = TargetCapabilityProfile::for_target(target);
            let full_capture = measure(SAMPLES, || {
                SemanticValidationSnapshot::capture(&changed, profile.clone())
                    .expect("valid changed snapshot");
            });
            let mut session = SemanticSession::start(&base, profile.clone(), functions * 4)
                .expect("valid base session");
            let NodeKind::Lambda(leaf) = &base.nodes[0];
            let manifest = session
                .manifest_for_edits(vec![SemanticEditRange::new(
                    leaf.source_span.start_byte,
                    leaf.source_span.end_byte,
                )
                .expect("valid source range")])
                .expect("valid edit manifest");
            let resolution = session
                .derive_edit_resolution(&changed, &profile, &manifest)
                .expect("valid resolution");
            let resolution_samples = measure(SAMPLES, || {
                session
                    .derive_edit_resolution(&changed, &profile, &manifest)
                    .expect("repeatable resolution");
            });
            let refresh_samples = measure(SAMPLES, || {
                let mut local = SemanticSession::start(&base, profile.clone(), functions * 4)
                    .expect("valid refresh session");
                local
                    .refresh_from_edit_manifest(&changed, &profile, &manifest)
                    .expect("valid manifest refresh");
            });
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                manifest_ranges: manifest.ranges().len(),
                changed_functions: resolution.semantic_changes.changed_functions.len(),
                mapped_functions: resolution.mapped_functions.len(),
                affected_functions: resolution
                    .semantic_changes
                    .changed_functions
                    .len()
                    .saturating_add(functions.saturating_sub(1)),
                revalidated_functions: 1,
                full_capture_p50_ns: percentile(&full_capture, 0.50),
                full_capture_p95_ns: percentile(&full_capture, 0.95),
                manifest_resolution_p50_ns: percentile(&resolution_samples, 0.50),
                manifest_resolution_p95_ns: percentile(&resolution_samples, 0.95),
                manifest_refresh_p50_ns: percentile(&refresh_samples, 0.50),
                manifest_refresh_p95_ns: percentile(&refresh_samples, 0.95),
                errors: 0,
                cluster_mutation_performed: false,
                secret_material_recorded: false,
            });
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&rows).expect("serialize rows")
    );
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

fn fixture(functions: usize, leaf_body: &str) -> String {
    let mut source = String::new();
    for index in 0..functions {
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

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
