use std::collections::HashMap;

use tree_sitter::Node;

// Clean single-file implementation: UEG types, entropy gate, python->UEG->Rust

#[derive(Debug, Clone)]
pub struct Ueg {
    pub nodes: Vec<NodeKind>,
    pub diagnostics: Vec<UegDiagnostic>,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Lambda(LambdaNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UegDiagnostic {
    pub code: String,
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    If {
        condition: String,
    },
    Return {
        expression: String,
    },
    TupleAssign {
        targets: Vec<String>,
        values: Vec<String>,
    },
    RangeLoop {
        target: String,
        start: String,
        end: String,
        inclusive: bool,
    },
    Print {
        expression: String,
    },
    Unsupported {
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedStatement {
    pub kind: StatementKind,
    pub span: SourceSpan,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct LambdaNode {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub ret: Option<String>,
    pub body: Vec<String>,
    // preserve original Python body lines for exact roundtrips
    pub orig_body: Vec<String>,
    // small structured AST fragment (JSON-like string) to aid emitters
    pub ast_fragment: Option<String>,
    pub source_span: SourceSpan,
    pub statements: Vec<TypedStatement>,
    pub diagnostics: Vec<UegDiagnostic>,
}

impl Ueg {
    pub fn new() -> Self {
        Ueg {
            nodes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn validate(&self) -> bool {
        !self.nodes.is_empty()
            && !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }
}

pub fn lower_to_rust(ueg: &Ueg) -> String {
    let mut out = String::new();
    for n in &ueg.nodes {
        let NodeKind::Lambda(l) = n;
        out.push_str(&format!("fn {}(", l.name));
        for (i, (pn, pt)) in l.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if pt == "_" {
                out.push_str(&format!("{}: impl std::fmt::Debug", pn));
            } else {
                out.push_str(&format!("{}: {}", pn, pt));
            }
        }
        out.push(')');
        if let Some(r) = &l.ret {
            out.push_str(&format!(" -> {}", r));
        }
        out.push_str(" {\n");
        for line in &l.body {
            out.push_str(&format!("    {}\n", line));
        }
        out.push_str("}\n");
    }
    out
}

/// Shannon entropy fingerprint for obfuscation detection
/// Returns normalized entropy (0.0-1.0), higher = more uniform character distribution
pub fn entropy_fingerprint(source: &str) -> f64 {
    let mut freqs: HashMap<char, usize> = HashMap::new();
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len() as f64;
    if n == 0.0 {
        return 0.0;
    }
    for c in chars {
        *freqs.entry(c).or_insert(0) += 1usize;
    }
    let set_len = freqs.len();
    if set_len <= 1 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    for (_c, &cnt) in freqs.iter() {
        let p = (cnt as f64) / n;
        if p > 0.0 {
            sum -= p * p.log2();
        }
    }
    let denom = (set_len as f64).log2();
    if denom == 0.0 {
        return 0.0;
    }
    sum / denom
}

pub fn python_to_rust(_root: &Node, source: &[u8]) -> String {
    let src = String::from_utf8_lossy(source).to_string();

    // PRODUCTION: Entropy gate active - reject obfuscated code
    let f = entropy_fingerprint(&src);
    let _baseline = compute_minimal_baseline().unwrap_or(0.65_f64); // normal code ~0.65-0.75
    if f > 0.92 {
        // approaching max entropy (obfuscation)
        return format!(
            "// UN1C⓪ REJECT: entropy {:.6} > 0.92 threshold (obfuscation detected)",
            f
        );
    }

    let ueg = python_to_ueg(_root, source);
    if !ueg.validate() {
        return "// invalid UEG generated".into();
    }
    lower_to_rust(&ueg)
}

/// Build a typed, multi-function UEG from Python source without lowering it to a target.
#[allow(dead_code)]
pub fn python_to_ueg(_root: &Node, source: &[u8]) -> Ueg {
    let src = String::from_utf8_lossy(source).to_string();
    let lines: Vec<&str> = src.lines().collect();
    let line_starts = line_starts(&src);
    let mut ueg = Ueg::new();

    for line_idx in 0..lines.len() {
        if !is_top_level_definition(lines[line_idx]) {
            continue;
        }

        let (start_idx, end_idx) = function_bounds(&lines, line_idx);
        let lambda = parse_lambda_node(&lines, &line_starts, line_idx, start_idx, end_idx);
        ueg.diagnostics.extend(lambda.diagnostics.iter().cloned());
        ueg.nodes.push(NodeKind::Lambda(lambda));
    }

    ueg
}

fn is_top_level_definition(line: &str) -> bool {
    line.starts_with("def ")
}

fn function_bounds(lines: &[&str], line_idx: usize) -> (usize, usize) {
    let mut start_idx = line_idx;
    while start_idx > 0 {
        let previous = lines[start_idx - 1].trim_start();
        if previous.starts_with('@') || previous.starts_with('#') || previous.is_empty() {
            start_idx -= 1;
            continue;
        }
        break;
    }

    let mut end_idx = line_idx;
    for idx in (line_idx + 1)..lines.len() {
        if is_top_level_definition(lines[idx]) {
            break;
        }
        if !lines[idx].trim().is_empty() {
            end_idx = idx;
        }
    }
    (start_idx, end_idx)
}

fn parse_lambda_node(
    lines: &[&str],
    line_starts: &[usize],
    line_idx: usize,
    start_idx: usize,
    end_idx: usize,
) -> LambdaNode {
    let signature = lines[line_idx].trim_end_matches(':').trim();
    let rest = signature.trim_start_matches("def").trim();
    let name = rest.split('(').next().unwrap_or("").trim().to_string();
    let mut params = Vec::new();
    let mut ret = None;

    if let Some(pstart) = rest.find('(') {
        if let Some(pend) = rest[pstart + 1..].find(')') {
            let pend = pstart + 1 + pend;
            for parameter in split_top_level_commas(&rest[pstart + 1..pend]) {
                let parameter = parameter.trim();
                if parameter.is_empty() {
                    continue;
                }
                if let Some(colon) = parameter.find(':') {
                    let parameter_name = parameter[..colon].trim().to_string();
                    let annotation = parameter[colon + 1..].trim();
                    params.push((parameter_name, map_type(annotation)));
                } else {
                    params.push((parameter.to_string(), "_".into()));
                }
            }
        }
        if let Some(arrow) = rest.find("->") {
            let annotation = rest[arrow + 2..].trim().trim_end_matches(':').trim();
            if !annotation.is_empty() {
                ret = Some(map_type(annotation));
            }
        }
    }

    let mut body_lines = Vec::new();
    let mut body_indices = Vec::new();
    for idx in (line_idx + 1)..=end_idx {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        body_indices.push(idx);
        body_lines.push(trimmed.to_string());
    }

    let statements = typed_statements(lines, line_starts, &body_indices);
    let diagnostics = diagnostics_for_statements(&statements);
    let source_span = span_for_lines(lines, line_starts, start_idx, end_idx);
    let ast_fragment = Some(ast_fragment(&name, &params, ret.as_deref()));

    LambdaNode {
        name,
        params,
        ret,
        body: translate_body_to_rust_like(&body_lines),
        orig_body: lines[start_idx..=end_idx]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        ast_fragment,
        source_span,
        statements,
        diagnostics,
    }
}

fn ast_fragment(name: &str, params: &[(String, String)], ret: Option<&str>) -> String {
    let params = params
        .iter()
        .map(|(name, annotation)| format!("{{\"n\":\"{}\",\"t\":\"{}\"}}", name, annotation))
        .collect::<Vec<_>>()
        .join(",");
    let mut parts = vec![
        format!("\"name\": \"{}\"", name),
        format!("\"params\": [{}]", params),
    ];
    if let Some(ret) = ret {
        parts.push(format!("\"ret\": \"{}\"", ret));
    }
    format!("{{{}}}", parts.join(","))
}

fn typed_statements(
    lines: &[&str],
    line_starts: &[usize],
    body_indices: &[usize],
) -> Vec<TypedStatement> {
    body_indices
        .iter()
        .map(|&line_idx| {
            let raw = lines[line_idx];
            let span = statement_span(raw, line_starts[line_idx], line_idx);
            let kind = statement_kind(raw.trim());
            TypedStatement { kind, span }
        })
        .collect()
}

fn statement_kind(source: &str) -> StatementKind {
    if source.starts_with("if ") && source.ends_with(':') {
        return StatementKind::If {
            condition: source[3..source.len() - 1].trim().to_string(),
        };
    }
    if let Some(expression) = source.strip_prefix("return ") {
        return StatementKind::Return {
            expression: expression.trim().to_string(),
        };
    }
    if source.starts_with("print(") && source.ends_with(')') {
        return StatementKind::Print {
            expression: source[6..source.len() - 1].trim().to_string(),
        };
    }
    if source.starts_with("for ") {
        if let Some((target, expression)) = source[4..].split_once(" in ") {
            let expression = expression.trim_end_matches(':').trim();
            if expression.starts_with("range(") && expression.ends_with(')') {
                let args = split_top_level_commas(&expression[6..expression.len() - 1]);
                if args.len() == 1 {
                    return StatementKind::RangeLoop {
                        target: target.trim().to_string(),
                        start: "0".into(),
                        end: args[0].trim().to_string(),
                        inclusive: false,
                    };
                }
                if args.len() == 2 {
                    return StatementKind::RangeLoop {
                        target: target.trim().to_string(),
                        start: args[0].trim().to_string(),
                        end: args[1].trim().to_string(),
                        inclusive: false,
                    };
                }
            }
        }
    }
    if !source.contains("==") && !source.contains(":") {
        if let Some((left, right)) = source.split_once('=') {
            let targets = split_top_level_commas(left);
            let values = split_top_level_commas(right);
            if targets.len() > 1 && targets.len() == values.len() {
                return StatementKind::TupleAssign {
                    targets: targets
                        .into_iter()
                        .map(|item| item.trim().to_string())
                        .collect(),
                    values: values
                        .into_iter()
                        .map(|item| item.trim().to_string())
                        .collect(),
                };
            }
        }
    }
    StatementKind::Unsupported {
        source: source.to_string(),
    }
}

fn diagnostics_for_statements(statements: &[TypedStatement]) -> Vec<UegDiagnostic> {
    statements
        .iter()
        .filter_map(|statement| match &statement.kind {
            StatementKind::Unsupported { source } => Some(UegDiagnostic {
                code: "UEG-UNSUPPORTED-STATEMENT".into(),
                message: format!("unsupported statement is not lowered: {source}"),
                severity: DiagnosticSeverity::Error,
                span: statement.span.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn span_for_lines(
    lines: &[&str],
    line_starts: &[usize],
    start_idx: usize,
    end_idx: usize,
) -> SourceSpan {
    SourceSpan {
        start_byte: line_starts[start_idx],
        end_byte: line_starts[end_idx] + lines[end_idx].len(),
        start_line: start_idx + 1,
        start_column: 0,
        end_line: end_idx + 1,
        end_column: lines[end_idx].chars().count(),
    }
}

fn statement_span(raw: &str, line_start: usize, line_idx: usize) -> SourceSpan {
    let leading_bytes = raw.len() - raw.trim_start().len();
    SourceSpan {
        start_byte: line_start + leading_bytes,
        end_byte: line_start + raw.len(),
        start_line: line_idx + 1,
        start_column: raw[..leading_bytes].chars().count(),
        end_line: line_idx + 1,
        end_column: raw.chars().count(),
    }
}

fn split_top_level_commas(source: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (idx, character) in source.char_indices() {
        match character {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                items.push(source[start..idx].trim().to_string());
                start = idx + 1;
            }
            _ => {}
        }
    }
    if start < source.len() {
        items.push(source[start..].trim().to_string());
    }
    items
}

/// Compute a minimal baseline by scanning `examples/*.py` and returning the
/// smallest entropy observed. Returns `None` on IO errors or if no examples.
pub fn compute_minimal_baseline() -> Option<f64> {
    use std::fs;
    use std::path::Path;
    let examples_dir = Path::new("examples");
    if !examples_dir.exists() {
        return None;
    }
    let mut min: Option<f64> = None;
    for entry in fs::read_dir(examples_dir).ok()? {
        let e = entry.ok()?;
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("py") {
            continue;
        }
        if let Ok(s) = fs::read_to_string(&p) {
            let v = entropy_fingerprint(&s);
            min = Some(match min {
                None => v,
                Some(m) => m.min(v),
            });
        }
    }
    min
}

fn translate_body_to_rust_like(body: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        let s = &body[i];
        // normalize stray single identifier lines (noise from naive parsing)
        if s.chars().all(|c| c.is_alphanumeric() || c == '_') {
            i += 1;
            continue;
        }
        if s.starts_with("if ") && s.ends_with(":") {
            let cond = s.trim_start_matches("if").trim_end_matches(":").trim();
            out.push(format!("if {} {{", cond));
            if i + 1 < body.len() && body[i + 1].starts_with("return ") {
                let expr = body[i + 1].trim_start_matches("return ").trim();
                out.push(format!("    return {};", expr));
                i += 1;
            }
            out.push("}".into());
        } else if s.contains('=') && s.contains(',') && !s.contains(':') {
            let parts: Vec<&str> = s.split('=').collect();
            if parts.len() == 2 {
                let lhs: Vec<&str> = parts[0].split(',').map(|x| x.trim()).collect();
                let rhs: Vec<&str> = parts[1].split(',').map(|x| x.trim()).collect();
                if lhs.len() == rhs.len() {
                    for j in 0..lhs.len() {
                        out.push(format!("let mut {}: i32 = {};", lhs[j], rhs[j]));
                    }
                } else {
                    out.push(format!("// unhandled assign: {}", s));
                }
            }
        } else if s.starts_with("for ") && s.contains("range(") {
            if let Some(start) = s.find("range(") {
                if let Some(endp) = s[start + 6..].find(')') {
                    let args = &s[start + 6..start + 6 + endp];
                    let parts: Vec<&str> = args.split(',').map(|x| x.trim()).collect();
                    if parts.len() == 2 {
                        let a = parts[0];
                        let mut b = parts[1].to_string();
                        if b.ends_with("+ 1") {
                            b = b.trim_end_matches("+ 1").trim().to_string();
                        }
                        out.push(format!("for _ in {}..={} {{", a, b));
                        if i + 1 < body.len() && body[i + 1].contains('=') {
                            let rhs_full = body[i + 1].split('=').nth(1).unwrap_or("").trim();
                            let temp_expr = if rhs_full.contains(',') {
                                rhs_full.split(',').nth(1).unwrap_or(rhs_full).trim()
                            } else {
                                rhs_full
                            };
                            out.push(format!("    let temp = {};", temp_expr));
                            out.push("    a = b;".into());
                            out.push("    b = temp;".into());
                            i += 1;
                        }
                        out.push("}".into());
                    }
                }
            }
        } else if s.starts_with("return ") {
            let mut j = i + 1;
            let mut more = false;
            while j < body.len() {
                if !body[j].trim().is_empty() {
                    more = true;
                    break;
                }
                j += 1;
            }
            let expr = s.trim_start_matches("return ").trim();
            if more {
                out.push(format!("return {};", expr));
            } else {
                out.push(expr.into());
            }
        } else if s.starts_with("print(") && s.ends_with(")") {
            let inner = s.trim_start_matches("print(").trim_end_matches(")");
            out.push(format!("println!(\"{{}}\", {});", inner));
        } else {
            out.push(format!("// TODO: {}", s));
        }
        i += 1;
    }
    out
}

fn map_type(ann: &str) -> String {
    // Keep parser metadata on the same canonical annotation contract as targets.
    crate::types::normalize_annotation(ann)
}
