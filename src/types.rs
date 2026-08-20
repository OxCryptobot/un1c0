// Small utility to parse and normalize type annotations, including nested generics.
// Returns a canonical form using angle-bracket generics, e.g. `List<Optional<i32>>`.

pub fn normalize_annotation(s: &str) -> String {
    let s = s.trim();
    let parsed = parse_type(s);
    parsed
}

fn parse_type(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "".into();
    }
    // if contains top-level comma, leave as tuple-like
    let items = split_top_commas(s);
    if items.len() > 1 {
        let inner = items
            .into_iter()
            .map(|it| parse_type(&it))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("({})", inner);
    }
    // find base and generic part
    if let Some((base, inner)) = split_generic_top(s) {
        let inner_items = split_top_commas(&inner);
        let inner_parsed = inner_items
            .into_iter()
            .map(|it| parse_type(&it))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("{}<{}>", base.trim(), inner_parsed);
    }
    // primitives normalization
    match s {
        "int" => "i32".into(),
        "float" => "f64".into(),
        "str" => "String".into(),
        "bool" => "bool".into(),
        "None" => "Option::<_>".into(),
        other => other.into(),
    }
}

fn split_generic_top(s: &str) -> Option<(String, String)> {
    // support formats like Name[...], Name<...>, Name(...)
    if let Some(pos) = s.find('[') {
        if s.ends_with(']') {
            return Some((s[..pos].to_string(), s[pos + 1..s.len() - 1].to_string()));
        }
    }
    if let Some(pos) = s.find('<') {
        if s.ends_with('>') {
            return Some((s[..pos].to_string(), s[pos + 1..s.len() - 1].to_string()));
        }
    }
    if let Some(pos) = s.find('(') {
        if s.ends_with(')') {
            return Some((s[..pos].to_string(), s[pos + 1..s.len() - 1].to_string()));
        }
    }
    None
}

fn split_top_commas(s: &str) -> Vec<String> {
    let mut res = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => {
                if depth > 0 {
                    depth -= 1
                }
            }
            ',' if depth == 0 => {
                res.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        res.push(s[start..].trim().to_string());
    }
    res
}
