use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};

use crate::codegen::TargetBinding;
use crate::walker::{
    BinaryOperator, DiagnosticSeverity, ExpressionKind, NodeKind, SourceSpan, StatementKind,
    TypedExpression, Ueg, UnaryOperator,
};

const BUILTINS: &[&str] = &[
    "abs", "bool", "float", "int", "len", "max", "min", "print", "range", "str", "sum",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCapabilityProfile {
    pub target: TargetBinding,
    pub supports_calls: bool,
    pub supports_tuples: bool,
    pub supports_strings: bool,
    pub supports_booleans: bool,
    pub supports_floats: bool,
    pub supported_unary_operators: Vec<UnaryOperator>,
    pub supported_binary_operators: Vec<BinaryOperator>,
}

impl TargetCapabilityProfile {
    pub fn for_target(target: TargetBinding) -> Self {
        let unary = vec![
            UnaryOperator::Not,
            UnaryOperator::Negate,
            UnaryOperator::Positive,
        ];
        let binary = vec![
            BinaryOperator::Add,
            BinaryOperator::Subtract,
            BinaryOperator::Multiply,
            BinaryOperator::Divide,
            BinaryOperator::Modulo,
            BinaryOperator::Equal,
            BinaryOperator::NotEqual,
            BinaryOperator::Less,
            BinaryOperator::LessEqual,
            BinaryOperator::Greater,
            BinaryOperator::GreaterEqual,
            BinaryOperator::And,
            BinaryOperator::Or,
        ];
        Self {
            target,
            supports_calls: true,
            supports_tuples: true,
            supports_strings: true,
            supports_booleans: true,
            supports_floats: true,
            supported_unary_operators: unary,
            supported_binary_operators: binary,
        }
    }

    pub fn supports_unary(&self, operator: &UnaryOperator) -> bool {
        self.supported_unary_operators.contains(operator)
    }

    pub fn supports_binary(&self, operator: &BinaryOperator) -> bool {
        self.supported_binary_operators.contains(operator)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticDiagnostic {
    pub code: String,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub target: TargetBinding,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticValidationReport {
    pub target: TargetBinding,
    pub function_count: usize,
    pub expression_count: usize,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

impl SemanticValidationReport {
    pub fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            .count()
    }
}

pub fn validate_ueg_for_target(ueg: &Ueg, target: TargetBinding) -> SemanticValidationReport {
    validate_ueg_with_profile(ueg, TargetCapabilityProfile::for_target(target))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticFunctionValidationError {
    FunctionIndexOutOfBounds { index: usize, function_count: usize },
}

impl Display for SemanticFunctionValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FunctionIndexOutOfBounds {
                index,
                function_count,
            } => write!(
                formatter,
                "semantic function index {index} is outside {function_count} functions"
            ),
        }
    }
}

impl std::error::Error for SemanticFunctionValidationError {}

pub fn validate_ueg_with_profile(
    ueg: &Ueg,
    profile: TargetCapabilityProfile,
) -> SemanticValidationReport {
    let target = profile.target;
    let mut diagnostics = Vec::new();
    let mut functions = BTreeSet::new();
    let mut function_spans = Vec::new();

    for node in &ueg.nodes {
        let NodeKind::Lambda(lambda) = node;
        if !functions.insert(lambda.name.clone()) {
            diagnostics.push(diagnostic(
                "UEG-DUPLICATE-FUNCTION",
                format!("function `{}` is declared more than once", lambda.name),
                target,
                lambda.source_span.clone(),
            ));
        }
        function_spans.push(lambda.name.clone());
    }

    let mut expression_count = 0;
    for node in &ueg.nodes {
        let NodeKind::Lambda(lambda) = node;
        let mut symbols = BTreeSet::new();
        for (name, _) in &lambda.params {
            if !symbols.insert(name.clone()) {
                diagnostics.push(diagnostic(
                    "UEG-DUPLICATE-PARAMETER",
                    format!("parameter `{name}` is declared more than once"),
                    target,
                    lambda.source_span.clone(),
                ));
            }
        }
        for statement in &lambda.statements {
            validate_statement(
                statement,
                &mut symbols,
                &functions,
                &profile,
                &mut expression_count,
                &mut diagnostics,
            );
        }
    }

    diagnostics.sort_by(|left, right| {
        (
            left.span.start_byte,
            left.span.end_byte,
            left.code.as_str(),
            left.message.as_str(),
        )
            .cmp(&(
                right.span.start_byte,
                right.span.end_byte,
                right.code.as_str(),
                right.message.as_str(),
            ))
    });

    SemanticValidationReport {
        target,
        function_count: function_spans.len(),
        expression_count,
        diagnostics,
    }
}

pub fn validate_function_with_profile(
    ueg: &Ueg,
    function_index: usize,
    profile: TargetCapabilityProfile,
) -> Result<SemanticValidationReport, SemanticFunctionValidationError> {
    let Some(node) = ueg.nodes.get(function_index) else {
        return Err(SemanticFunctionValidationError::FunctionIndexOutOfBounds {
            index: function_index,
            function_count: ueg.nodes.len(),
        });
    };
    let NodeKind::Lambda(lambda) = node;
    let functions = ueg
        .nodes
        .iter()
        .map(|node| {
            let NodeKind::Lambda(lambda) = node;
            lambda.name.clone()
        })
        .collect::<BTreeSet<_>>();
    let mut symbols = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for (name, _) in &lambda.params {
        if !symbols.insert(name.clone()) {
            diagnostics.push(diagnostic(
                "UEG-DUPLICATE-PARAMETER",
                format!("parameter `{name}` is declared more than once"),
                profile.target,
                lambda.source_span.clone(),
            ));
        }
    }
    let mut expression_count = 0;
    for statement in &lambda.statements {
        validate_statement(
            statement,
            &mut symbols,
            &functions,
            &profile,
            &mut expression_count,
            &mut diagnostics,
        );
    }
    diagnostics.sort_by(|left, right| {
        (
            left.span.start_byte,
            left.span.end_byte,
            left.code.as_str(),
            left.message.as_str(),
        )
            .cmp(&(
                right.span.start_byte,
                right.span.end_byte,
                right.code.as_str(),
                right.message.as_str(),
            ))
    });
    Ok(SemanticValidationReport {
        target: profile.target,
        function_count: 1,
        expression_count,
        diagnostics,
    })
}

fn validate_statement(
    statement: &crate::walker::TypedStatement,
    symbols: &mut BTreeSet<String>,
    functions: &BTreeSet<String>,
    profile: &TargetCapabilityProfile,
    expression_count: &mut usize,
    diagnostics: &mut Vec<SemanticDiagnostic>,
) {
    match &statement.kind {
        StatementKind::If { condition }
        | StatementKind::Return {
            expression: condition,
        }
        | StatementKind::Print {
            expression: condition,
        } => {
            validate_expression(
                condition,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
        }
        StatementKind::Assign { target, value } => {
            validate_expression(
                value,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
            if let ExpressionKind::Identifier { name } = &target.kind {
                symbols.insert(name.clone());
            } else {
                diagnostics.push(diagnostic(
                    "UEG-INVALID-ASSIGN-TARGET",
                    format!("assignment target `{}` is not an identifier", target.source),
                    profile.target,
                    target.span.clone(),
                ));
            }
            *expression_count += 1;
        }
        StatementKind::TupleAssign { targets, values } => {
            for value in values {
                validate_expression(
                    value,
                    symbols,
                    functions,
                    profile,
                    expression_count,
                    diagnostics,
                );
            }
            for target in targets {
                if let ExpressionKind::Identifier { name } = &target.kind {
                    symbols.insert(name.clone());
                } else {
                    diagnostics.push(diagnostic(
                        "UEG-INVALID-ASSIGN-TARGET",
                        format!("assignment target `{}` is not an identifier", target.source),
                        profile.target,
                        target.span.clone(),
                    ));
                }
                *expression_count += 1;
            }
        }
        StatementKind::RangeLoop {
            target, start, end, ..
        } => {
            validate_expression(
                start,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
            validate_expression(
                end,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
            if let ExpressionKind::Identifier { name } = &target.kind {
                symbols.insert(name.clone());
            } else {
                diagnostics.push(diagnostic(
                    "UEG-INVALID-LOOP-TARGET",
                    format!("loop target `{}` is not an identifier", target.source),
                    profile.target,
                    target.span.clone(),
                ));
            }
            *expression_count += 1;
        }
        StatementKind::Unsupported { source } => diagnostics.push(diagnostic(
            "UEG-UNSUPPORTED-STATEMENT",
            format!("unsupported statement is not lowered: {source}"),
            profile.target,
            statement.span.clone(),
        )),
    }
}

fn validate_expression(
    expression: &TypedExpression,
    symbols: &BTreeSet<String>,
    functions: &BTreeSet<String>,
    profile: &TargetCapabilityProfile,
    expression_count: &mut usize,
    diagnostics: &mut Vec<SemanticDiagnostic>,
) {
    *expression_count += 1;
    match &expression.kind {
        ExpressionKind::Identifier { name } => {
            if !symbols.contains(name)
                && !functions.contains(name)
                && !BUILTINS.contains(&name.as_str())
            {
                diagnostics.push(diagnostic(
                    "UEG-UNDEFINED-NAME",
                    format!("name `{name}` is not defined in the function or UEG"),
                    profile.target,
                    expression.span.clone(),
                ));
            }
        }
        ExpressionKind::Integer { .. } => {}
        ExpressionKind::Float { .. } if profile.supports_floats => {}
        ExpressionKind::Float { .. } => diagnostics.push(diagnostic(
            "UEG-TARGET-UNSUPPORTED-FLOAT",
            "target profile does not support float expressions".into(),
            profile.target,
            expression.span.clone(),
        )),
        ExpressionKind::String { .. } if profile.supports_strings => {}
        ExpressionKind::String { .. } => diagnostics.push(diagnostic(
            "UEG-TARGET-UNSUPPORTED-STRING",
            "target profile does not support string expressions".into(),
            profile.target,
            expression.span.clone(),
        )),
        ExpressionKind::Boolean { .. } if profile.supports_booleans => {}
        ExpressionKind::Boolean { .. } => diagnostics.push(diagnostic(
            "UEG-TARGET-UNSUPPORTED-BOOLEAN",
            "target profile does not support boolean expressions".into(),
            profile.target,
            expression.span.clone(),
        )),
        ExpressionKind::Unary { operator, operand } => {
            if !profile.supports_unary(operator) {
                diagnostics.push(diagnostic(
                    "UEG-TARGET-UNSUPPORTED-UNARY",
                    format!("target profile does not support unary operator {operator:?}"),
                    profile.target,
                    expression.span.clone(),
                ));
            }
            validate_expression(
                operand,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
        }
        ExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            if !profile.supports_binary(operator) {
                diagnostics.push(diagnostic(
                    "UEG-TARGET-UNSUPPORTED-BINARY",
                    format!("target profile does not support binary operator {operator:?}"),
                    profile.target,
                    expression.span.clone(),
                ));
            }
            validate_expression(
                left,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
            validate_expression(
                right,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
        }
        ExpressionKind::Call {
            function,
            arguments,
        } => {
            if !profile.supports_calls {
                diagnostics.push(diagnostic(
                    "UEG-TARGET-UNSUPPORTED-CALL",
                    "target profile does not support function calls".into(),
                    profile.target,
                    expression.span.clone(),
                ));
            }
            validate_expression(
                function,
                symbols,
                functions,
                profile,
                expression_count,
                diagnostics,
            );
            for argument in arguments {
                validate_expression(
                    argument,
                    symbols,
                    functions,
                    profile,
                    expression_count,
                    diagnostics,
                );
            }
        }
        ExpressionKind::Tuple { items } => {
            if !profile.supports_tuples {
                diagnostics.push(diagnostic(
                    "UEG-TARGET-UNSUPPORTED-TUPLE",
                    "target profile does not support tuple expressions".into(),
                    profile.target,
                    expression.span.clone(),
                ));
            }
            for item in items {
                validate_expression(
                    item,
                    symbols,
                    functions,
                    profile,
                    expression_count,
                    diagnostics,
                );
            }
        }
        ExpressionKind::Unsupported { source } => diagnostics.push(diagnostic(
            "UEG-UNSUPPORTED-EXPRESSION",
            format!("unsupported expression is not lowered: {source}"),
            profile.target,
            expression.span.clone(),
        )),
    }
}

fn diagnostic(
    code: &str,
    message: String,
    target: TargetBinding,
    span: SourceSpan,
) -> SemanticDiagnostic {
    SemanticDiagnostic {
        code: code.into(),
        message,
        severity: DiagnosticSeverity::Error,
        target,
        span,
    }
}
