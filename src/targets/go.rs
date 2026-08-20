use crate::walker::Ueg;
// Removed duplicate import of Ueg

// Very small Go emitter scaffold. Produces a Go function signature and body
// from a Lambda node; body lines are emitted as comments or simple translations.

pub fn lower_to_go(ueg: &Ueg) -> String {
    let mut out = String::new();
    out.push_str("package main\n\n");
    out.push_str("import \"fmt\"\n\n");
    for n in &ueg.nodes {
        let crate::walker::NodeKind::Lambda(l) = n;
        // Use normalized annotation forms for param types
        let mut params = String::new();
        {
            params.clear();
            for (i, (pn, pt)) in l.params.iter().enumerate() {
                if i > 0 {
                    params.push_str(", ");
                }
                let norm = crate::types::normalize_annotation(pt);
                params.push_str(pn);
                params.push(' ');
                params.push_str(&map_type_go(&norm));
            }
        }
        let ret = l
            .ret
            .as_ref()
            .map(|r| map_type_go(&crate::types::normalize_annotation(r)));
        if let Some(rty) = ret.as_ref() {
            out.push_str(&format!("func {}({}) {} {{\n", l.name, params, rty));
        } else {
            out.push_str(&format!("func {}({}) {{\n", l.name, params));
        }
        for line in &l.body {
            let mut go_line = line.clone();
            // simple translations
            // handle simple if-block openings
            if go_line.trim_end().ends_with("{") && go_line.trim_start().starts_with("if ") {
                // emit as-is (Rust-like condition should be valid in Go for simple expressions)
                out.push_str(&format!("    {}\n", go_line));
                continue;
            }
            // handle simple range loops produced by the translator: `for _ in a..=b {`
            if go_line.trim_start().starts_with("for _ in ") && go_line.contains("..=") {
                // parse a and b naively
                if let Some(range) = go_line.split_whitespace().nth(3) {
                    if let Some((a, b)) = range.split_once("..=") {
                        let a = a.trim();
                        let b = b.trim().trim_end_matches('{');
                        out.push_str(&format!("    for i := {}; i <= {}; i++ {{\n", a, b));
                        continue;
                    }
                }
            }
            if go_line.starts_with("let mut ") || go_line.starts_with("let ") {
                if let Some(eq) = go_line.find('=') {
                    // extract name between `let` and `:` or `=`
                    let head = if go_line.starts_with("let mut ") {
                        &go_line[8..eq]
                    } else {
                        &go_line[4..eq]
                    };
                    let lhs = head.split(':').next().unwrap_or("").trim();
                    let rhs = go_line[eq + 1..].trim().trim_end_matches(';');
                    go_line = format!("{} := {}", lhs, rhs);
                }
            }
            // handle return statements
            if go_line.trim_start().starts_with("return ") {
                let expr = go_line
                    .trim_start()
                    .trim_start_matches("return ")
                    .trim()
                    .trim_end_matches(';');
                go_line = format!("return {}", expr);
            }
            // simple assignment without let/decl (keep as-is, may need declaration)
            if go_line.contains('=')
                && !go_line.starts_with("fmt.Println")
                && !go_line.starts_with("//")
            {
                let s = go_line.trim_end_matches(';').to_string();
                go_line = s;
            }
            if go_line.contains("println!(") {
                let inner = go_line
                    .splitn(2, ",")
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(")")
                    .trim();
                go_line = format!("fmt.Println({})", inner);
            }
            if go_line.trim() == "}" {
                continue;
            }
            out.push_str(&format!("    {}\n", go_line));
        }
        out.push_str("}\n");
    }
    out
}

fn map_type_go(ann: &str) -> String {
    // support simple generics like Vec<T>, Option<T>, HashMap<K,V>
    let a = ann.trim();
    if let Some((base, inner)) = split_generic(a) {
        match base.as_str() {
            "Vec" | "List" => return format!("[]{}", map_type_go(&inner)),
            "Option" | "Option::<_>" => return map_type_go(&inner),
            "HashMap" | "Map" | "Dict" => {
                if let Some((k, v)) = split_two(&inner) {
                    return format!("map[{}]{}", map_type_go(&k), map_type_go(&v));
                }
            }
            _ => {}
        }
    }
    match a {
        "i32" => "int".to_string(),
        "f64" => "float64".to_string(),
        "String" => "string".to_string(),
        "bool" => "bool".to_string(),
        other => other.to_string(),
    }
}

fn split_generic(s: &str) -> Option<(String, String)> {
    if let Some(start) = s.find('<') {
        if s.ends_with('>') {
            let base = s[..start].trim().to_string();
            let inner = s[start + 1..s.len() - 1].to_string();
            return Some((base, inner));
        }
    }
    if let Some(start) = s.find('[') {
        if s.ends_with(']') {
            let base = s[..start].trim().to_string();
            let inner = s[start + 1..s.len() - 1].to_string();
            return Some((base, inner));
        }
    }
    None
}

fn split_two(s: &str) -> Option<(String, String)> {
    // split top-level comma only (no nested parsing)
    let mut depth = 0usize;
    let mut idx = None;
    for (i, ch) in s.chars().enumerate() {
        match ch {
            '<' | '[' => depth += 1,
            '>' | ']' => {
                if depth > 0 {
                    depth -= 1
                }
            }
            ',' if depth == 0 => {
                idx = Some(i);
                break;
            }
            _ => {}
        }
    }
    if let Some(i) = idx {
        let a = s[..i].trim().to_string();
        let b = s[i + 1..].trim().to_string();
        Some((a, b))
    } else {
        None
    }
}
