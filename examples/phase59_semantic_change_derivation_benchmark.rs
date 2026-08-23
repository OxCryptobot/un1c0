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
    changed_functions: usize,
    unchanged_functions: usize,
    affected_functions: usize,
    revalidated_functions: usize,
    full_capture_p50_ns: u128,
    full_capture_p95_ns: u128,
    full_capture_p99_ns: u128,
    derive_p50_ns: u128,
    derive_p95_ns: u128,
    derive_p99_ns: u128,
    auto_refresh_p50_ns: u128,
    auto_refresh_p95_ns: u128,
    auto_refresh_p99_ns: u128,
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
            let mut evidence_session =
                SemanticSession::start(&base, profile.clone(), functions * 4)
                    .expect("valid base session");
            let changes = evidence_session
                .derive_change_set(&changed, &profile)
                .expect("derive change set");
            let derive = measure(SAMPLES, || {
                evidence_session
                    .derive_change_set(&changed, &profile)
                    .expect("repeatable derivation");
            });
            let refresh = measure_auto_refresh(SAMPLES, &base, &changed, &profile);
            let mut validation_session =
                SemanticSession::start(&base, profile.clone(), functions * 4)
                    .expect("valid evidence session");
            let evidence = validation_session
                .refresh_auto(&changed, &profile)
                .expect("valid auto refresh");
            rows.push(BenchmarkRow {
                functions,
                samples: SAMPLES,
                target: target.label(),
                changed_functions: changes.changed_functions.len(),
                unchanged_functions: changes.unchanged_functions.len(),
                affected_functions: evidence.validation.affected_functions.len(),
                revalidated_functions: evidence.validation.revalidated_functions.len(),
                full_capture_p50_ns: percentile(&full_capture, 0.50),
                full_capture_p95_ns: percentile(&full_capture, 0.95),
                full_capture_p99_ns: percentile(&full_capture, 0.99),
                derive_p50_ns: percentile(&derive, 0.50),
                derive_p95_ns: percentile(&derive, 0.95),
                derive_p99_ns: percentile(&derive, 0.99),
                auto_refresh_p50_ns: percentile(&refresh.samples, 0.50),
                auto_refresh_p95_ns: percentile(&refresh.samples, 0.95),
                auto_refresh_p99_ns: percentile(&refresh.samples, 0.99),
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

fn measure_auto_refresh(
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
        if session.refresh_auto(changed, profile).is_err() {
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

#[allow(dead_code)]
fn _keep_set_import_used(_: BTreeSet<usize>) {}
