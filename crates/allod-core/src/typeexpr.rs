//! The attribute type grammar of §1.3: base types, `list<T>`, and
//! `map<string, T>`.

use crate::vocab::BASE_TYPES;

/// Validate a type expression, returning every violation found.
/// An empty result means the expression conforms.
pub fn type_expr_errors(expr: &str) -> Vec<String> {
    let mut errors = Vec::new();
    check(expr.trim(), &mut errors);
    errors
}

fn check(expr: &str, errors: &mut Vec<String>) {
    if BASE_TYPES.contains(&expr) {
        return;
    }
    if let Some(inner) = expr
        .strip_prefix("list<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        check(inner.trim(), errors);
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
        check(inner[split + 1..].trim(), errors);
        return;
    }
    errors.push(format!("unknown attribute type {expr:?}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_expressions() {
        assert!(type_expr_errors("string").is_empty());
        assert!(type_expr_errors("list<string>").is_empty());
        assert!(type_expr_errors("list<map<string, string>>").is_empty());
        assert!(type_expr_errors("map<string, list<int>>").is_empty());
    }

    #[test]
    fn rejects_invalid_expressions() {
        assert_eq!(type_expr_errors("money").len(), 1);
        assert_eq!(type_expr_errors("map<int, money>").len(), 2);
        assert_eq!(type_expr_errors("map<string>").len(), 1);
        assert_eq!(type_expr_errors("list<map<string").len(), 1);
    }
}
