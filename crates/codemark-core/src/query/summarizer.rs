//! Query summarization for generating human-readable headlines from tree-sitter queries.
//!
//! This module uses tree-sitter-tsquery to parse query strings and extract semantic
//! information to produce concise summaries like "function cycle_mode" or "class AuthService".

use crate::parser::languages::Language;
use crate::query::classifier::classify_node_type;
use thiserror::Error;
use tree_sitter::{Node, Parser};

#[derive(Debug, Error)]
pub enum SummarizeError {
    #[error("Failed to parse query string")]
    ParseError,
    #[error("No @target capture found in query")]
    NoTarget,
    #[error("Could not determine node type for @target")]
    UnknownNodeType,
}

/// A summary of a tree-sitter query, containing the label and optional identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuerySummary {
    /// The human-readable label (e.g., "function", "class", "method").
    pub label: String,
    /// The identifier/name if found in the query (e.g., "cycle_mode", "AuthService").
    pub identifier: Option<String>,
    /// The language this query belongs to.
    pub language: Option<Language>,
    /// Best-effort identifier of the deepest labelled node in the query, used as
    /// a display fallback when the target itself carries no `#eq` name (e.g. a
    /// bare statement anchored only by its enclosing named landmarks). This is
    /// purely for display via [`Self::short_display`]; it is intentionally kept
    /// out of [`Self::format`] and semantic-enrichment text so those stay
    /// scoped to the target node.
    fallback_identifier: Option<String>,
}

impl QuerySummary {
    /// Format the summary as a string for display.
    ///
    /// Returns "label identifier" if both exist, or just "label" if no identifier.
    pub fn format(&self) -> Option<String> {
        if self.label.is_empty() {
            return None;
        }
        match &self.identifier {
            Some(id) => Some(format!("{} {}", self.label, id)),
            None => Some(self.label.clone()),
        }
    }

    /// A concise display string for list rows.
    ///
    /// Prefers the target's own `#eq` identifier, then the deepest labelled
    /// node's identifier (usually the enclosing declaration), and finally the
    /// humanized node-type label. Never returns the raw query text.
    pub fn short_display(&self) -> String {
        self.identifier
            .clone()
            .or_else(|| self.fallback_identifier.clone())
            .unwrap_or_else(|| self.label.clone())
    }

    /// Create a new QuerySummary with the given label and identifier.
    pub fn new(label: String, identifier: Option<String>, language: Option<Language>) -> Self {
        Self { label, identifier, language, fallback_identifier: None }
    }

    /// Create a QuerySummary with only a label (no identifier).
    pub fn label_only(label: String, language: Option<Language>) -> Self {
        Self { label, identifier: None, language, fallback_identifier: None }
    }
}

/// Summarize a tree-sitter query by extracting its semantic information.
///
/// This function parses the query string and looks for:
/// 1. The node tagged with `@target` capture
/// 2. The node type of that target (e.g., "function_item", "class_declaration")
/// 3. Any `#eq?` or `#match?` predicates that contain an identifier
///
/// # Arguments
///
/// * `query_str` - The tree-sitter query string to parse
/// * `language` - The programming language this query is for
///
/// # Returns
///
/// * `Ok(QuerySummary)` if the query could be parsed and summarized
/// * `Err(SummarizeError)` if parsing failed or no @target was found
///
/// # Examples
///
/// ```
/// use codemark_core::query::summarizer::summarize_query;
/// use codemark_core::parser::languages::Language;
///
/// let query = r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "cycle_mode")) @target"#;
/// let rust: Language = "rust".parse().unwrap();
/// let summary = summarize_query(query, Some(rust)).unwrap();
/// assert_eq!(summary.label, "function");
/// assert_eq!(summary.identifier, Some("cycle_mode".to_string()));
/// ```
pub fn summarize_query(
    query_str: &str,
    language: Option<Language>,
) -> Result<QuerySummary, SummarizeError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_tsquery::LANGUAGE.into())
        .map_err(|_| SummarizeError::ParseError)?;

    let tree = parser.parse(query_str, None).ok_or(SummarizeError::ParseError)?;
    let root = tree.root_node();

    // 1. Find @target capture node
    let target_capture = find_capture_node(root, query_str, "target")?;
    let target_parent = target_capture.parent().ok_or(SummarizeError::NoTarget)?;

    // 2. Find the associated node pattern and its type
    let node_type = find_target_node_type(target_capture, query_str)?;

    // 3. Get all capture names inside the target node pattern
    let mut target_captures = Vec::new();
    get_capture_names(target_parent, query_str, &mut target_captures);

    // 4. Extract identifier from predicates that reference target captures.
    // Predicate values can be whole expressions (a multi-line call chain with
    // trailing closures, an `if let` condition), so compact them to one line.
    let identifier =
        extract_identifier(root, query_str, &target_captures).and_then(|v| compact_identifier(&v));

    // When the target has no name of its own, fall back (for display only) to
    // the nearest enclosing labelled node's identifier so a list row shows the
    // containing name instead of the whole query string.
    let fallback_identifier = if identifier.is_none() {
        enclosing_predicate_identifier(target_parent, query_str)
            .and_then(|v| compact_identifier(&v))
    } else {
        None
    };

    let profile = language.as_ref().map(|lang| lang.profile());
    let label =
        classify_node_type(&node_type, profile).unwrap_or_else(|| node_type.replace('_', " "));

    Ok(QuerySummary { label, identifier, language, fallback_identifier })
}

/// Recursively find all capture names inside a node.
fn get_capture_names(node: Node, source: &str, names: &mut Vec<String>) {
    if node.kind() == "capture"
        && let Ok(name) = node.utf8_text(source.as_bytes())
    {
        names.push(name.to_string());
    }
    for i in 0..node.child_count() {
        get_capture_names(node.child(i).unwrap(), source, names);
    }
}

/// Recursively find a capture node with the given name (e.g., "target").
fn find_capture_node<'a>(
    node: Node<'a>,
    source: &str,
    name: &str,
) -> Result<Node<'a>, SummarizeError> {
    if node.kind() == "capture" {
        let capture_name = node.utf8_text(source.as_bytes()).unwrap_or("");
        if capture_name == format!("@{}", name) {
            return Ok(node);
        }
    }

    for i in 0..node.child_count() {
        if let Ok(found) = find_capture_node(node.child(i).unwrap(), source, name) {
            return Ok(found);
        }
    }

    Err(SummarizeError::NoTarget)
}

/// Find the node type of the pattern that has the @target capture.
fn find_target_node_type(capture_node: Node, source: &str) -> Result<String, SummarizeError> {
    // In tree-sitter-tsquery, @target is a child of a named_node
    let mut current = capture_node;
    while let Some(parent) = current.parent() {
        if parent.kind() == "named_node"
            && let Some(name_node) = parent.child_by_field_name("name")
        {
            return Ok(name_node.utf8_text(source.as_bytes()).unwrap_or("").to_string());
        }
        current = parent;
    }

    Err(SummarizeError::UnknownNodeType)
}

/// Extract an identifier from #eq? or #match? predicates in the query.
fn extract_identifier(root: Node, source: &str, target_captures: &[String]) -> Option<String> {
    let mut identifier = None;
    find_identifier_in_predicates(root, source, target_captures, &mut identifier);
    identifier
}

fn find_identifier_in_predicates(
    node: Node,
    source: &str,
    target_captures: &[String],
    identifier: &mut Option<String>,
) {
    if let Some((cap, val)) = predicate_capture_and_value(node, source)
        && target_captures.contains(&cap)
    {
        *identifier = Some(val);
        return;
    }

    for i in 0..node.child_count() {
        find_identifier_in_predicates(node.child(i).unwrap(), source, target_captures, identifier);
        if identifier.is_some() {
            return;
        }
    }
}

/// Find the `#eq?`/`#match?` string bound to the nearest node pattern that
/// *encloses* the `@target` pattern, ignoring which capture it targets.
///
/// Used as a display fallback when the target node itself carries no name
/// predicate: the closest enclosing named landmark (typically the containing
/// declaration). Only ancestors of the target are considered, so a predicate in
/// an unrelated sibling branch can never label the row, however deeply nested.
fn enclosing_predicate_identifier(target_pattern: Node, source: &str) -> Option<String> {
    let mut current = Some(target_pattern);
    while let Some(node) = current {
        if node.kind() == "named_node" {
            let mut cursor = node.walk();
            let found = node
                .children(&mut cursor)
                .find_map(|child| predicate_capture_and_value(child, source).map(|(_, v)| v));
            if found.is_some() {
                return found;
            }
        }
        current = node.parent();
    }
    None
}

/// Maximum length (in chars) of a compacted identifier before it is truncated.
const MAX_IDENTIFIER_CHARS: usize = 60;

/// Reduce a predicate value to a short, single-line display identifier.
///
/// Names (`cycle_mode`) pass through untouched. Whole expressions, which the
/// generator emits for calls and conditions, get cleaned up:
/// - `{ ... }` blocks (closure / trailing-closure bodies) are dropped,
/// - parenthesised groups spanning several lines collapse to `(…)`,
/// - line breaks and indentation are folded (a line starting with `.` is
///   glued to the previous one, so `a\n    .b` becomes `a.b`),
/// - the result is truncated to [`MAX_IDENTIFIER_CHARS`] with a trailing `…`.
///
/// Returns `None` when nothing displayable is left.
fn compact_identifier(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut stripped = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(close) = closing_delimiter(c)
            && let Some(end) = matching_close(&chars, i, c, close)
        {
            if c == '{' {
                stripped.truncate(stripped.trim_end().len());
                i = end + 1;
                continue;
            }
            if chars[i + 1..end].contains(&'\n') {
                stripped.push_str("(…)");
                i = end + 1;
                continue;
            }
        }
        stripped.push(c);
        i += 1;
    }

    let mut joined = String::with_capacity(stripped.len());
    for line in stripped.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        let glue = joined.is_empty()
            || line.starts_with(['.', ')', ']', ',', '?', '!'])
            || joined.ends_with(['(', '[']);
        if !glue {
            joined.push(' ');
        }
        joined.push_str(&line);
    }

    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() > MAX_IDENTIFIER_CHARS {
        let mut truncated: String = joined.chars().take(MAX_IDENTIFIER_CHARS - 1).collect();
        truncated.truncate(truncated.trim_end().len());
        truncated.push('…');
        return Some(truncated);
    }
    Some(joined)
}

/// The closing delimiter for the groups [`compact_identifier`] rewrites.
fn closing_delimiter(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '(' => Some(')'),
        _ => None,
    }
}

/// Index of the delimiter closing the group opened at `start`, skipping over
/// string literals so a `)` or `}` inside quotes does not end the group early.
/// Returns `None` for an unbalanced group, which is then left as-is.
fn matching_close(chars: &[char], start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            match c {
                '\\' => i += 1,
                '"' => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// If `node` is an `#eq?`/`#match?` predicate, return the `(capture, string)`
/// literal it binds, e.g. `(#eq? @fn_name "cycle_mode")` -> `("@fn_name", "cycle_mode")`.
fn predicate_capture_and_value(node: Node, source: &str) -> Option<(String, String)> {
    if node.kind() != "predicate" {
        return None;
    }
    // Accept only positive equality/match predicates. `#not-eq?`/`#not-match?`
    // name a value the query *excludes*, so they must never label a bookmark.
    let mut cursor = node.walk();
    let operator = node
        .children(&mut cursor)
        .find(|child| child.kind() == "identifier")
        .and_then(|child| child.utf8_text(source.as_bytes()).ok())?;
    if !matches!(operator, "eq" | "match" | "any-eq" | "any-match") {
        return None;
    }
    let is_predicate = node
        .child_by_field_name("type")
        .and_then(|t| t.utf8_text(source.as_bytes()).ok())
        .is_some_and(|t| t == "?");
    if !is_predicate {
        return None;
    }

    let params = node.child_by_field_name("parameters")?;
    let mut capture = None;
    let mut value = None;
    for i in 0..params.child_count() {
        let child = params.child(i).unwrap();
        match child.kind() {
            "capture" => capture = child.utf8_text(source.as_bytes()).ok().map(str::to_string),
            "string" => value = string_literal_content(child, source),
            _ => {}
        }
    }

    Some((capture?, value?))
}

/// Extract the inner text of a tree-sitter query `string` node, unwrapping the
/// `string_content` child when present and stripping surrounding quotes otherwise.
/// Escape sequences (`\n`, `\"`, ...) are decoded to the characters they stand
/// for, so the value matches the source text the query was generated from.
fn string_literal_content(string_node: Node, source: &str) -> Option<String> {
    for j in 0..string_node.child_count() {
        let content = string_node.child(j).unwrap();
        if content.kind() == "string_content" {
            return content.utf8_text(source.as_bytes()).ok().map(unescape_query_string);
        }
    }
    string_node
        .utf8_text(source.as_bytes())
        .ok()
        .map(|s| unescape_query_string(s.trim_matches('"')))
}

/// Decode the escape sequences tree-sitter query strings support.
fn unescape_query_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_simple_function_query() {
        let query =
            r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "cycle_mode")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.label, "function");
        assert_eq!(summary.identifier, Some("cycle_mode".to_string()));
        assert_eq!(summary.format(), Some("function cycle_mode".to_string()));
    }

    #[test]
    fn test_summarize_class_query() {
        let query = r#"(class_declaration name: (type_identifier) @name0 (#eq? @name0 "AuthService")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("swift"))).unwrap();

        assert_eq!(summary.label, "class");
        assert_eq!(summary.identifier, Some("AuthService".to_string()));
        assert_eq!(summary.format(), Some("class AuthService".to_string()));
    }

    #[test]
    fn test_summarize_method_query() {
        let query = r#"(method_definition name: (property_identifier) @fn_name (#eq? @fn_name "validateToken")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("typescript"))).unwrap();

        assert_eq!(summary.label, "method");
        assert_eq!(summary.identifier, Some("validateToken".to_string()));
    }

    #[test]
    fn test_summarize_query_without_identifier() {
        let query = r#"(if_statement) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.label, "if statement");
        assert_eq!(summary.identifier, None);
        assert_eq!(summary.format(), Some("if statement".to_string()));
    }

    #[test]
    fn test_summarize_invalid_query_returns_error() {
        let query = "not a valid query";
        let summary = summarize_query(query, None);

        assert!(summary.is_err());
    }

    #[test]
    fn test_short_display_prefers_target_identifier() {
        let query =
            r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "cycle_mode")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.short_display(), "cycle_mode");
    }

    // A target with no name of its own, anchored only by an enclosing named
    // landmark: `short_display` must surface the deepest labelled node's
    // identifier (the containing function) rather than the raw query string.
    #[test]
    fn test_short_display_falls_back_to_deepest_node_identifier() {
        let query = r#"(function_item
              name: (identifier) @name0
              (#eq? @name0 "process")
              body: (block
                (expression_statement) @target))"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        // No target-scoped #eq, so the strict identifier stays empty...
        assert_eq!(summary.identifier, None);
        // ...but the display falls back to the nearest enclosing name.
        assert_eq!(summary.short_display(), "process");
    }

    // Nested landmarks: the *deepest* predicate (closest to the target) wins, so
    // the method name is preferred over the enclosing class name.
    #[test]
    fn test_short_display_deepest_wins_over_outer_landmark() {
        let query = r#"(class_declaration
              name: (type_identifier) @name0
              (#eq? @name0 "AuthService")
              body: (class_body
                (method_definition
                  name: (property_identifier) @name1
                  (#eq? @name1 "validateToken")
                  body: (statement_block
                    (return_statement) @target))))"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("typescript"))).unwrap();

        assert_eq!(summary.identifier, None);
        assert_eq!(summary.short_display(), "validateToken");
    }

    // With no predicates at all, `short_display` falls back to the humanized
    // node-type label — still short, never the raw query.
    #[test]
    fn test_short_display_falls_back_to_label() {
        let query = r#"(if_statement) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.identifier, None);
        assert_eq!(summary.short_display(), "if statement");
    }

    #[test]
    fn test_summarize_enum_query() {
        let query = r#"(enum_declaration name: (type_identifier) @name0 (#eq? @name0 "AuthError")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("swift"))).unwrap();

        assert_eq!(summary.label, "enum");
        assert_eq!(summary.identifier, Some("AuthError".to_string()));
    }

    // A Swift call chain with trailing closures: the generator pins the callee
    // text, which spans lines and carries closure bodies. The summary must be a
    // single line with no braces or escaped newlines.
    #[test]
    fn test_summarize_swift_call_chain_is_compacted() {
        let query = r#"(function_declaration
  name: (simple_identifier) @name0 @sticky.function
  (#eq? @name0 "pipeline")
  (function_body
    (statements
      (call_expression
        (_) @func
        (#eq? @func "[1, 2, 3]\n        .map { $0 * 2 }\n        .filter { $0 > 2 }\n        .forEach")) @target)))"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("swift"))).unwrap();

        assert_eq!(summary.label, "call");
        assert_eq!(summary.identifier.as_deref(), Some("[1, 2, 3].map.filter.forEach"));
        assert_eq!(summary.short_display(), "[1, 2, 3].map.filter.forEach");
    }

    #[test]
    fn test_summarize_multiline_navigation_is_joined() {
        let query = r#"(call_expression
  function: (_) @func
  (#eq? @func "state\n            .github\n            .verify_write_access")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.short_display(), "state.github.verify_write_access");
    }

    // Multi-line argument lists collapse to `(…)`; single-line ones are kept.
    #[test]
    fn test_summarize_multiline_arguments_collapse() {
        let query = r#"(if_expression
  condition: (_) @cond
  (#eq? @cond "let Some(cmd) = config.open.build_editor_command(\n            &step.file_path,\n            line_end,\n        )")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.short_display(), "let Some(cmd) = config.open.build_editor_command(…)");
    }

    #[test]
    fn test_summarize_unescapes_quotes() {
        let query = r#"(if_expression
  condition: (_) @cond
  (#eq? @cond "target.starts_with(\"__pick__\")")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.short_display(), r#"target.starts_with("__pick__")"#);
    }

    #[test]
    fn test_compact_identifier_truncates_long_values() {
        let long = "a".repeat(100);
        let compacted = compact_identifier(&long).unwrap();

        assert_eq!(compacted.chars().count(), MAX_IDENTIFIER_CHARS);
        assert!(compacted.ends_with('…'));
    }

    #[test]
    fn test_compact_identifier_keeps_plain_names_and_drops_empty() {
        assert_eq!(compact_identifier("cycle_mode").as_deref(), Some("cycle_mode"));
        assert_eq!(compact_identifier("{ x in\n  print(x)\n}"), None);
        // Braces inside string literals do not open a group.
        assert_eq!(compact_identifier(r#"f("{")"#).as_deref(), Some(r#"f("{")"#));
    }

    // #223: negative predicates name a value the query excludes.
    #[test]
    fn test_negative_predicates_are_ignored() {
        for op in ["not-eq", "not-match"] {
            let query = format!(
                r#"(function_item name: (identifier) @fn_name (#{op}? @fn_name "excluded")) @target"#
            );
            let summary = summarize_query(&query, Some(crate::testing::grammar("rust"))).unwrap();

            assert_eq!(summary.identifier, None, "{op}");
            assert_eq!(summary.short_display(), "function", "{op}");
        }
    }

    #[test]
    fn test_any_eq_predicate_is_accepted() {
        let query = r#"(function_item name: (identifier) @fn_name (#any-eq? @fn_name "cycle_mode")) @target"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("rust"))).unwrap();

        assert_eq!(summary.identifier.as_deref(), Some("cycle_mode"));
    }

    // #224: a predicate in a sibling branch, nested deeper than the target's
    // enclosing landmark, must not label the row.
    #[test]
    fn test_fallback_ignores_sibling_branch_predicates() {
        let query = r#"(class_declaration
              name: (type_identifier) @name0
              (#eq? @name0 "AuthService")
              body: (class_body
                (method_definition
                  name: (property_identifier) @other
                  (#eq? @other "unrelated")
                  body: (statement_block
                    (if_statement
                      (identifier) @deep
                      (#eq? @deep "deeper"))))
                (return_statement) @target))"#;
        let summary = summarize_query(query, Some(crate::testing::grammar("typescript"))).unwrap();

        assert_eq!(summary.identifier, None);
        assert_eq!(summary.short_display(), "AuthService");
    }
}
