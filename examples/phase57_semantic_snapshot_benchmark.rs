use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_snapshot::SemanticValidationSnapshot;
use un1c0::walker::{python_to_ueg, Ueg};

const SAMPLES: usize = 64;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    expression_count: usize,
    capture_p50_ns: u128,
    capture_p95_ns: u128,
    capture_p99_ns: u128,
    verify_p50_ns: u128,
    verify_p95_ns: u128,
    verify_p99_ns: u128,
    verification_errors: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let mut rows = Vec::new();
    for &functions in FUNCTION_LEVELS {
        let source = fixture(functions);
        let ueg = parse(&source);
        for target in TargetBinding::ALL {
            let profile = TargetCapabilityProfile::for_target(target);
            let snapshot = SemanticValidationSnapshot::capture(&ueg, profile.clone())
                .expect("valid semantic snapshot");
            let capture = measure(SAMPLES, || {
                SemanticValidationSnapshot::capture(&ueg, profile.clone())
                    .expect("repeatable valid snapshot");
            });
            let verification = measure(SAMPLES, || {
                snapshot
                    .verify_for(&ueg, &profile)
                    .expect("same-input verification");
            });
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                expression_count: snapshot.report().expression_count,
                capture_p50_ns: percentile(&capture, 0.50),
                capture_p95_ns: percentile(&capture, 0.95),
                capture_p99_ns: percentile(&capture, 0.99),
                verify_p50_ns: percentile(&verification, 0.50),
                verify_p95_ns: percentile(&verification, 0.95),
                verify_p99_ns: percentile(&verification, 0.99),
                verification_errors: 0,
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

fn fixture(functions: usize) -> String {
    let mut source = String::new();
    for index in 0..functions {
        let body = if index == 0 {
            "value + 1".to_string()
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
