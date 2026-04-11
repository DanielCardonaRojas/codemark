use regex::Regex;

use crate::error::Result;

/// Strip text predicates (#eq?, #match?) from a query string (Tier 1 -> Tier 2).
/// Keeps the structural nesting and captures, removes text constraints.
pub fn relax_query(query: &str) -> Result<String> {
    let predicate_re = Regex::new(r#"\s*\(#(?:eq|match)\?\s+@\w+\s+"[^"]*"\)"#).unwrap();
    let relaxed = predicate_re.replace_all(query, "");
    let result = clean_whitespace(&relaxed);
    Ok(result)
}

/// Extract only the innermost @target pattern with its name predicate (Tier 1 -> Tier 3).
pub fn minimize_query(query: &str) -> Result<String> {
    // Find the innermost node pattern that has @target
    // Strategy: find the last opening paren before @target that starts a node type
    let target_pos = query
        .find(") @target")
        .ok_or_else(|| crate::error::Error::TreeSitter("no @target in query".into()))?;

    // Walk backward from @target to find the matching opening paren
    let before_target = &query[..target_pos + 1]; // include the closing )
    let mut depth = 0;
    let mut start = 0;

    for (i, ch) in before_target.char_indices().rev() {
        match ch {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    start = i;
                    break;
                }
            }
            _ => {}
        }
    }

    let target_pattern = &query[start..target_pos + 1];
    // Add @target back and keep only the name predicate for the target
    let mut minimal = format!("{target_pattern} @target");

    // Extract the fn_name predicate if present
    let fn_name_pred = Regex::new(r#"\(#eq\?\s+@fn_name\s+"[^"]*"\)"#).unwrap();
    if let Some(m) = fn_name_pred.find(query) {
        // Only include if the target pattern has @fn_name
        if target_pattern.contains("@fn_name") {
            minimal.push('\n');
            minimal.push_str(m.as_str());
        }
    }

    Ok(clean_whitespace(&minimal))
}

/// Clean up excess whitespace in a query string.
fn clean_whitespace(query: &str) -> String {
    let lines: Vec<&str> = query.lines().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relax_removes_predicates() {
        let tier1 = r#"(class_declaration
  name: (type_identifier) @name0
  (#eq? @name0 "AuthService")
  (class_body
    (function_declaration
      name: (simple_identifier) @fn_name
      (#eq? @fn_name "validateToken")) @target))"#;

        let relaxed = relax_query(tier1).unwrap();
        assert!(!relaxed.contains("#eq?"));
        assert!(relaxed.contains("class_declaration"));
        assert!(relaxed.contains("function_declaration"));
        assert!(relaxed.contains("@target"));
    }

    #[test]
    fn minimize_extracts_target() {
        let tier1 = r#"(class_declaration
  name: (type_identifier) @name0
  (#eq? @name0 "AuthService")
  (class_body
    (function_declaration
      name: (simple_identifier) @fn_name
      (#eq? @fn_name "validateToken")) @target))"#;

        let minimal = minimize_query(tier1).unwrap();
        assert!(minimal.contains("function_declaration"));
        assert!(minimal.contains("@target"));
        assert!(minimal.contains("#eq? @fn_name"));
        // Should NOT contain parent class info
        assert!(!minimal.contains("class_declaration"));
    }

    #[test]
    fn minimize_top_level_function() {
        let tier1 = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "createDefaultAuthService")) @target"#;

        let minimal = minimize_query(tier1).unwrap();
        assert!(minimal.contains("function_declaration"));
        assert!(minimal.contains("@fn_name"));
        assert!(minimal.contains("createDefaultAuthService"));
    }

    #[test]
    fn relaxed_query_matches_more() {
        // Verify with real parser that relaxed query still matches
        use crate::parser::languages::{Language, Parser};
        use crate::query::matcher;

        let mut parser = Parser::new(Language::Swift).unwrap();
        let source = b"class Foo {\n  func bar() {}\n  func baz() {}\n}";
        let tree = parser.parse(source).unwrap();
        let lang = Language::Swift.tree_sitter_language();

        let exact = r#"(class_declaration
  name: (type_identifier) @name0
  (#eq? @name0 "Foo")
  (class_body
    (function_declaration
      name: (simple_identifier) @fn_name
      (#eq? @fn_name "bar")) @target))"#;

        let exact_matches = matcher::run_query(exact, &tree, source, &lang).unwrap();
        assert_eq!(exact_matches.len(), 1);

        let relaxed = relax_query(exact).unwrap();
        let relaxed_matches = matcher::run_query(&relaxed, &tree, source, &lang).unwrap();
        // Relaxed should match both bar and baz (no name filter)
        assert_eq!(relaxed_matches.len(), 2);
    }
}
