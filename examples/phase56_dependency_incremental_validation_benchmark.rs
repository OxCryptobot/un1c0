use std::collections::BTreeSet;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::incremental_semantic::DependencyAwareSemanticValidator;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_cache::SemanticValidationCache;
use un1c0::walker::{python_to_ueg, Ueg};

const SAMPLES: usize = 64;
const FUNCTION_LEVELS: &[usize] = &[1, 2, 4, 8, 16, 32];

#[derive(Debug, Serialize)]
struct BenchmarkRow {
    functions: usize,
    samples: usize,
    target: &'static str,
    expression_count: usize,
    full_validation_p50_ns: u128,
    full_validation_p95_ns: u128,
    full_validation_p99_ns: u128,
    dependency_incremental_p50_ns: u128,
    dependency_incremental_p95_ns: u128,
    dependency_incremental_p99_ns: u128,
    affected_function_count: usize,
    revalidated_function_count: usize,
    cache_hits_per_sample: u64,
    cache_misses_per_sample: u64,
    diagnostics: usize,
    cluster_mutation_performed: bool,
    secret_material_recorded: bool,
}

fn main() {
    let mut rows = Vec::new();
    for &functions in FUNCTION_LEVELS {
        let base = parse(&fixture(functions, false));
        let changed = parse(&fixture(functions, true));
        for target in TargetBinding::ALL {
            let profile = TargetCapabilityProfile::for_target(target);
            let cache = SemanticValidationCache::new(functions + 4).expect("fingerprint cache");
            let base_fingerprint = cache.fingerprint_for(&base, &profile);
            let changed_fingerprint = cache.fingerprint_for(&changed, &profile);
            let all = (0..functions).collect::<BTreeSet<_>>();
            let changed_set = BTreeSet::from([0]);
            let full = measure(SAMPLES, || {
                let report = un1c0::semantic::validate_ueg_with_profile(&changed, profile.clone());
                assert!(report.is_valid());
            });
            let mut validator = DependencyAwareSemanticValidator::new(functions + 4);
            let initial = validator
                .validate(&base, profile.clone(), &base_fingerprint, &all)
                .expect("initial full population");
            assert!(initial.report.is_valid());
            let first_changed = validator
                .validate(
                    &changed,
                    profile.clone(),
                    &changed_fingerprint,
                    &changed_set,
                )
                .expect("first changed validation");
            assert!(first_changed.report.is_valid());
            let incremental = measure_with_reports(SAMPLES, || {
                validator
                    .validate(
                        &changed,
                        profile.clone(),
                        &changed_fingerprint,
                        &changed_set,
                    )
                    .expect("warm changed validation")
            });
            let last = incremental.last().expect("incremental report");
            assert!(last.1.report.is_valid());
            let metrics = validator.cache_metrics();
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                expression_count: first_changed.report.expression_count,
                full_validation_p50_ns: percentile(&full, 0.50),
                full_validation_p95_ns: percentile(&full, 0.95),
                full_validation_p99_ns: percentile(&full, 0.99),
                dependency_incremental_p50_ns: percentile(
                    &incremental
                        .iter()
                        .map(|(time, _)| *time)
                        .collect::<Vec<_>>(),
                    0.50,
                ),
                dependency_incremental_p95_ns: percentile(
                    &incremental
                        .iter()
                        .map(|(time, _)| *time)
                        .collect::<Vec<_>>(),
                    0.95,
                ),
                dependency_incremental_p99_ns: percentile(
                    &incremental
                        .iter()
                        .map(|(time, _)| *time)
                        .collect::<Vec<_>>(),
                    0.99,
                ),
                affected_function_count: first_changed.affected_functions.len(),
                revalidated_function_count: first_changed.revalidated_functions.len(),
                cache_hits_per_sample: first_changed.cache_hits,
                cache_misses_per_sample: first_changed.cache_misses,
                diagnostics: first_changed.report.diagnostics.len(),
                cluster_mutation_performed: false,
                secret_material_recorded: false,
            });
            let _ = metrics;
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

fn measure_with_reports<F>(
    samples: usize,
    mut operation: F,
) -> Vec<(
    u128,
    un1c0::incremental_semantic::IncrementalValidationReport,
)>
where
    F: FnMut() -> un1c0::incremental_semantic::IncrementalValidationReport,
{
    let mut values = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let report = operation();
        values.push((started.elapsed().as_nanos(), report));
    }
    values.sort_by_key(|(time, _)| *time);
    values
}

fn fixture(functions: usize, changed: bool) -> String {
    let mut source = String::new();
    for index in 0..functions {
        let body = if index == 0 {
            if changed {
                "value + 9".to_string()
            } else {
                "value + 1".to_string()
            }
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
