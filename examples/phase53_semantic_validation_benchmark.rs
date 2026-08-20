use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::validate_ueg_for_target;
use un1c0::walker::{python_to_ueg, Ueg};

const SAMPLES: usize = 96;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
    min_ns: u128,
    max_ns: u128,
    expression_count: usize,
    diagnostics: usize,
    valid_samples: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let mut rows = Vec::new();
    for &functions in FUNCTION_LEVELS {
        let source = fixture(functions);
        let ueg = parse(&source);
        for target in TargetBinding::ALL {
            let mut timings = Vec::with_capacity(SAMPLES);
            let mut valid_samples = 0;
            let mut expression_count = 0;
            let mut diagnostics = 0;
            for _ in 0..SAMPLES {
                let started = Instant::now();
                let report = validate_ueg_for_target(&ueg, target);
                timings.push(started.elapsed().as_nanos());
                valid_samples += usize::from(report.is_valid());
                expression_count = report.expression_count;
                diagnostics = report.diagnostics.len();
            }
            timings.sort_unstable();
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                p50_ns: percentile(&timings, 0.50),
                p95_ns: percentile(&timings, 0.95),
                p99_ns: percentile(&timings, 0.99),
                min_ns: *timings.first().expect("samples"),
                max_ns: *timings.last().expect("samples"),
                expression_count,
                diagnostics,
                valid_samples,
                cluster_mutation_performed: false,
                secret_material_recorded: false,
            });
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&rows).expect("serialize benchmark rows")
    );
}

fn fixture(functions: usize) -> String {
    let mut source = String::new();
    for index in 0..functions {
        source.push_str(&format!(
            "def function_{index}(value: int) -> int:\n    result = value + {index}\n    return result\n\n"
        ));
    }
    source
}

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}
