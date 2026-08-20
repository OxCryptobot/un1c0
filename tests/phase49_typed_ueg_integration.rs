use tree_sitter::Parser as TsParser;
use un1c0::walker::{python_to_ueg, DiagnosticSeverity, NodeKind, StatementKind, Ueg};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

#[test]
fn parser_builds_multiple_typed_functions_with_exact_boundaries() {
    let source = "def first(x: int) -> int:\n    if x > 0:\n        return x\n\ndef second(values: List[str]):\n    for item in range(1, 3):\n        print(item)\n";
    let ueg = parse_ueg(source);

    assert!(ueg.validate());
    assert_eq!(ueg.nodes.len(), 2);
    assert!(ueg.diagnostics.is_empty());

    let first = match &ueg.nodes[0] {
        NodeKind::Lambda(lambda) => lambda,
    };
    assert_eq!(first.name, "first");
    assert_eq!(first.params, vec![("x".into(), "i32".into())]);
    assert_eq!(first.ret.as_deref(), Some("i32"));
    assert!(first
        .orig_body
        .iter()
        .all(|line| !line.starts_with("def second")));
    assert_eq!(
        &source[first.source_span.start_byte..first.source_span.end_byte],
        "def first(x: int) -> int:\n    if x > 0:\n        return x"
    );
    assert_eq!(first.statements.len(), 2);
    assert!(matches!(
        first.statements[0].kind,
        StatementKind::If { ref condition } if condition == "x > 0"
    ));
    assert!(matches!(
        first.statements[1].kind,
        StatementKind::Return { ref expression } if expression == "x"
    ));
    let return_statement = &first.statements[1];
    assert_eq!(
        &source[return_statement.span.start_byte..return_statement.span.end_byte],
        "return x"
    );
    assert_eq!(return_statement.span.start_line, 3);
    assert_eq!(return_statement.span.start_column, 8);

    let second = match &ueg.nodes[1] {
        NodeKind::Lambda(lambda) => lambda,
    };
    assert_eq!(second.name, "second");
    assert_eq!(
        second.params,
        vec![("values".into(), "List<String>".into())]
    );
    assert_eq!(second.ret, None);
    assert!(second
        .orig_body
        .iter()
        .all(|line| !line.starts_with("def first")));
    assert!(matches!(
        second.statements[0].kind,
        StatementKind::RangeLoop {
            ref target,
            ref start,
            ref end,
            inclusive: false
        } if target == "item" && start == "1" && end == "3"
    ));
    assert!(matches!(
        second.statements[1].kind,
        StatementKind::Print { ref expression } if expression == "item"
    ));

    let rust = un1c0::walker::lower_to_rust(&ueg);
    assert!(rust.contains("fn first("));
    assert!(rust.contains("fn second("));
}

#[test]
fn unsupported_statements_emit_structured_error_diagnostics_and_fail_closed() {
    let source = "def unsafe_case(value: int):\n    while value > 0:\n        value -= 1\n";
    let ueg = parse_ueg(source);

    assert!(!ueg.validate());
    assert_eq!(ueg.nodes.len(), 1);
    assert_eq!(ueg.diagnostics.len(), 2);
    assert!(ueg.diagnostics.iter().all(|diagnostic| {
        diagnostic.code == "UEG-UNSUPPORTED-STATEMENT"
            && diagnostic.severity == DiagnosticSeverity::Error
    }));

    let first = match &ueg.nodes[0] {
        NodeKind::Lambda(lambda) => lambda,
    };
    assert_eq!(first.diagnostics.len(), 2);
    assert!(matches!(
        first.statements[0].kind,
        StatementKind::Unsupported { ref source } if source == "while value > 0:"
    ));
    assert_eq!(first.diagnostics[0].span.start_line, 2);
    assert_eq!(
        &source[first.diagnostics[0].span.start_byte..first.diagnostics[0].span.end_byte],
        "while value > 0:"
    );

    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    let rust = un1c0::walker::python_to_rust(&tree.root_node(), source.as_bytes());
    assert_eq!(rust, "// invalid UEG generated");
}
