use crate::walker::Ueg;

// Minimal Zig emitter scaffold. Emits a Zig fn with body comments.

pub fn lower_to_zig(ueg: &Ueg) -> String {
    let mut out = String::new();
    out.push_str("const std = @import(\"std\");\n\n");
    for n in &ueg.nodes {
        let crate::walker::NodeKind::Lambda(l) = n;
        // build params
        let mut params = String::new();
        for (i, (pn, pt)) in l.params.iter().enumerate() {
            if i > 0 {
                params.push_str(", ");
            }
            params.push_str(pn);
            params.push_str(": ");
            let norm = crate::types::normalize_annotation(pt);
            params.push_str(&map_type_zig(&norm));
        }
        let ret = l
            .ret
            .as_ref()
            .map(|r| map_type_zig(&crate::types::normalize_annotation(r)));
        if let Some(rty) = ret.as_ref() {
            out.push_str(&format!("pub fn {}({}) {} {{\n", l.name, params, rty));
        } else {
            out.push_str(&format!("pub fn {}({}) void {{\n", l.name, params));
        }
        for line in &l.body {
            let mut zline = line.clone();
            if zline.starts_with("let mut ") || zline.starts_with("let ") {
                if let Some(eq) = zline.find('=') {
                    let head = if zline.starts_with("let mut ") {
                        &zline[8..eq]
                    } else {
                        &zline[4..eq]
                    };
                    let lhs = head.split(':').next().unwrap_or("").trim();
                    let rhs = zline[eq + 1..].trim().trim_end_matches(';');
                    zline = format!("var {} = {};", lhs, rhs);
                }
            }
            // handle return statements
            if zline.trim_start().starts_with("return ") {
                let expr = zline
                    .trim_start()
                    .trim_start_matches("return ")
                    .trim()
                    .trim_end_matches(';');
                zline = format!("return {};", expr);
            }
            // simple assignment without let/decl
            if zline.contains('=') && !zline.starts_with("std.debug") && !zline.starts_with("//") {
                let s = zline.trim_end_matches(';').to_string();
                zline = format!("{};", s);
            }
            // handle simple range loops `for _ in a..=b {` -> while loop in Zig
            if zline.trim_start().starts_with("for _ in ") && zline.contains("..=") {
                if let Some(range) = zline.split_whitespace().nth(3) {
                    if let Some((a, b)) = range.split_once("..=") {
                        let a = a.trim();
                        let b = b.trim().trim_end_matches('{');
                        zline = format!("var i: i32 = {};\n    while (i <= {}) {{", a, b);
                    }
                }
            }
            if zline.contains("println!(") {
                let inner = zline
                    .splitn(2, ",")
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(")")
                    .trim();
                zline = format!("std.debug.print({} , .{});", inner, "");
            }
            if zline.trim() == "}" {
                continue;
            }
            out.push_str(&format!("    {}\n", zline));
        }
        out.push_str("}\n");
    }
    out
}

fn map_type_zig(ann: &str) -> String {
    let a = ann.trim();
    if let Some((base, inner)) = split_generic(a) {
        match base.as_str() {
            "Vec" | "List" => return format!("[]{}", map_type_zig(&inner)),
            "Option" | "Option::<_>" => return map_type_zig(&inner),
            "HashMap" | "Map" | "Dict" => {
                if let Some((k, v)) = split_two(&inner) {
                    return format!(
                        "std.hash_map.HashMap({}, {})",
                        map_type_zig(&k),
                        map_type_zig(&v)
                    );
                }
            }
            _ => {}
        }
    }
    match a {
        "i32" => "i32".to_string(),
        "f64" => "f64".to_string(),
        "String" => "[]const u8".to_string(),
        "bool" => "bool".to_string(),
        other => other.to_string(),
    }
}

fn split_generic(s: &str) -> Option<(String, String)> {
    if let Some(start) = s.find('<') {
        if s.ends_with('>') {
            return Some((
                s[..start].trim().to_string(),
                s[start + 1..s.len() - 1].to_string(),
            ));
        }
    }
    if let Some(start) = s.find('[') {
        if s.ends_with(']') {
            return Some((
                s[..start].trim().to_string(),
                s[start + 1..s.len() - 1].to_string(),
            ));
        }
    }
    None
}

fn split_two(s: &str) -> Option<(String, String)> {
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
        Some((s[..i].trim().to_string(), s[i + 1..].trim().to_string()))
    } else {
        None
    }
}
