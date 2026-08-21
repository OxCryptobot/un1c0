use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_cache::{SemanticFingerprint, SemanticValidationCache};
use un1c0::walker::{python_to_ueg, LambdaNode, NodeKind, Ueg};

const SAMPLES: usize = 128;
const CACHE_CAPACITY: usize = 8;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    expression_count: usize,
    full_fingerprint_p50_ns: u128,
    full_fingerprint_p95_ns: u128,
    full_fingerprint_p99_ns: u128,
    incremental_update_p50_ns: u128,
    incremental_update_p95_ns: u128,
    incremental_update_p99_ns: u128,
    p50_speedup_x1000: u128,
    changed_report_diagnostics: usize,
    cache_entries: usize,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let mut rows = Vec::new();
    for &functions in FUNCTION_LEVELS {
        let base_source = fixture(functions, false);
        let changed_source = fixture(functions, true);
        let base = parse(&base_source);
        let changed = parse(&changed_source);
        for target in TargetBinding::ALL {
            let profile = TargetCapabilityProfile::for_target(target);
            let cache = SemanticValidationCache::new(CACHE_CAPACITY).expect("cache");
            let base_fingerprint = cache.fingerprint_for(&base, &profile);
            let changed_fingerprint = cache.fingerprint_for(&changed, &profile);
            let _base_report = cache.validate_with_fingerprint(&base, &profile, &base_fingerprint);
            let changed_report =
                cache.validate_with_fingerprint(&changed, &profile, &changed_fingerprint);
            let _warm_changed_report =
                cache.validate_with_fingerprint(&changed, &profile, &changed_fingerprint);
            let full = measure(SAMPLES, || {
                cache.fingerprint_for(&changed, &profile);
            });
            let incremental = measure_incremental(
                SAMPLES,
                &base_fingerprint,
                lambda_at(&changed, functions - 1),
            );
            let metrics = cache.metrics();
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                expression_count: changed_report.expression_count,
                full_fingerprint_p50_ns: percentile(&full, 0.50),
                full_fingerprint_p95_ns: percentile(&full, 0.95),
                full_fingerprint_p99_ns: percentile(&full, 0.99),
                incremental_update_p50_ns: percentile(&incremental, 0.50),
                incremental_update_p95_ns: percentile(&incremental, 0.95),
                incremental_update_p99_ns: percentile(&incremental, 0.99),
                p50_speedup_x1000: speedup_x1000(
                    percentile(&full, 0.50),
                    percentile(&incremental, 0.50),
                ),
                changed_report_diagnostics: changed_report.diagnostics.len(),
                cache_entries: metrics.entries,
                cache_hits: metrics.hits,
                cache_misses: metrics.misses,
                cache_evictions: metrics.evictions,
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

fn measure<F>(samples: usize, mut operation: F) -> Vec<u128>
where
    F: FnMut(),
{
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation();
        timings.push(started.elapsed().as_nanos());
    }
    timings.sort_unstable();
    timings
}

fn measure_incremental(
    samples: usize,
    base_fingerprint: &SemanticFingerprint,
    changed_lambda: &LambdaNode,
) -> Vec<u128> {
    let mut fingerprint = base_fingerprint.clone();
    let index = fingerprint.function_keys().len() - 1;
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        fingerprint
            .replace_function(index, changed_lambda)
            .expect("valid changed function index");
        timings.push(started.elapsed().as_nanos());
        fingerprint
            .replace_function(index, changed_lambda)
            .expect("valid changed function index");
    }
    timings.sort_unstable();
    timings
}

fn fixture(functions: usize, changed: bool) -> String {
    let mut source = String::new();
    for index in 0..functions {
        let value = if changed && index + 1 == functions {
            index + 1000
        } else {
            index
        };
        source.push_str(&format!(
            "def function_{index}(value: int) -> int:\n    result = value + {value}\n    return result\n\n"
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

fn lambda_at(ueg: &Ueg, index: usize) -> &LambdaNode {
    match &ueg.nodes[index] {
        NodeKind::Lambda(lambda) => lambda,
    }
}

fn percentile(sorted: &[u128], quantile: f64) -> u128 {
    let index = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[index]
}

fn speedup_x1000(full: u128, incremental: u128) -> u128 {
    if incremental == 0 {
        return 0;
    }
    full.saturating_mul(1000) / incremental
}
