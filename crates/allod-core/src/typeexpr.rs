//! The attribute type grammar of §1.3: base types, `enum<…>`,
//! `list<T>`, `map<string, T>`, and named structs (§2.1).

use crate::vocab::BASE_TYPES;
use std::collections::BTreeSet;

/// Validate a type expression, returning every violation found.
/// `structs` holds the graph-global names of declared structs (§2.1),
/// which are valid wherever a base type is. An empty result means the
/// expression conforms.
pub fn type_expr_errors(expr: &str, structs: &BTreeSet<String>) -> Vec<String> {
    let mut errors = Vec::new();
    check(expr.trim(), structs, &mut errors);
    errors
}

fn check(expr: &str, structs: &BTreeSet<String>, errors: &mut Vec<String>) {
    if BASE_TYPES.contains(&expr) || structs.contains(expr) {
        return;
    }
    if let Some(inner) = expr
        .strip_prefix("enum<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        let symbols: Vec<&str> = inner.split('|').map(str::trim).collect();
        if symbols.len() < 2 {
            errors.push(format!("enum needs at least two symbols: {expr}"));
        }
        for sym in symbols {
            let ok = !sym.is_empty()
                && sym.chars().all(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
                });
            if !ok {
                errors.push(format!(
                    "enum symbol {sym:?} must be nonempty [A-Za-z0-9._-] (§1.3)"
                ));
            }
        }
        return;
    }
    if let Some(inner) = expr
        .strip_prefix("list<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        check(inner.trim(), structs, errors);
        return;
    }
    if let Some(inner) = expr
        .strip_prefix("map<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        let mut depth = 0usize;
        let mut split = None;
        for (i, ch) in inner.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    split = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(split) = split else {
            errors.push(format!("map type needs key and value: {expr}"));
            return;
        };
        let key = inner[..split].trim();
        if key != "string" {
            errors.push(format!("map keys must be string, got {key}"));
        }
        check(inner[split + 1..].trim(), structs, errors);
        return;
    }
    errors.push(format!("unknown attribute type {expr:?}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_structs() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn accepts_valid_expressions() {
        let s = no_structs();
        assert!(type_expr_errors("string", &s).is_empty());
        assert!(type_expr_errors("selector", &s).is_empty());
        assert!(type_expr_errors("list<string>", &s).is_empty());
        assert!(type_expr_errors("list<map<string, string>>", &s).is_empty());
        assert!(type_expr_errors("map<string, list<int>>", &s).is_empty());
        assert!(type_expr_errors("enum<open|done|dropped>", &s).is_empty());
        assert!(type_expr_errors("list<enum<state|history|subscribe>>", &s).is_empty());
    }

    #[test]
    fn accepts_declared_structs() {
        let mut s = no_structs();
        s.insert("KeyRecord".into());
        assert!(type_expr_errors("KeyRecord", &s).is_empty());
        assert!(type_expr_errors("list<KeyRecord>", &s).is_empty());
        assert_eq!(type_expr_errors("Ghost", &s).len(), 1);
    }

    #[test]
    fn rejects_invalid_expressions() {
        let s = no_structs();
        assert_eq!(type_expr_errors("money", &s).len(), 1);
        assert_eq!(type_expr_errors("map<int, money>", &s).len(), 2);
        assert_eq!(type_expr_errors("map<string>", &s).len(), 1);
        assert_eq!(type_expr_errors("list<map<string", &s).len(), 1);
        assert_eq!(type_expr_errors("enum<only>", &s).len(), 1);
        assert_eq!(type_expr_errors("enum<a||b>", &s).len(), 1);
        assert_eq!(type_expr_errors("enum<a|b c>", &s).len(), 1);
    }
}
