use tree_sitter::Parser as TsParser;
use un1c0::types::normalize_annotation;
use un1c0::walker::{python_to_ueg, LambdaNode, NodeKind, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn first_lambda(ueg: &Ueg) -> &LambdaNode {
    match ueg.nodes.first().expect("one UEG node") {
        NodeKind::Lambda(lambda) => lambda,
    }
}

#[test]
fn annotation_normalization_matrix_is_deterministic_and_nested() {
    let cases = [
        ("", ""),
        (" int ", "i32"),
        ("float", "f64"),
        ("str", "String"),
        ("bool", "bool"),
        ("None", "Option::<_>"),
        ("i32", "i32"),
        ("String", "String"),
        ("Vec[int]", "Vec<i32>"),
        ("List[str]", "List<String>"),
        ("HashMap[str, Vec[int]]", "HashMap<String, Vec<i32>>"),
        ("Dict[str, int]", "Dict<String, i32>"),
        ("tuple[int, str]", "tuple<i32, String>"),
        ("int, str", "(i32, String)"),
        (
            "Pair[Vec[int], HashMap[str, float]]",
            "Pair<Vec<i32>, HashMap<String, f64>>",
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(
            normalize_annotation(input),
            expected,
            "annotation: {input:?}"
        );
        assert_eq!(
            normalize_annotation(&normalize_annotation(input)),
            expected,
            "normalization must be idempotent: {input:?}"
        );
    }
}

#[test]
fn annotation_normalization_preserves_unsupported_or_malformed_forms() {
    let cases = [
        "Callable[[int, str], bool]",
        "Vec[int",
        "Map[str, int",
        "custom.Type",
        "",
    ];

    for input in cases {
        let normalized = normalize_annotation(input);
        assert!(!normalized.contains('\0'));
        assert_eq!(
            normalize_annotation(&normalized),
            normalized,
            "malformed or unsupported annotation should remain stable: {input:?}"
        );
    }
}

#[test]
fn parser_captures_typed_signature_metadata_and_lowered_body() {
    let source = "# module comment\n@trace\n# function comment\ndef first(x: int, values: List[str], flag) -> Optional[int]:\n    if x > 0:\n        return x\n    print(values)\n\ndef second(y: float) -> float:\n    return y\n";
    let ueg = parse_ueg(source);
    let lambda = first_lambda(&ueg);

    assert!(ueg.validate());
    assert_eq!(lambda.name, "first");
    assert_eq!(
        lambda.params,
        vec![
            ("x".to_string(), "i32".to_string()),
            ("values".to_string(), "List<String>".to_string()),
            ("flag".to_string(), "_".to_string()),
        ]
    );
    assert_eq!(lambda.ret.as_deref(), Some("Optional<i32>"));
    assert_eq!(
        lambda.orig_body.first().map(String::as_str),
        Some("# module comment")
    );
    assert!(lambda.orig_body.iter().any(|line| line == "@trace"));
    assert!(lambda
        .orig_body
        .iter()
        .any(|line| line == "def first(x: int, values: List[str], flag) -> Optional[int]:"));
    assert!(!lambda
        .orig_body
        .iter()
        .any(|line| line.starts_with("def second(")));
    assert!(lambda.body.iter().any(|line| line == "if x > 0 {"));
    assert!(lambda.body.iter().any(|line| line == "    return x;"));
    assert!(lambda
        .body
        .iter()
        .any(|line| line == "println!(\"{}\", values);"));
    let fragment = lambda.ast_fragment.as_deref().expect("AST fragment");
    assert!(fragment.contains("\"name\": \"first\""));
    assert!(fragment.contains("\"ret\": \"Optional<i32>\""));
}

#[test]
fn parser_rejects_source_without_a_function_via_invalid_ueg() {
    let ueg = parse_ueg("print('not a function')\n");
    assert!(!ueg.validate());
    assert!(ueg.nodes.is_empty());
}
