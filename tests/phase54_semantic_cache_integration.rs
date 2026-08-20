use std::sync::Arc;
use std::thread;

use tree_sitter::Parser as TsParser;
use un1c0::codegen::{
    generate_incrementally, GenerationError, IncrementalCodeGenerator, TargetBinding,
};
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_cache::{SemanticCacheConfigError, SemanticValidationCache};
use un1c0::walker::{python_to_ueg, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn source(function: &str, expression: &str) -> String {
    format!("def {function}(value: int) -> int:\n    return {expression}\n")
}

#[test]
fn cache_rejects_zero_capacity_and_separates_target_profiles() {
    assert!(matches!(
        SemanticValidationCache::new(0),
        Err(SemanticCacheConfigError::ZeroCapacity)
    ));
    let cache = SemanticValidationCache::new(4).expect("cache");
    let ueg = parse_ueg(&source("main", "value + 1"));
    let rust_profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    let go_profile = TargetCapabilityProfile::for_target(TargetBinding::Go);
    assert_ne!(
        cache.key_for(&ueg, &rust_profile),
        cache.key_for(&ueg, &go_profile)
    );
    let mut restricted = rust_profile.clone();
    restricted.supports_calls = false;
    assert_ne!(
        cache.key_for(&ueg, &rust_profile),
        cache.key_for(&ueg, &restricted)
    );
}

#[test]
fn cache_hits_return_equivalent_reports_and_evict_oldest_entry() {
    let cache = SemanticValidationCache::new(1).expect("cache");
    let first = parse_ueg(&source("first", "value + 1"));
    let second = parse_ueg(&source("second", "value + 2"));
    let first_report = cache.validate_for_target(&first, TargetBinding::Rust);
    let repeated_report = cache.validate_for_target(&first, TargetBinding::Rust);
    assert_eq!(first_report, repeated_report);
    let _second_report = cache.validate_for_target(&second, TargetBinding::Rust);
    let _first_again = cache.validate_for_target(&first, TargetBinding::Rust);
    let metrics = cache.metrics();
    assert_eq!(metrics.capacity, 1);
    assert_eq!(metrics.entries, 1);
    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.misses, 3);
    assert_eq!(metrics.insertions, 3);
    assert_eq!(metrics.evictions, 2);
}

#[test]
fn cache_is_thread_safe_and_remains_bounded_under_concurrent_validation() {
    let cache = Arc::new(SemanticValidationCache::new(2).expect("cache"));
    let ueg = Arc::new(parse_ueg(&source("main", "value + 1")));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let cache = Arc::clone(&cache);
        let ueg = Arc::clone(&ueg);
        workers.push(thread::spawn(move || {
            cache.validate_for_target(&ueg, TargetBinding::Python)
        }));
    }
    let reports: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker result"))
        .collect();
    assert!(reports.windows(2).all(|window| window[0] == window[1]));
    let metrics = cache.metrics();
    assert_eq!(metrics.entries, 1);
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 7);
}

#[test]
fn cached_generation_preserves_fail_closed_semantic_validation() {
    let invalid = parse_ueg(&source("broken", "missing"));
    let cache = SemanticValidationCache::new(4).expect("cache");
    let mut generator = IncrementalCodeGenerator::with_semantic_cache(TargetBinding::Rust, cache);
    let error = generator
        .next_chunk(&invalid)
        .expect_err("undefined name must block cached generation");
    assert!(matches!(error, GenerationError::SemanticValidation { .. }));
    assert!(generate_incrementally(&invalid, TargetBinding::Rust).is_err());
}

#[test]
fn cache_profile_changes_recompute_target_specific_diagnostics() {
    let cache = SemanticValidationCache::new(4).expect("cache");
    let ueg = parse_ueg(&source("call", "max(value)"));
    let normal = cache.validate_for_target(&ueg, TargetBinding::Rust);
    assert!(normal.is_valid());
    let mut restricted = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    restricted.supports_calls = false;
    let rejected = cache.validate_with_profile(&ueg, &restricted);
    assert!(!rejected.is_valid());
    assert_eq!(rejected.diagnostics[0].code, "UEG-TARGET-UNSUPPORTED-CALL");
    let metrics = cache.metrics();
    assert_eq!(metrics.misses, 2);
    assert_eq!(metrics.entries, 2);
}
