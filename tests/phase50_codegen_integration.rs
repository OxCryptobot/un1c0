use tree_sitter::Parser as TsParser;
use un1c0::codegen::{
    generate_incrementally, GenerationError, IncrementalCodeGenerator, TargetBinding,
};
use un1c0::walker::{python_to_ueg, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

#[test]
fn incremental_cursor_emits_each_function_once_in_source_order() {
    let source = "def first(value: int) -> int:\n    return value\n\ndef second(value: int) -> int:\n    return value + 1\n";
    let ueg = parse_ueg(source);
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);

    let first = generator
        .next_chunk(&ueg)
        .expect("first generation")
        .expect("first chunk");
    assert_eq!(first.node_index, 0);
    assert_eq!(first.function_name, "first");
    assert!(first.code.contains("fn first("));
    assert_eq!(generator.cursor(), 1);

    let second = generator
        .next_chunk(&ueg)
        .expect("second generation")
        .expect("second chunk");
    assert_eq!(second.node_index, 1);
    assert_eq!(second.function_name, "second");
    assert!(second.code.contains("fn second("));
    assert_eq!(generator.cursor(), 2);

    assert!(generator
        .next_chunk(&ueg)
        .expect("end generation")
        .is_none());
    assert!(generator
        .next_chunk(&ueg)
        .expect("stable end generation")
        .is_none());
}

#[test]
fn bounded_sink_receives_only_remaining_chunks_and_reports_bytes() {
    let source = "def first(value: int) -> int:\n    return value\n\ndef second(value: int) -> int:\n    return value + 1\n";
    let ueg = parse_ueg(source);
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Go);
    let first = generator.next_chunk(&ueg).expect("first chunk").unwrap();
    assert_eq!(first.function_name, "first");

    let mut names = Vec::new();
    let stats = generator
        .emit_remaining(&ueg, |chunk| {
            names.push(chunk.function_name);
            Ok::<(), &'static str>(())
        })
        .expect("remaining chunks");
    assert_eq!(names, vec!["second"]);
    assert_eq!(stats.target, TargetBinding::Go);
    assert_eq!(stats.chunks_emitted, 1);
    assert!(stats.bytes_emitted > 0);
}

#[test]
fn all_target_bindings_emit_multi_function_output_with_expected_headers() {
    let source = "def first(value: int) -> int:\n    return value\n\ndef second(value: List[str]):\n    print(value)\n";
    let ueg = parse_ueg(source);

    for target in TargetBinding::ALL {
        let (output, stats) = generate_incrementally(&ueg, target).expect("target generation");
        assert_eq!(stats.target, target);
        assert_eq!(stats.chunks_emitted, 2);
        assert_eq!(stats.bytes_emitted, output.len() - target.preamble().len());
        match target {
            TargetBinding::Rust => {
                assert!(output.contains("fn first("));
                assert!(output.contains("fn second("));
            }
            TargetBinding::Go => {
                assert!(output.starts_with("package main\n\nimport \"fmt\"\n\n"));
                assert!(output.contains("func first("));
                assert!(output.contains("func second("));
            }
            TargetBinding::Zig => {
                assert!(output.starts_with("const std = @import(\"std\");\n\n"));
                assert!(output.contains("pub fn first("));
                assert!(output.contains("pub fn second("));
            }
            TargetBinding::Python => {
                assert_eq!(output.matches("def first(").count(), 1);
                assert_eq!(output.matches("def second(").count(), 1);
            }
        }
    }
}

#[test]
fn invalid_ueg_rejects_incremental_generation_before_emitter_invocation() {
    let source = "def unsafe_case(value: int):\n    while value > 0:\n        value -= 1\n";
    let ueg = parse_ueg(source);
    for target in TargetBinding::ALL {
        let error = generate_incrementally(&ueg, target).expect_err("invalid UEG must fail closed");
        assert!(matches!(
            error,
            GenerationError::InvalidUeg {
                diagnostic_count: 2
            }
        ));
    }
}
