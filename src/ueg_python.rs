use crate::walker::Ueg;

// Minimal UEG -> Python lowering stub for Day 1. Produces a human-readable
// Python function from a Lambda node. This is intentionally simple: it
// emits the stored body lines and parameter list.

pub fn lower_to_python(ueg: &Ueg) -> String {
    let mut out = String::new();
    for n in &ueg.nodes {
        let crate::walker::NodeKind::Lambda(l) = n;
        out.push_str(&format!("def {}(", l.name));
        for (i, (pn, pt)) in l.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            if pt != "_" {
                out.push_str(&format!("{}: {}", pn, map_type_python(pt)));
            } else {
                out.push_str(pn);
            }
        }
        if let Some(r) = &l.ret {
            out.push_str(&format!(") -> {}:", map_type_python(r)));
        } else {
            out.push_str(":");
        }
        out.push('\n');
        // If we preserved original body, use it for exact roundtrip (preserve indentation)
        if !l.orig_body.is_empty() {
            // compute minimal leading indent across non-empty lines
            let mut min_indent: Option<usize> = None;
            for line in &l.orig_body {
                if line.trim().is_empty() {
                    continue;
                }
                let count = line.chars().take_while(|c| *c == ' ').count();
                min_indent = Some(min_indent.map_or(count, |m| m.min(count)));
            }
            let min_indent = min_indent.unwrap_or(0);
            // The orig_body contains the header lines (decorators and def) and the full body.
            // We'll emit them verbatim but ensure the header isn't duplicated: if the first
            // non-empty orig line starts with "def ", emit orig_body as the entire function
            // (including decorators). Otherwise fall back to emitting the generated signature + orig body.
            let mut first_non_empty = None;
            for line in &l.orig_body {
                if !line.trim().is_empty() {
                    first_non_empty = Some(line);
                    break;
                }
            }
            if let Some(first) = first_non_empty {
                if first.trim_start().starts_with("def ") || first.trim_start().starts_with('@') {
                    // emit orig_body lines with preserved indentation shifted to 4 spaces
                    for line in &l.orig_body {
                        out.push_str("    ");
                        let stripped = if line.len() > min_indent {
                            line[min_indent..].to_string()
                        } else {
                            line.trim_start().to_string()
                        };
                        out.push_str(&stripped);
                        out.push('\n');
                    }
                    continue;
                } else {
                    // otherwise, emit generated signature and then the original body lines
                    for line in &l.orig_body {
                        out.push_str("    ");
                        let stripped = if line.len() > min_indent {
                            line[min_indent..].to_string()
                        } else {
                            line.trim_start().to_string()
                        };
                        out.push_str(&stripped);
                        out.push('\n');
                    }
                    continue;
                }
            }
        }
        // Heuristic re-conversion of the stored body lines back to Python
        for line in &l.body {
            out.push_str("    ");
            let mut python_line = line.clone();
            // let mut x: i32 = expr;  -> x = expr
            if python_line.starts_with("let mut ") {
                python_line = python_line.trim_start_matches("let mut ").to_string();
                if let Some(idx) = python_line.find(':') {
                    if let Some(eq) = python_line.find('=') {
                        let name = python_line[..idx].trim();
                        let rhs = python_line[eq + 1..].trim().trim_end_matches(';');
                        python_line = format!("{} = {}", name, rhs);
                    }
                }
            }
            // return expr; -> return expr
            if python_line.trim_start().starts_with("return ")
                && python_line.trim_end().ends_with(';')
            {
                python_line = python_line.trim_end_matches(';').to_string();
            }
            // println! -> print
            if python_line.contains("println!(") {
                python_line = python_line
                    .replace("println!(\"{ }\",", "print(")
                    .replace(");", ")");
            }
            // convert simple if/brace forms
            if python_line.trim_end().ends_with("{") && python_line.trim_start().starts_with("if ")
            {
                let cond = python_line
                    .trim()
                    .trim_end_matches('{')
                    .trim_start_matches("if")
                    .trim();
                python_line = format!("if {}:", cond);
            }
            // drop closing brace tokens
            if python_line.trim() == "}" {
                python_line.clear();
            }

            // Preserve docstrings that passed through as TODO comments
            if python_line.trim_start().starts_with("// TODO:") {
                let inner = python_line.trim_start_matches("// TODO:").trim();
                if inner.starts_with("\"\"\"") || inner.starts_with("'''") || inner.starts_with('"')
                {
                    out.push_str(inner);
                    out.push('\n');
                    continue;
                }
            }
            if !python_line.is_empty() {
                out.push_str(&python_line);
                out.push('\n');
            }
        }
    }
    out
}

fn map_type_python(ann: &str) -> String {
    match ann.trim() {
        "i32" => "int".to_string(),
        "f64" => "float".to_string(),
        "String" => "str".to_string(),
        "bool" => "bool".to_string(),
        "Option::<_>" => "Optional[Any]".to_string(),
        other => other.to_string(),
    }
}
