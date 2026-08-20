use tree_sitter::Parser as TsParser;
use un1c0::codegen::{generate_incrementally, GenerationError, TargetBinding};
use un1c0::semantic::{validate_ueg_for_target, TargetCapabilityProfile};
use un1c0::walker::{python_to_ueg, DiagnosticSeverity, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

#[test]
fn semantic_validation_accepts_source_ordered_symbols_and_user_calls() {
    let source = "def helper(value: int) -> int:\n    return value + 1\n\ndef main(value: int) -> int:\n    result, extra = helper(value), 2\n    for item in range(0, result):\n        print(item)\n    return result\n";
    let ueg = parse_ueg(source);
    assert!(ueg.validate());

    for target in TargetBinding::ALL {
        let report = validate_ueg_for_target(&ueg, target);
        assert!(report.is_valid(), "{target:?}: {:?}", report.diagnostics);
        assert_eq!(report.function_count, 2);
        assert!(report.expression_count >= 12);
    }
}

#[test]
fn semantic_validation_rejects_undefined_names_and_duplicate_parameters_deterministically() {
    let source = "def broken(value: int, value: int) -> int:\n    return missing + value\n";
    let ueg = parse_ueg(source);
    let first = validate_ueg_for_target(&ueg, TargetBinding::Rust);
    let second = validate_ueg_for_target(&ueg, TargetBinding::Rust);
    assert_eq!(first, second);
    assert!(!first.is_valid());
    assert_eq!(first.error_count(), 2);
    assert_eq!(first.diagnostics[0].code, "UEG-DUPLICATE-PARAMETER");
    assert_eq!(first.diagnostics[1].code, "UEG-UNDEFINED-NAME");
    assert!(first
        .diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error));
    assert_eq!(first.diagnostics[1].span.start_line, 2);
    assert_eq!(
        &source[first.diagnostics[1].span.start_byte..first.diagnostics[1].span.end_byte],
        "missing"
    );
}

#[test]
fn target_capability_profile_rejects_disabled_features_with_exact_span() {
    let source = "def call(value: int) -> int:\n    return max(value)\n";
    let ueg = parse_ueg(source);
    assert!(ueg.validate());

    let mut profile = TargetCapabilityProfile::for_target(TargetBinding::Rust);
    profile.supports_calls = false;
    let report = un1c0::semantic::validate_ueg_with_profile(&ueg, profile);
    assert!(!report.is_valid());
    assert_eq!(report.diagnostics[0].code, "UEG-TARGET-UNSUPPORTED-CALL");
    assert_eq!(
        &source[report.diagnostics[0].span.start_byte..report.diagnostics[0].span.end_byte],
        "max(value)"
    );
}

#[test]
fn code_generation_fails_closed_before_target_emitter_on_semantic_errors() {
    let source = "def broken(value: int) -> int:\n    return missing\n";
    let ueg = parse_ueg(source);
    assert!(ueg.validate());
    for target in TargetBinding::ALL {
        let error =
            generate_incrementally(&ueg, target).expect_err("semantic error must block emit");
        assert!(matches!(error, GenerationError::SemanticValidation { .. }));
        assert!(error.to_string().contains("semantic validation errors"));
    }
}

#[test]
fn semantic_report_counts_nested_typed_expressions() {
    let source = "def nested(value: int) -> int:\n    return max(value + 1, abs(value - 2))\n";
    let ueg = parse_ueg(source);
    let report = validate_ueg_for_target(&ueg, TargetBinding::Python);
    assert!(report.is_valid());
    assert!(report.expression_count >= 8);
}
