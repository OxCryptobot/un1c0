use tree_sitter::Parser as TsParser;
use un1c0::codegen::{IncrementalCodeGenerator, TargetBinding};
use un1c0::walker::{
    python_to_ueg, BinaryOperator, DiagnosticSeverity, ExpressionKind, NodeKind, StatementKind, Ueg,
};

fn parse_ueg(source: &str) -> Ueg {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    python_to_ueg(&tree.root_node(), source.as_bytes())
}

fn first_lambda(ueg: &Ueg) -> &un1c0::walker::LambdaNode {
    match ueg.nodes.first().expect("one UEG node") {
        NodeKind::Lambda(lambda) => lambda,
    }
}

#[test]
fn expressions_are_typed_serializable_and_source_spanned() {
    let source = "def calculate(value: int) -> int:\n    if value >= 1 and value < 10:\n        return max(value + 1, 2 * value)\n";
    let ueg = parse_ueg(source);
    assert!(ueg.validate());
    let lambda = first_lambda(&ueg);

    let condition = match &lambda.statements[0].kind {
        StatementKind::If { condition } => condition,
        other => panic!("unexpected statement: {other:?}"),
    };
    assert_eq!(condition.source, "value >= 1 and value < 10");
    assert_eq!(
        &source[condition.span.start_byte..condition.span.end_byte],
        "value >= 1 and value < 10"
    );
    assert!(matches!(
        condition.kind,
        ExpressionKind::Binary {
            operator: BinaryOperator::And,
            ..
        }
    ));

    let returned = match &lambda.statements[1].kind {
        StatementKind::Return { expression } => expression,
        other => panic!("unexpected statement: {other:?}"),
    };
    assert!(matches!(returned.kind, ExpressionKind::Call { .. }));
    assert_eq!(
        &source[returned.span.start_byte..returned.span.end_byte],
        "max(value + 1, 2 * value)"
    );

    let serialized = serde_json::to_string(&lambda.ast_fragment).expect("serialize typed AST");
    let restored: un1c0::walker::AstFragment =
        serde_json::from_str(&serialized).expect("deserialize typed AST");
    assert_eq!(restored, lambda.ast_fragment);
    assert_eq!(restored.statements.len(), 2);
}

#[test]
fn range_and_tuple_expressions_preserve_child_source_spans() {
    let source = "def unpack(value: int):\n    for item in range(1, value + 1):\n        print(item)\n    a, b = value, 2\n";
    let ueg = parse_ueg(source);
    assert!(ueg.validate());
    let lambda = first_lambda(&ueg);

    let range = match &lambda.statements[0].kind {
        StatementKind::RangeLoop {
            target, start, end, ..
        } => (target, start, end),
        other => panic!("unexpected statement: {other:?}"),
    };
    assert_eq!(range.0.source, "item");
    assert_eq!(range.1.source, "1");
    assert_eq!(range.2.source, "value + 1");
    assert_eq!(
        &source[range.2.span.start_byte..range.2.span.end_byte],
        "value + 1"
    );

    let assignment = match &lambda.statements[2].kind {
        StatementKind::TupleAssign { targets, values } => (targets, values),
        other => panic!("unexpected statement: {other:?}"),
    };
    assert_eq!(
        assignment
            .0
            .iter()
            .map(|e| e.source.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        assignment
            .1
            .iter()
            .map(|e| e.source.as_str())
            .collect::<Vec<_>>(),
        ["value", "2"]
    );
}

#[test]
fn unsupported_expression_is_diagnostic_and_generation_fails_closed() {
    let source = "def unsupported(value: int) -> int:\n    return value[0]\n# value value value value value value value value value value\n# value value value value value value value value value value\n";
    let ueg = parse_ueg(source);
    assert!(!ueg.validate());
    assert_eq!(ueg.diagnostics.len(), 1);
    let diagnostic = &ueg.diagnostics[0];
    assert_eq!(diagnostic.code, "UEG-UNSUPPORTED-EXPRESSION");
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Error);
    assert_eq!(
        &source[diagnostic.span.start_byte..diagnostic.span.end_byte],
        "value[0]"
    );
    assert!(un1c0::walker::lower_to_rust(&ueg).contains("fn unsupported(value: i32)"));
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("tree-sitter Python language");
    let tree = parser.parse(source, None).expect("parse Python source");
    assert_eq!(
        un1c0::walker::python_to_rust(&tree.root_node(), source.as_bytes()),
        "// invalid UEG generated"
    );
}

#[test]
fn typed_ast_drives_deterministic_emitter_hints() {
    let source = "def calculate(value: int) -> int:\n    if value >= 1:\n        return max(value + 1, 2 * value)\n";
    let ueg = parse_ueg(source);
    let mut generator = IncrementalCodeGenerator::new(TargetBinding::Rust);
    let chunk = generator
        .next_chunk(&ueg)
        .expect("generate chunk")
        .expect("chunk");
    assert_eq!(
        chunk.hints.source_span,
        match &ueg.nodes[0] {
            NodeKind::Lambda(lambda) => lambda.source_span.clone(),
        }
    );
    assert_eq!(chunk.hints.control_flow_sites, 1);
    assert_eq!(chunk.hints.call_sites, 1);
    assert!(chunk.hints.expression_nodes >= 8);
}

#[test]
fn typed_ast_contains_canonical_parameter_and_return_metadata() {
    let ueg = parse_ueg("def typed(value: List[int]) -> Optional[int]:\n    return value\n");
    let lambda = first_lambda(&ueg);
    assert_eq!(lambda.ast_fragment.params[0].annotation, "List<i32>");
    assert_eq!(lambda.ast_fragment.ret.as_deref(), Some("Optional<i32>"));
    assert_eq!(lambda.ast_fragment.source_span, lambda.source_span);
}
