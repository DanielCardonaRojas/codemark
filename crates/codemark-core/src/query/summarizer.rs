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
/// let summary = summarize_query(query, Some(Language::Rust)).unwrap();
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

    // 4. Extract identifier from predicates that reference target captures
    let identifier = extract_identifier(root, query_str, &target_captures);

    // When the target has no name of its own, fall back (for display only) to
    // the deepest labelled node's identifier so a list row shows the nearest
    // enclosing name instead of the whole query string.
    let fallback_identifier =
        if identifier.is_none() { deepest_predicate_identifier(root, query_str) } else { None };

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

/// Find the `#eq?`/`#match?` string bound to the *deepest* (most nested) node in
/// the query, ignoring which capture it targets.
///
/// Used as a display fallback when the target node itself carries no name
/// predicate: the deepest predicate is the one closest to the target, i.e. the
/// nearest enclosing named landmark (typically the containing declaration).
fn deepest_predicate_identifier(node: Node, source: &str) -> Option<String> {
    fn walk(node: Node, source: &str, depth: usize, best: &mut Option<(usize, String)>) {
        if let Some((_, val)) = predicate_capture_and_value(node, source)
            && best.as_ref().is_none_or(|(d, _)| depth >= *d)
        {
            *best = Some((depth, val));
        }
        for i in 0..node.child_count() {
            walk(node.child(i).unwrap(), source, depth + 1, best);
        }
    }

    let mut best = None;
    walk(node, source, 0, &mut best);
    best.map(|(_, val)| val)
}

/// If `node` is an `#eq?`/`#match?` predicate, return the `(capture, string)`
/// literal it binds, e.g. `(#eq? @fn_name "cycle_mode")` -> `("@fn_name", "cycle_mode")`.
fn predicate_capture_and_value(node: Node, source: &str) -> Option<(String, String)> {
    if node.kind() != "predicate" {
        return None;
    }
    let text = node.utf8_text(source.as_bytes()).unwrap_or("");
    if !(text.contains("eq?") || text.contains("match?")) {
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
fn string_literal_content(string_node: Node, source: &str) -> Option<String> {
    for j in 0..string_node.child_count() {
        let content = string_node.child(j).unwrap();
        if content.kind() == "string_content" {
            return content.utf8_text(source.as_bytes()).ok().map(str::to_string);
        }
    }
    string_node.utf8_text(source.as_bytes()).ok().map(|s| s.trim_matches('"').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_simple_function_query() {
        let query =
            r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "cycle_mode")) @target"#;
        let summary = summarize_query(query, Some(Language::Rust)).unwrap();

        assert_eq!(summary.label, "function");
        assert_eq!(summary.identifier, Some("cycle_mode".to_string()));
        assert_eq!(summary.format(), Some("function cycle_mode".to_string()));
    }

    #[test]
    fn test_summarize_class_query() {
        let query = r#"(class_declaration name: (type_identifier) @name0 (#eq? @name0 "AuthService")) @target"#;
        let summary = summarize_query(query, Some(Language::Swift)).unwrap();

        assert_eq!(summary.label, "class");
        assert_eq!(summary.identifier, Some("AuthService".to_string()));
        assert_eq!(summary.format(), Some("class AuthService".to_string()));
    }

    #[test]
    fn test_summarize_method_query() {
        let query = r#"(method_definition name: (property_identifier) @fn_name (#eq? @fn_name "validateToken")) @target"#;
        let summary = summarize_query(query, Some(Language::TypeScript)).unwrap();

        assert_eq!(summary.label, "method");
        assert_eq!(summary.identifier, Some("validateToken".to_string()));
    }

    #[test]
    fn test_summarize_query_without_identifier() {
        let query = r#"(if_statement) @target"#;
        let summary = summarize_query(query, Some(Language::Rust)).unwrap();

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
        let summary = summarize_query(query, Some(Language::Rust)).unwrap();

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
        let summary = summarize_query(query, Some(Language::Rust)).unwrap();

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
        let summary = summarize_query(query, Some(Language::TypeScript)).unwrap();

        assert_eq!(summary.identifier, None);
        assert_eq!(summary.short_display(), "validateToken");
    }

    // With no predicates at all, `short_display` falls back to the humanized
    // node-type label — still short, never the raw query.
    #[test]
    fn test_short_display_falls_back_to_label() {
        let query = r#"(if_statement) @target"#;
        let summary = summarize_query(query, Some(Language::Rust)).unwrap();

        assert_eq!(summary.identifier, None);
        assert_eq!(summary.short_display(), "if statement");
    }

    #[test]
    fn test_summarize_enum_query() {
        let query = r#"(enum_declaration name: (type_identifier) @name0 (#eq? @name0 "AuthError")) @target"#;
        let summary = summarize_query(query, Some(Language::Swift)).unwrap();

        assert_eq!(summary.label, "enum");
        assert_eq!(summary.identifier, Some("AuthError".to_string()));
    }
}
