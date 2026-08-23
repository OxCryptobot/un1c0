use tree_sitter::Parser as TsParser;
use un1c0::codegen::{GenerationError, IncrementalCodeGenerator, TargetBinding};
use un1c0::semantic::TargetCapabilityProfile;
use un1c0::semantic_snapshot::{SemanticSnapshotError, SemanticValidationSnapshot};
use un1c0::walker::{python_to_ueg, Ueg};

fn parse(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn valid_source(value: &str) -> String {
    format!("def compute(value: int) -> int:\n    return value + {value}\n")
}

#[test]
fn valid_snapshot_allows_emission_without_repeating_semantic_preflight() {
    let source = valid_source("1");
    let ueg = parse(&source);
    let snapshot = SemanticValidationSnapshot::capture(
        &ueg,
        TargetCapabilityProfile::for_target(TargetBinding::Rust),
    )
    .expect("valid snapshot");
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);
    let mut chunks = 0;
    let stats = generator
        .emit_remaining_with_snapshot(&ueg, &snapshot, |_chunk| {
            chunks += 1;
            Ok::<(), &'static str>(())
        })
        .expect("snapshot-bound emission");
    assert_eq!(chunks, 1);
    assert_eq!(stats.chunks_emitted, 1);
    assert_eq!(snapshot.report().error_count(), 0);
}

#[test]
fn stale_ueg_snapshot_fails_before_sink_execution() {
    let original = parse(&valid_source("1"));
    let changed = parse(&valid_source("9"));
    let snapshot = SemanticValidationSnapshot::capture(
        &original,
        TargetCapabilityProfile::for_target(TargetBinding::Rust),
    )
    .expect("valid snapshot");
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);
    let mut sink_calls = 0;
    let result = generator.emit_remaining_with_snapshot(&changed, &snapshot, |_chunk| {
        sink_calls += 1;
        Ok::<(), &'static str>(())
    });
    assert!(matches!(
        result,
        Err(GenerationError::ValidationSnapshot {
            target: TargetBinding::Rust,
            ..
        })
    ));
    assert_eq!(sink_calls, 0);
}

#[test]
fn target_profile_mismatch_fails_closed() {
    let ueg = parse(&valid_source("1"));
    let snapshot = SemanticValidationSnapshot::capture(
        &ueg,
        TargetCapabilityProfile::for_target(TargetBinding::Python),
    )
    .expect("valid Python snapshot");
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);
    let result = generator
        .emit_remaining_with_snapshot(&ueg, &snapshot, |_chunk| Ok::<(), &'static str>(()));
    assert!(matches!(
        result,
        Err(GenerationError::ValidationSnapshot {
            target: TargetBinding::Rust,
            ..
        })
    ));
}

#[test]
fn invalid_semantics_cannot_be_captured_as_a_valid_snapshot() {
    let ueg = parse("def compute(value: int) -> int:\n    return missing_name\n");
    assert!(matches!(
        SemanticValidationSnapshot::capture(
            &ueg,
            TargetCapabilityProfile::for_target(TargetBinding::Rust),
        ),
        Err(SemanticSnapshotError::ValidationFailed { .. })
    ));
}
