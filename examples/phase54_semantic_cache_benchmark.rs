use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::{validate_ueg_for_target, TargetCapabilityProfile};
use un1c0::semantic_cache::SemanticValidationCache;
use un1c0::walker::{python_to_ueg, Ueg};

const SAMPLES: usize = 128;
const CACHE_CAPACITY: usize = 8;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    uncached_p50_ns: u128,
    uncached_p95_ns: u128,
    uncached_p99_ns: u128,
    cached_p50_ns: u128,
    cached_p95_ns: u128,
    cached_p99_ns: u128,
    key_p50_ns: u128,
    key_p95_ns: u128,
    key_p99_ns: u128,
    p50_speedup_x1000: u128,
    expression_count: usize,
    diagnostics: usize,
    valid_samples: usize,
    cache_capacity: usize,
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
        let source = fixture(functions);
        let ueg = parse(&source);
        for target in TargetBinding::ALL {
            let uncached = measure(SAMPLES, || {
                validate_ueg_for_target(&ueg, target);
            });
            let cache = SemanticValidationCache::new(CACHE_CAPACITY).expect("cache");
            let profile = TargetCapabilityProfile::for_target(target);
            let key = cache.key_for(&ueg, &profile);
            let warm_report = cache.validate_with_key(&ueg, &profile, key);
            let cached = measure(SAMPLES, || {
                cache.validate_with_key(&ueg, &profile, key);
            });
            let key_timings = measure(SAMPLES, || {
                cache.key_for(&ueg, &profile);
            });
            let metrics = cache.metrics();
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                uncached_p50_ns: percentile(&uncached, 0.50),
                uncached_p95_ns: percentile(&uncached, 0.95),
                uncached_p99_ns: percentile(&uncached, 0.99),
                cached_p50_ns: percentile(&cached, 0.50),
                cached_p95_ns: percentile(&cached, 0.95),
                cached_p99_ns: percentile(&cached, 0.99),
                key_p50_ns: percentile(&key_timings, 0.50),
                key_p95_ns: percentile(&key_timings, 0.95),
                key_p99_ns: percentile(&key_timings, 0.99),
                p50_speedup_x1000: speedup_x1000(
                    percentile(&uncached, 0.50),
                    percentile(&cached, 0.50),
                ),
                expression_count: warm_report.expression_count,
                diagnostics: warm_report.diagnostics.len(),
                valid_samples: SAMPLES + 1,
                cache_capacity: metrics.capacity,
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

fn speedup_x1000(uncached: u128, cached: u128) -> u128 {
    if cached == 0 {
        return 0;
    }
    uncached.saturating_mul(1000) / cached
}
