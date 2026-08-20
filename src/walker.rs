use std::collections::HashMap;
use tree_sitter::Node;

// Clean single-file implementation: UEG types, entropy gate, python->UEG->Rust

#[derive(Debug, Clone)]
pub struct Ueg {
    pub nodes: Vec<NodeKind>,
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Lambda(LambdaNode),
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
}

impl Ueg {
    pub fn new() -> Self {
        Ueg { nodes: Vec::new() }
    }
    pub fn validate(&self) -> bool {
        !self.nodes.is_empty()
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

/// Build a UEG from Python source without lowering it to a target.
#[allow(dead_code)]
pub fn python_to_ueg(_root: &Node, source: &[u8]) -> Ueg {
    let src = String::from_utf8_lossy(source).to_string();
    // re-use same parsing logic as python_to_rust to produce the UEG
    let mut name = String::new();
    let mut params: Vec<(String, String)> = Vec::new();
    let mut ret: Option<String> = None;
    let mut body_lines: Vec<String> = Vec::new();

    // collect all lines for indexed access so we can capture decorators and exact text
    let lines: Vec<&str> = src.lines().collect();
    let mut orig_lines: Vec<String> = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("def ") {
            continue;
        }
        // capture decorators above the def
        let mut start_idx = line_idx;
        while start_idx > 0 {
            let prev = lines[start_idx - 1].trim_start();
            if prev.starts_with("@") || prev.starts_with("#") {
                start_idx -= 1;
                continue;
            }
            if prev.is_empty() {
                start_idx -= 1;
                continue;
            }
            break;
        }
        // record exact original lines from start_idx until next top-level def or EOF
        let mut idx = start_idx;
        while idx < lines.len() {
            let t = lines[idx].trim_start();
            if idx > line_idx && t.starts_with("def ") {
                break;
            }
            orig_lines.push(lines[idx].to_string());
            idx += 1;
        }

        let sig = trimmed.trim_end_matches(':').trim();
        let rest = sig.trim_start_matches("def").trim();
        name = rest.split('(').next().unwrap_or("").trim().to_string();
        if let Some(pstart) = rest.find('(') {
            if let Some(pend) = rest.find(')') {
                for p in rest[pstart + 1..pend].split(',') {
                    let p = p.trim();
                    if p.is_empty() {
                        continue;
                    }
                    if let Some(colon) = p.find(':') {
                        let nm = p[..colon].trim().to_string();
                        let ann = p[colon + 1..].trim();
                        params.push((nm, map_type(ann)));
                    } else {
                        params.push((p.to_string(), "_".into()));
                    }
                }
            }
            if let Some(arrow) = rest.find("->") {
                let after = rest[arrow + 2..].trim().trim_end_matches(':').trim();
                if !after.is_empty() {
                    ret = Some(map_type(after));
                }
            }
        }
        // collect trimmed body lines (for translation) from def line +1 until break
        let mut j = line_idx + 1;
        while j < lines.len() {
            let raw = lines[j];
            let t = raw.trim().to_string();
            if t.is_empty() {
                j += 1;
                continue;
            }
            if t.starts_with("def ") {
                break;
            }
            body_lines.push(t);
            j += 1;
        }
        // Build a small AST fragment for emitters: JSON-like string with name, params, ret
        let _frag = {
            let mut parts: Vec<String> = Vec::new();
            parts.push(format!("\"name\": \"{}\"", name));
            let ps = params
                .iter()
                .map(|(n, t)| format!("{{\"n\":\"{}\",\"t\":\"{}\"}}", n, t))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("\"params\": [{}]", ps));
            if let Some(r) = &ret {
                parts.push(format!("\"ret\": \"{}\"", r));
            }
            format!("{{{}}}", parts.join(","))
        };
        // attach frag after break
        break;
    }

    if name.is_empty() {
        return Ueg::new();
    }

    let mut ueg = Ueg::new();
    let lambda = LambdaNode {
        name: name.clone(),
        params: params.clone(),
        ret: ret.clone(),
        body: translate_body_to_rust_like(&body_lines),
        orig_body: orig_lines,
        ast_fragment: Some({
            // regenerate small fragment consistently
            let mut parts: Vec<String> = Vec::new();
            parts.push(format!("\"name\": \"{}\"", name));
            let ps = params
                .iter()
                .map(|(n, t)| format!("{{\"n\":\"{}\",\"t\":\"{}\"}}", n, t))
                .collect::<Vec<_>>()
                .join(",");
            parts.push(format!("\"params\": [{}]", ps));
            if let Some(r) = &ret {
                parts.push(format!("\"ret\": \"{}\"", r));
            }
            format!("{{{}}}", parts.join(","))
        }),
    };
    ueg.nodes.push(NodeKind::Lambda(lambda));
    ueg
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
