use std::hint::black_box;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::{generate_incrementally, TargetBinding};
use un1c0::walker::python_to_ueg;

const ITERATIONS: usize = 64;
const FUNCTION_COUNTS: [usize; 6] = [1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkArtifact {
    phase: u8,
    iterations: usize,
    function_counts: Vec<FunctionBenchmark>,
    secret_material_recorded: bool,
    cluster_mutation_performed: bool,
    workload: &'static str,
}

#[derive(Debug, Serialize)]
struct FunctionBenchmark {
    functions: usize,
    source_bytes: usize,
    parser: ParserMetrics,
    generation: Vec<TargetMetrics>,
}

#[derive(Debug, Serialize)]
struct ParserMetrics {
    parsed_nodes: usize,
    parse_p50_us: u64,
    parse_p95_us: u64,
    parse_max_us: u64,
    parse_per_function_p95_us: u64,
    baseline_single_function_p95_us: u64,
}

#[derive(Debug, Serialize)]
struct TargetMetrics {
    target: &'static str,
    chunks: usize,
    bytes: usize,
    generation_p50_us: u64,
    generation_p95_us: u64,
    generation_max_us: u64,
}

fn main() {
    let mut results = Vec::new();
    for &functions in &FUNCTION_COUNTS {
        let source = source_for(functions);
        let parse_samples = measure_parse(&source);
        let ueg = parse_once(&source);
        let baseline_single_function_p95_us = if functions == 1 {
            percentile(&parse_samples, 0.95)
        } else {
            percentile(&measure_parse(&source_for(1)), 0.95)
        };
        let generation = TargetBinding::ALL
            .into_iter()
            .map(|target| measure_generation(&ueg, target))
            .collect();
        results.push(FunctionBenchmark {
            functions,
            source_bytes: source.len(),
            parser: ParserMetrics {
                parsed_nodes: ueg.nodes.len(),
                parse_p50_us: percentile(&parse_samples, 0.50),
                parse_p95_us: percentile(&parse_samples, 0.95),
                parse_max_us: *parse_samples.iter().max().unwrap_or(&0),
                parse_per_function_p95_us: percentile(&parse_samples, 0.95) / functions as u64,
                baseline_single_function_p95_us,
            },
            generation,
        });
    }

    let artifact = BenchmarkArtifact {
        phase: 50,
        iterations: ITERATIONS,
        function_counts: results,
        secret_material_recorded: false,
        cluster_mutation_performed: false,
        workload: "deterministic Phase 49 multi-function UEG parsing versus single-function baseline with incremental target generation",
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&artifact).expect("serialize benchmark artifact")
    );
}

fn source_for(functions: usize) -> String {
    let mut source = String::new();
    for index in 0..functions {
        source.push_str(&format!(
            "def fn_{index}(value: List[int], limit: int) -> int:\n    if limit <= 0:\n        return value[0]\n    total, current = value[0], value[0]\n    for item in range(1, limit + 1):\n        print(item)\n    return total + current\n\n"
        ));
    }
    source
}

fn parse_once(source: &str) -> un1c0::walker::Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse benchmark source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn measure_parse(source: &str) -> Vec<u64> {
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let ueg = black_box(parse_once(source));
        assert!(ueg.validate(), "benchmark fixture must remain valid");
        samples.push(started.elapsed().as_micros() as u64);
    }
    samples
}

fn measure_generation(ueg: &un1c0::walker::Ueg, target: TargetBinding) -> TargetMetrics {
    let mut samples = Vec::with_capacity(ITERATIONS);
    let mut bytes = 0usize;
    let mut chunks = 0usize;
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let (output, stats) =
            black_box(generate_incrementally(ueg, target).expect("generate target"));
        samples.push(started.elapsed().as_micros() as u64);
        bytes = stats.bytes_emitted + target.preamble().len();
        chunks = stats.chunks_emitted;
        assert_eq!(output.len(), bytes, "reported output bytes must match");
    }
    TargetMetrics {
        target: target.label(),
        chunks,
        bytes,
        generation_p50_us: percentile(&samples, 0.50),
        generation_p95_us: percentile(&samples, 0.95),
        generation_max_us: *samples.iter().max().unwrap_or(&0),
    }
}

fn percentile(samples: &[u64], quantile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let position = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[position.min(sorted.len() - 1)]
}
