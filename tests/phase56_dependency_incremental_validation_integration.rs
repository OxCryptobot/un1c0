use std::collections::BTreeSet;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::incremental_semantic::{
    DependencyAwareSemanticValidator, DependencyGraph, DependencyGraphError,
    IncrementalValidationError,
};
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_cache::SemanticValidationCache;
use un1c0::walker::{python_to_ueg, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn fixture(leaf_expression: &str) -> String {
    format!(
        "def leaf(value: int) -> int:\n    return {leaf_expression}\n\ndef middle(value: int) -> int:\n    return leaf(value)\n\ndef root(value: int) -> int:\n    return middle(value)\n\ndef unrelated(value: int) -> int:\n    return value + 7\n"
    )
}

#[test]
fn dependency_graph_collects_reverse_callers_and_is_bounded() {
    let ueg = parse_ueg(&fixture("value + 1"));
    let graph = DependencyGraph::from_ueg(&ueg).expect("unique function graph");
    assert_eq!(
        graph.function_names(),
        ["leaf", "middle", "root", "unrelated"]
    );
    assert_eq!(
        graph.dependencies_for(1).expect("middle deps"),
        &BTreeSet::from([0])
    );
    assert_eq!(
        graph.dependencies_for(2).expect("root deps"),
        &BTreeSet::from([1])
    );
    assert_eq!(
        graph.dependencies_for(3).expect("unrelated deps"),
        &BTreeSet::new()
    );
    assert_eq!(
        graph
            .affected_by_changed(&BTreeSet::from([0]))
            .expect("affected closure"),
        BTreeSet::from([0, 1, 2])
    );
}

#[test]
fn changed_leaf_reuses_unchanged_callers_but_preserves_conservative_closure() {
    let base = parse_ueg(&fixture("value + 1"));
    let changed = parse_ueg(&fixture("value + z"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let cache = SemanticValidationCache::new(8).expect("fingerprint cache");
    let base_fingerprint = cache.fingerprint_for(&base, &profile);
    let changed_fingerprint = cache.fingerprint_for(&changed, &profile);
    let mut validator = DependencyAwareSemanticValidator::new(8);

    let all = BTreeSet::from([0, 1, 2, 3]);
    let initial = validator
        .validate(&base, profile.clone(), &base_fingerprint, &all)
        .expect("initial validation");
    assert!(initial.report.is_valid());
    assert_eq!(initial.revalidated_functions, all);
    assert_eq!(initial.cache_misses, 4);

    let leaf_only = BTreeSet::from([0]);
    let changed_report = validator
        .validate(&changed, profile, &changed_fingerprint, &leaf_only)
        .expect("changed validation");
    assert_eq!(changed_report.affected_functions, BTreeSet::from([0, 1, 2]));
    assert_eq!(changed_report.revalidated_functions, BTreeSet::from([0]));
    assert_eq!(changed_report.cache_misses, 1);
    assert_eq!(changed_report.cache_hits, 2);
    assert!(!changed_report.report.is_valid());
    assert_eq!(
        changed_report.report.diagnostics[0].code,
        "UEG-UNDEFINED-NAME"
    );
}

#[test]
fn duplicate_functions_and_fingerprint_shape_fail_closed() {
    let duplicate = parse_ueg(
        "def same(value: int) -> int:\n    return value\n\ndef same(value: int) -> int:\n    return value + 1\n",
    );
    assert!(matches!(
        DependencyGraph::from_ueg(&duplicate),
        Err(DependencyGraphError::DuplicateFunction { .. })
    ));

    let valid = parse_ueg(&fixture("value + 1"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let cache = SemanticValidationCache::new(4).expect("cache");
    let fingerprint = cache.fingerprint_for(&valid, &profile);
    let mut validator = DependencyAwareSemanticValidator::new(4);
    let empty_ueg = parse_ueg("def only(value: int) -> int:\n    return value\n");
    let result = validator.validate(&empty_ueg, profile, &fingerprint, &BTreeSet::from([0]));
    assert!(matches!(
        result,
        Err(IncrementalValidationError::Dependency(
            DependencyGraphError::FingerprintShapeMismatch { .. }
        ))
    ));
}
