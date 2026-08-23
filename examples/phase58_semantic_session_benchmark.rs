use std::collections::BTreeSet;
use std::time::Instant;

use serde::Serialize;
use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_session::SemanticSession;
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
    full_capture_p50_ns: u128,
    full_capture_p95_ns: u128,
    full_capture_p99_ns: u128,
    warm_refresh_p50_ns: u128,
    warm_refresh_p95_ns: u128,
    warm_refresh_p99_ns: u128,
    affected_functions: usize,
    revalidated_functions: usize,
    cache_hits: u64,
    cache_misses: u64,
    refresh_errors: usize,
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
            let refresh = measure_refresh(SAMPLES, &base, &changed, &profile);
            let mut warm_session = SemanticSession::start(&base, profile.clone(), functions * 4)
                .expect("valid warm session");
            let evidence = warm_session
                .refresh(&changed, &BTreeSet::from([0]), &profile)
                .expect("valid warm refresh");
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                expression_count: evidence.snapshot.report().expression_count,
                full_capture_p50_ns: percentile(&full_capture, 0.50),
                full_capture_p95_ns: percentile(&full_capture, 0.95),
                full_capture_p99_ns: percentile(&full_capture, 0.99),
                warm_refresh_p50_ns: percentile(&refresh.samples, 0.50),
                warm_refresh_p95_ns: percentile(&refresh.samples, 0.95),
                warm_refresh_p99_ns: percentile(&refresh.samples, 0.99),
                affected_functions: evidence.validation.affected_functions.len(),
                revalidated_functions: evidence.validation.revalidated_functions.len(),
                cache_hits: evidence.validation.cache_hits,
                cache_misses: evidence.validation.cache_misses,
                refresh_errors: refresh.errors,
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

struct RefreshSamples {
    samples: Vec<u128>,
    errors: usize,
}

fn measure_refresh(
    samples: usize,
    base: &Ueg,
    changed: &Ueg,
    profile: &TargetCapabilityProfile,
) -> RefreshSamples {
    let mut values = Vec::with_capacity(samples);
    let mut errors = 0;
    for _ in 0..samples {
        let mut session = SemanticSession::start(base, profile.clone(), base.nodes.len() * 4)
            .expect("valid base session");
        let started = Instant::now();
        if session
            .refresh(changed, &BTreeSet::from([0]), profile)
            .is_err()
        {
            errors += 1;
        }
        values.push(started.elapsed().as_nanos());
    }
    values.sort_unstable();
    RefreshSamples {
        samples: values,
        errors,
    }
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
