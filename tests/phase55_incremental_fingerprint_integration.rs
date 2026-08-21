use tree_sitter::Parser as TsParser;
use un1c0::codegen::TargetBinding;
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_cache::{
    SemanticCacheMetrics, SemanticFingerprintError, SemanticValidationCache,
};
use un1c0::walker::{python_to_ueg, NodeKind, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn function_source(second_expression: &str) -> String {
    format!(
        "def first(value: int) -> int:\n    return value + 1\n\ndef second(value: int) -> int:\n    return {second_expression}\n"
    )
}

fn lambda_at(ueg: &Ueg, index: usize) -> &un1c0::walker::LambdaNode {
    match &ueg.nodes[index] {
        NodeKind::Lambda(lambda) => lambda,
    }
}

#[test]
fn incremental_fingerprint_changes_only_the_modified_function_digest() {
    let base = parse_ueg(&function_source("value + 2"));
    let changed = parse_ueg(&function_source("value + 99"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let base_fingerprint = SemanticValidationCache::new(2)
        .expect("cache")
        .fingerprint_for(&base, &profile);
    let changed_fingerprint = SemanticValidationCache::new(2)
        .expect("cache")
        .fingerprint_for(&changed, &profile);

    assert_eq!(
        base_fingerprint.profile_key(),
        changed_fingerprint.profile_key()
    );
    assert_eq!(
        base_fingerprint.function_keys()[0],
        changed_fingerprint.function_keys()[0]
    );
    assert_ne!(
        base_fingerprint.function_keys()[1],
        changed_fingerprint.function_keys()[1]
    );
    assert_ne!(base_fingerprint.root_key(), changed_fingerprint.root_key());
}

#[test]
fn in_place_function_replacement_recomposes_the_same_root_key() {
    let base = parse_ueg(&function_source("value + 2"));
    let changed = parse_ueg(&function_source("value + 99"));
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Python);
    let cache = SemanticValidationCache::new(2).expect("cache");
    let mut fingerprint = cache.fingerprint_for(&base, &profile);
    let changed_fingerprint = cache.fingerprint_for(&changed, &profile);

    fingerprint
        .replace_function(1, lambda_at(&changed, 1))
        .expect("valid function index");
    assert_eq!(fingerprint.root_key(), changed_fingerprint.root_key());
    assert_eq!(
        fingerprint.function_keys(),
        changed_fingerprint.function_keys()
    );
    assert!(matches!(
        fingerprint.replace_function(2, lambda_at(&changed, 0)),
        Err(SemanticFingerprintError::FunctionIndexOutOfBounds {
            index: 2,
            function_count: 2
        })
    ));
}

#[test]
fn profile_and_function_order_changes_invalidate_the_root_key() {
    let base = parse_ueg(&function_source("value + 2"));
    let reordered = parse_ueg(
        "def second(value: int) -> int:\n    return value + 2\n\ndef first(value: int) -> int:\n    return value + 1\n",
    );
    let profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let mut restricted = profile.clone();
    restricted.supports_calls = false;
    let cache = SemanticValidationCache::new(2).expect("cache");
    let base_key = cache.key_for(&base, &profile);
    assert_ne!(base_key, cache.key_for(&reordered, &profile));
    assert_ne!(base_key, cache.key_for(&base, &restricted));
}

#[test]
fn changed_input_misses_cache_and_invalid_input_remains_fail_closed() {
    let valid = parse_ueg(&function_source("value + 2"));
    let invalid = parse_ueg(&function_source("missing_name"));
    let cache = SemanticValidationCache::new(4).expect("cache");
    assert!(cache
        .validate_for_target(&valid, TargetBinding::Rust)
        .is_valid());
    let invalid_report = cache.validate_for_target(&invalid, TargetBinding::Rust);
    assert!(!invalid_report.is_valid());
    assert_eq!(invalid_report.diagnostics[0].code, "UEG-UNDEFINED-NAME");
    let metrics: SemanticCacheMetrics = cache.metrics();
    assert_eq!(metrics.misses, 2);
    assert_eq!(metrics.hits, 0);
    assert_eq!(metrics.entries, 2);
    assert_eq!(metrics.evictions, 0);
}
