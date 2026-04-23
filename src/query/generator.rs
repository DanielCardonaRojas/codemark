use tree_sitter::{Language, Node, Tree};

use crate::error::{Error, Result};
use crate::query::matcher;

/// A generated tree-sitter query with metadata about the target.
#[derive(Debug)]
pub struct GeneratedQuery {
    pub query: String,
    pub target_node_type: String,
    pub target_name: Option<String>,
    pub byte_range: (usize, usize),
}

/// Context information passed to query strategies.
#[derive(Clone, Debug)]
pub struct QueryContext<'a> {
    pub source: &'a [u8],
    pub language: &'a Language,
    pub byte_range: (usize, usize),
    pub root: Node<'a>,
    pub tree: &'a Tree,
}

/// Semantic information that can distinguish a node from others of the same type.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum SemanticInfo {
    /// For if statements: the condition being tested.
    IfCondition(String),
    /// For call expressions: the function being called.
    CallTarget(String),
    /// For assignments: the variable being assigned.
    AssignmentTarget(String),
    /// For return statements: the value being returned.
    ReturnValue(String),
    /// For binary expressions: the operator.
    BinaryOperator(String),
}

/// One entry in the structural path from root to target.
#[derive(Debug, Clone)]
struct PathEntry {
    node_type: String,
    /// Name info for query generation.
    name_info: Option<NameInfo>,
}

/// How to query for the "name" of a node.
#[derive(Debug, Clone)]
struct NameInfo {
    /// The field name used in the parent (usually "name", but "type" for Rust impl_item)
    /// If None, the name is matched as a descendant without a specific field.
    field: Option<String>,
    /// The direct name node type (e.g., "simple_identifier" or "user_type")
    direct_type: String,
    /// If the name is nested (e.g., user_type > type_identifier), the inner type
    inner_type: Option<String>,
    /// The text value to match
    text: String,
}

/// Given a parsed tree and a byte range, generate a tree-sitter query that uniquely
/// identifies the target node.
pub fn generate_query(
    tree: &Tree,
    source: &[u8],
    byte_range: (usize, usize),
    language: &Language,
) -> Result<GeneratedQuery> {
    let ctx = QueryContext {
        source,
        language,
        byte_range,
        root: tree.root_node(),
        tree,
    };

    // 1. Select target node (favoring named declarations)
    let node = find_target_node(&ctx.root, ctx.source, ctx.byte_range)?;

    // 2. Extract metadata
    let name = extract_name_info(node, ctx.source)
        .map(|info| info.text)
        .or_else(|| extract_identifier_from_node(node, ctx.source));
    let semantic_info = extract_semantic_info(node, ctx.source);

    // 3. Build the base query
    let base_query = build_base_query(node, name.as_deref(), semantic_info, ctx.source)?;

    // 4. Disambiguate and anchor
    let query = disambiguate_query(base_query, node, &ctx)?;

    Ok(GeneratedQuery {
        query,
        target_node_type: node.kind().to_string(),
        target_name: name,
        byte_range: (node.start_byte(), node.end_byte()),
    })
}

/// Find the most appropriate target node for a declaration-favored search.
fn find_target_node<'a>(
    root: &Node<'a>,
    source: &[u8],
    byte_range: (usize, usize),
) -> Result<Node<'a>> {
    let mut start = byte_range.0;
    let mut end = byte_range.1;

    // Trim whitespace
    while start < end && (source[start] as char).is_whitespace() {
        start += 1;
    }
    while end > start && (source[end - 1] as char).is_whitespace() {
        end -= 1;
    }

    let node = root
        .descendant_for_byte_range(start, end)
        .ok_or_else(|| Error::TreeSitter("no node found at byte range".into()))?;

    // First, try to find a declaration that is contained within the trimmed range.
    if let Some(inner) = find_declaration_within(*root, (start, end)) {
        return Ok(inner);
    }

    // Otherwise, walk up from the deepest node to the nearest named declaration.
    Ok(walk_to_named_declaration(node))
}

fn build_base_query(
    node: Node,
    name: Option<&str>,
    semantic_info: Option<SemanticInfo>,
    source: &[u8],
) -> Result<String> {
    if let Some(ref semantic) = semantic_info {
        match semantic {
            SemanticInfo::IfCondition(cond) => {
                return Ok(format!(
                    "({} condition: (_) @cond) @target\n  (#eq? @cond \"{}\")",
                    node.kind(),
                    escape_query_text(cond)
                ));
            }
            SemanticInfo::CallTarget(func) => {
                let (child, field) = node
                    .child_by_field_name("function")
                    .map(|c| (c, Some("function")))
                    .or_else(|| node.named_child(0).map(|c| (c, None)))
                    .ok_or_else(|| {
                        Error::TreeSitter("call target missing function child".into())
                    })?;

                if let Some(field) = field {
                    return Ok(format!(
                        "({} {}: ({}) @func) @target\n  (#eq? @func \"{}\")",
                        node.kind(),
                        field,
                        child.kind(),
                        escape_query_text(func)
                    ));
                } else {
                    return Ok(format!(
                        "({} ({}) @func) @target\n  (#eq? @func \"{}\")",
                        node.kind(),
                        child.kind(),
                        escape_query_text(func)
                    ));
                }
            }
            SemanticInfo::AssignmentTarget(target_name) => {
                return Ok(format!(
                    "({} left: (_) @left) @target\n  (#eq? @left \"{}\")",
                    node.kind(),
                    escape_query_text(target_name)
                ));
            }
            SemanticInfo::ReturnValue(val) => {
                return Ok(format!(
                    "({} value: (_) @val) @target\n  (#eq? @val \"{}\")",
                    node.kind(),
                    escape_query_text(val)
                ));
            }
            SemanticInfo::BinaryOperator(op) => {
                return Ok(format!(
                    "({} operator: \"{}\") @target",
                    node.kind(),
                    escape_query_text(op)
                ));
            }
        }
    }

    if let Some(name) = name {
        // For leaf nodes (identifiers, literals), add a text match predicate
        // But don't match on huge text blocks (like closures)
        if node.named_child_count() == 0 && name.len() < 100 {
            return Ok(format!(
                "({}) @target\n  (#eq? @target \"{}\")",
                node.kind(),
                escape_query_text(name)
            ));
        }
    }

    // Fallback to simple type-based query
    Ok(format!("({}) @target", node.kind()))
}

/// Helper to ensure a query is unique by walking up parents if it matches multiple nodes.
fn disambiguate_query(
    base_query: String,
    target_node: Node,
    ctx: &QueryContext<'_>,
) -> Result<String> {
    let matches = matcher::run_query(&base_query, ctx.tree, ctx.source, ctx.language)?;
    if matches.len() == 1 {
        return Ok(base_query);
    }

    // Too many matches or 0 matches (invalid base query)
    // Fall back to a structural path approach that is guaranteed to be correct
    let mut path = build_structural_path(target_node, ctx.source);
    
    if path.is_empty() {
        let final_matches = matcher::run_query(&base_query, ctx.tree, ctx.source, ctx.language)?;
        if final_matches.len() != 1 {
            return Err(Error::AmbiguousQuery(format!(
                "Generated query matched {} nodes, expected 1. Try selecting a more specific range.",
                final_matches.len()
            )));
        }
        return Ok(base_query);
    }

    // Progressive disambiguation:
    // 1. Try structural path with only the target node named.
    // 2. Try structural path with target + 1st ancestor named, etc.
    
    // First, clear all names in the path to start with a pure structural query
    let mut names = Vec::new();
    for entry in &mut path {
        names.push(entry.name_info.take());
    }

    let depth = path.len();
    
    // Try increasing structural path depth
    for path_len in 1..=depth {
        let sub_path = &path[depth - path_len..depth];
        
        // Try with only target node named (highest priority)
        let mut named_target_path = sub_path.to_vec();
        let last = named_target_path.len() - 1;
        named_target_path[last].name_info = names[depth - 1].as_ref().map(|n| NameInfo {
            field: n.field.clone(),
            direct_type: n.direct_type.clone(),
            inner_type: n.inner_type.clone(),
            text: n.text.clone(),
        });

        // Anchor is a named ancestor (excluding the target node)
        let has_named_ancestor = |p: &[PathEntry]| {
             p.len() > 1 && p[0..p.len()-1].iter().any(|entry| entry.name_info.is_some())
        };

        let query = build_tier1_query(&named_target_path);
        let match_count = matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len();
        
        if match_count == 1 && (has_named_ancestor(&named_target_path) || path_len == depth) {
            return Ok(query);
        }

        // Try without any names (lower priority)
        let query = build_tier1_query(sub_path);
        let match_count = matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len();
        if match_count == 1 && (has_named_ancestor(&sub_path) || path_len == depth) {
            return Ok(query);
        }

        // Progressive disambiguation: try adding ancestor names one by one
        let mut named_path = named_target_path;
        for name_depth in 1..named_path.len() {
             let idx = named_path.len() - 1 - name_depth;
             named_path[idx].name_info = names[depth - 1 - name_depth].as_ref().map(|n| NameInfo {
                field: n.field.clone(),
                direct_type: n.direct_type.clone(),
                inner_type: n.inner_type.clone(),
                text: n.text.clone(),
            });

            let query = build_tier1_query(&named_path);
            let match_count = matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len();
            if match_count == 1 && (has_named_ancestor(&named_path) || path_len == depth) {
                return Ok(query);
            }
        }
    }

    // If still not unique, use the base_query if it was unique, 
    // otherwise return the best we could do (which might still be ambiguous)
    let final_query = build_tier1_query(&path);
    let final_matches = matcher::run_query(&final_query, ctx.tree, ctx.source, ctx.language)?;
    
    if final_matches.len() != 1 {
         return Err(Error::AmbiguousQuery(format!(
            "Generated query matched {} nodes, expected 1. Try selecting a more specific range.",
            final_matches.len()
        )));
    }

    Ok(final_query)
}

/// Given a specific AST node, generate a tree-sitter query for it.
pub fn generate_query_for_node(
    tree: &Tree,
    node: Node,
    source: &[u8],
    language: &Language,
) -> Result<GeneratedQuery> {
    generate_query(
        tree,
        source,
        (node.start_byte(), node.end_byte()),
        language,
    )
}

/// Helper to find a smaller child node that still contains the point.
fn find_tighter_child(node: Node, point: usize) -> Option<Node> {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.start_byte() <= point && child.end_byte() > point {
                return Some(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// Find the largest declaration node whose span is contained within the given byte range.
fn find_declaration_within(node: Node, byte_range: (usize, usize)) -> Option<Node> {
    let mut best: Option<Node> = None;

    fn search<'a>(node: Node<'a>, byte_range: (usize, usize), best: &mut Option<Node<'a>>) {
        // Skip nodes entirely outside the range
        if node.end_byte() <= byte_range.0 || node.start_byte() >= byte_range.1 {
            return;
        }

        // Check if this declaration fits within the user's range
        if DECLARATION_TYPES.contains(&node.kind())
            && node.start_byte() >= byte_range.0
            && node.end_byte() <= byte_range.1
        {
            // Prefer the largest declaration that fits
            if best.is_none_or(|b| {
                (node.end_byte() - node.start_byte()) > (b.end_byte() - b.start_byte())
            }) {
                *best = Some(node);
            }
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            search(child, byte_range, best);
        }
    }

    search(node, byte_range, &mut best);
    best
}

/// Walk up to the nearest named declaration node (function, class, struct, enum, etc).
fn walk_to_named_declaration(mut node: Node) -> Node {
    // If we're already on a non-local declaration, use it
    if DECLARATION_TYPES.contains(&node.kind()) && !is_local_declaration(node) {
        return node;
    }

    // Walk up to find the nearest non-local declaration
    while let Some(parent) = node.parent() {
        if DECLARATION_TYPES.contains(&parent.kind()) && !is_local_declaration(parent) {
            return parent;
        }
        // Stop at source_file
        if is_root_node(parent.kind()) {
            break;
        }
        node = parent;
    }

    // Fall back to the original node if no non-local declaration found
    node
}

/// Check if a declaration node is likely a local variable/constant.
fn is_local_declaration(node: Node) -> bool {
    let kind = node.kind();
    if kind != "property_declaration" && kind != "variable_declaration" && kind != "lexical_declaration" {
        return false;
    }

    // Check ancestors: if we're inside a function or closure, it's local
    let mut current = node;
    while let Some(parent) = current.parent() {
        let pk = parent.kind();
        if pk.contains("function") || pk.contains("method") || pk.contains("lambda") || pk == "closure_expression" {
            return true;
        }
        if is_root_node(pk) {
            break;
        }
        current = parent;
    }
    false
}

const DECLARATION_TYPES: &[&str] = &[
    // Common / Shared types
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "method_declaration",
    "constructor_declaration",
    "property_declaration",
    "type_alias_declaration",
    // Swift
    "function_declaration",
    "protocol_declaration",
    "init_declaration",
    "deinit_declaration",
    "subscript_declaration",
    "enum_entry",
    "protocol_function_declaration",
    // Rust
    "function_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "impl_item",
    "type_item",
    "const_item",
    "static_item",
    "mod_item",
    "macro_definition",
    // TypeScript
    "method_definition",
    "lexical_declaration",
    "export_statement",
    // Python
    "function_definition",
    "class_definition",
    "decorated_definition",
    // Go
    "type_declaration",
    "type_spec",
    "var_declaration",
    // Java (shared above)
    // C#
    "namespace_declaration",
    "record_declaration",
    "struct_declaration",
    // Dart
    "function_signature",
    "initialized_identifier",
    "enum_constant",
];

/// Build the structural path from the target node up to (but not including) the root.
/// Body nodes (class_body, etc.) are included to ensure the query nesting matches the AST.
/// Wrapper nodes (export_statement, decorated_definition) are skipped — they don't have
/// queryable name fields.
fn build_structural_path(target: Node, source: &[u8]) -> Vec<PathEntry> {
    let mut path = Vec::new();
    let mut current = target;

    loop {
        // Skip wrapper nodes that don't have structural meaning for queries
        if !is_wrapper_node(current.kind()) {
            let entry = PathEntry {
                node_type: current.kind().to_string(),
                name_info: if is_body_node(current.kind()) {
                    None
                } else {
                    extract_name_info(current, source)
                },
            };
            path.push(entry);
        }

        match current.parent() {
            Some(parent) if !is_root_node(parent.kind()) => {
                current = parent;
            }
            _ => break,
        }
    }

    path.reverse(); // outermost first
    path
}

/// Extract the "name" identifier from a node if it has one.
fn extract_name_info(node: Node, source: &[u8]) -> Option<NameInfo> {
    if node.kind() == "class_member" || node.kind() == "declaration" {
        return extract_nested_name(node, source);
    }
    extract_name_info_direct(node, source)
}

/// Helper to recursively search for a "name" field in a node's children.
fn extract_nested_name(node: Node, source: &[u8]) -> Option<NameInfo> {
    if let Some(mut info) = extract_name_info_direct(node, source) {
        info.field = None; // Field name is relative to child, not node
        return Some(info);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(info) = extract_nested_name(child, source) {
            return Some(info);
        }
    }
    None
}

/// Direct name extraction without recursion, to avoid infinite loops.
fn extract_name_info_direct(node: Node, source: &[u8]) -> Option<NameInfo> {
    // If the node itself is an identifier, it is its own name.
    // Literals (boolean_literal, etc.) should not be treated as names because they
    // are often not searchable as node types in the same way.
    if node.kind().contains("identifier") {
        return Some(NameInfo {
            field: None,
            direct_type: node.kind().to_string(),
            inner_type: None,
            text: node_text(node, source),
        });
    }

    // Try the "name" field first
    if let Some(name_node) = node.child_by_field_name("name") {
        // For Swift user_type nodes (extensions), we need nested matching
        if name_node.kind() == "user_type" {
            let mut cursor = name_node.walk();
            for child in name_node.named_children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return Some(NameInfo {
                        field: Some("name".to_string()),
                        direct_type: "user_type".to_string(),
                        inner_type: Some("type_identifier".to_string()),
                        text: node_text(child, source),
                    });
                }
            }
        }
        return Some(NameInfo {
            field: Some("name".to_string()),
            direct_type: name_node.kind().to_string(),
            inner_type: None,
            text: node_text(name_node, source),
        });
    }

    // For Rust impl_item: use "type" field as the name
    if node.kind() == "impl_item" && let Some(type_node) = node.child_by_field_name("type") {
        return Some(NameInfo {
            field: Some("type".to_string()),
            direct_type: type_node.kind().to_string(),
            inner_type: None,
            text: node_text(type_node, source),
        });
    }

    // For TS export_statement: get the name from the inner declaration
    if node.kind() == "export_statement" && let Some(decl) = node.child_by_field_name("declaration") {
        return extract_name_info_direct(decl, source);
    }

    // For Python decorated_definition: get the name from the inner definition
    if node.kind() == "decorated_definition" && let Some(def) = node.child_by_field_name("definition") {
        return extract_name_info_direct(def, source);
    }

    // For Swift enum_entry, try to find the name pattern
    if node.kind() == "enum_entry" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "simple_identifier" {
                return Some(NameInfo {
                    field: Some("name".to_string()),
                    direct_type: "simple_identifier".to_string(),
                    inner_type: None,
                    text: node_text(child, source),
                });
            }
        }
    }

    None
}

fn node_text(node: Node, source: &[u8]) -> String {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("").to_string()
}

fn is_body_node(kind: &str) -> bool {
    matches!(
        kind,
        // Swift
        "class_body"
            | "enum_class_body"
            | "protocol_body"
            | "function_body"
            | "statements"
            | "computed_getter"
            // Rust
            | "declaration_list"
            | "field_declaration_list"
            | "enum_variant_list"
            | "block"
            // TypeScript
            | "interface_body"
            | "enum_body"
            | "statement_block"
            | "object_type"
            // Python (block already listed)
            // Go
            | "interface_type"
            | "struct_type"
            // Java / C#
            | "constructor_body"
            | "enum_body_declarations"
            // Dart
            | "class_member"
            | "declaration"
    )
}

/// Wrapper nodes that should be skipped in the structural path.
fn is_wrapper_node(kind: &str) -> bool {
    matches!(kind, "export_statement" | "decorated_definition")
}

/// Root node types across languages.
fn is_root_node(kind: &str) -> bool {
    matches!(kind, "source_file" | "program" | "module" | "compilation_unit")
}

/// Build a Tier 1 (exact) S-expression query from the structural path.
fn build_tier1_query(path: &[PathEntry]) -> String {
    if path.is_empty() {
        return String::new();
    }

    let depth = path.len();
    let mut capture_counter = 0;

    // Build the query recursively: outermost node first, target node last.
    // Predicates go inside each node's pattern, before the closing paren.
    fn build_node(
        path: &[PathEntry],
        idx: usize,
        depth: usize,
        indent: usize,
        counter: &mut usize,
    ) -> String {
        let entry = &path[idx];
        let is_target = idx == depth - 1;
        let pad = "  ".repeat(indent);
        let mut s = format!("{pad}({}", entry.node_type);

        // Name field with text predicate
        let mut inner_predicate = String::new();
        let mut outer_predicate = String::new();
        if let Some(ref info) = entry.name_info {
            let capture_name = if is_target {
                "fn_name".to_string()
            } else {
                let name = format!("name{}", *counter);
                *counter += 1;
                name
            };
            if let Some(ref inner_type) = info.inner_type {
                // Nested name: e.g., name: (user_type (type_identifier) @capture)
                if let Some(ref field_name) = info.field {
                    s.push_str(&format!(
                        "\n{pad}  {}: ({} ({inner_type}) @{capture_name})",
                        field_name, info.direct_type
                    ));
                } else {
                    s.push_str(&format!(
                        "\n{pad}  ({} ({inner_type}) @{capture_name})",
                        info.direct_type
                    ));
                }
            } else {
                if let Some(ref field_name) = info.field {
                    s.push_str(&format!(
                        "\n{pad}  {}: ({}) @{capture_name}",
                        field_name, info.direct_type
                    ));
                } else {
                    // Match anywhere inside - handled by outer_predicate on target
                    if !is_target {
                         s.push_str(&format!(
                            "\n{pad}  (_) @{capture_name}"
                        ));
                    }
                }
            }
            if info.field.is_none() {
                // If it's a descendant name, match against the whole node text.
                // We use #eq? for exact match which is safer than regex #match? for code blocks.
                let cap = if is_target { "target" } else { &capture_name };
                outer_predicate = format!("\n{pad}  (#eq? @{} \"{}\")", cap, escape_query_text(&info.text));
            } else {
                inner_predicate = format!("\n{pad}  (#eq? @{} \"{}\")", capture_name, escape_query_text(&info.text));
            };
        }

        if is_target {
            // Add inner predicate and close inner node
            s.push_str(&inner_predicate);
            s.push(')');
            // Wrap in extra parens if we have an outer predicate or to safely attach @target
            s = format!("{pad}({} @target{}", &s[pad.len()..], outer_predicate);
            s.push(')');
        } else {
            // Add inner predicate, then nest the child
            s.push_str(&inner_predicate);
            let child_str = build_node(path, idx + 1, depth, indent + 1, counter);
            s.push('\n');
            s.push_str(&child_str);
            s.push(')');
            // Add outer predicate (though we don't expect them for non-targets yet)
            s.push_str(&outer_predicate);
        }

        s
    }

    build_node(path, 0, depth, 0, &mut capture_counter)
}

fn escape_query_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Extract semantic information from a node for fine-grained targeting.
fn extract_semantic_info(node: Node, source: &[u8]) -> Option<SemanticInfo> {
    match node.kind() {
        "if_statement" | "if_expression" => {
            // Extract the condition
            if let Some(cond) = node.child_by_field_name("condition") {
                let text = node_text(cond, source);
                return Some(SemanticInfo::IfCondition(text));
            }
        }
        "call_expression" | "macro_invocation" => {
            // Extract the function being called
            if let Some(func) = node.child_by_field_name("function").or_else(|| node.named_child(0)) {
                let text = node_text(func, source);
                return Some(SemanticInfo::CallTarget(text));
            }
        }
        "assignment_expression" | "assignment_statement" => {
            // Extract the variable being assigned
            if let Some(left) = node.child_by_field_name("left") {
                let text = node_text(left, source);
                return Some(SemanticInfo::AssignmentTarget(text));
            }
        }
        "return_statement" => {
            // Extract the return value
            if let Some(value) = node.child_by_field_name("value") {
                let text = node_text(value, source);
                if !text.trim().is_empty() {
                    return Some(SemanticInfo::ReturnValue(text));
                }
            }
        }
        _ => {}
    }
    None
}

/// Extract an identifier from a node for fallback naming.
fn extract_identifier_from_node(node: Node, source: &[u8]) -> Option<String> {
    // Try to find any identifier child
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind().contains("identifier") {
            return Some(node_text(child, source));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::{Language as CodemarkLang, Parser};

    fn parse_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/swift/{name}"));
        let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_declaration"
                && let Some(name_node) = node.child_by_field_name("name")
            {
                let text = &source[name_node.byte_range()];
                if text == name {
                    return Some((node.start_byte(), node.end_byte()));
                }
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in fixture"))
    }

    #[test]
    fn generate_query_for_top_level_function() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let range = find_function_byte_range(&tree, &source, "createDefaultAuthService");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_node_type, "function_declaration");
        assert_eq!(result.target_name.as_deref(), Some("createDefaultAuthService"));

        // Verify the generated query finds exactly the right node
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].node_text.contains("createDefaultAuthService"));
    }

    #[test]
    fn generate_query_for_class_method() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let range = find_function_byte_range(&tree, &source, "validateToken");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_node_type, "function_declaration");
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].node_text.contains("validateToken"));
    }

    #[test]
    fn generate_query_for_private_method() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let range = find_function_byte_range(&tree, &source, "decode");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn generate_query_for_extension_method() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let range = find_function_byte_range(&tree, &source, "invalidateCache");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("invalidateCache"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn generated_query_round_trips() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        // For each function in the fixture, generate a query and verify it matches
        let functions = [
            "validateToken",
            "refreshToken",
            "decode",
            "encode",
            "checkPermission",
            "invalidateCache",
            "cacheSize",
            "createDefaultAuthService",
        ];

        for func_name in functions {
            let range = find_function_byte_range(&tree, &source, func_name);
            let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
            let matches =
                matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();

            assert!(
                !matches.is_empty(),
                "query for '{func_name}' returned no matches: {}",
                result.query
            );
            // Verify the match covers the original byte range
            let m = &matches[0];
            assert!(
                m.byte_range.0 <= range.0 && m.byte_range.1 >= range.1,
                "match for '{func_name}' doesn't cover original range"
            );
        }
    }

    // --- Rust tests ---

    fn parse_rust_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/rust/{name}"));
        let mut parser = Parser::new(CodemarkLang::Rust).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_rust_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_item"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Rust fixture"))
    }

    #[test]
    fn rust_top_level_function() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let range = find_rust_function_byte_range(&tree, &source, "create_default_auth_service");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_node_type, "function_item");
        assert_eq!(result.target_name.as_deref(), Some("create_default_auth_service"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn rust_impl_method() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let range = find_rust_function_byte_range(&tree, &source, "decode");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
        assert!(matches[0].node_text.contains("fn decode"));
    }

    #[test]
    fn rust_trait_impl_method() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        // validate_token appears both in the trait and in the impl
        let range = find_rust_function_byte_range(&tree, &source, "validate_token");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        // Should match at least 1 (may match trait decl too if query isn't precise enough)
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn rust_generic_function() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let range = find_rust_function_byte_range(&tree, &source, "validate_and_check");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validate_and_check"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn rust_round_trips() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let functions = [
            "new",
            "decode",
            "encode",
            "check_permission",
            "create_default_auth_service",
            "validate_and_check",
        ];

        for func_name in functions {
            let range = find_rust_function_byte_range(&tree, &source, func_name);
            let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
            let matches =
                matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();

            assert!(
                !matches.is_empty(),
                "query for '{func_name}' returned no matches: {}",
                result.query
            );
        }
    }

    // --- TypeScript tests ---

    fn parse_ts_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/typescript/{name}"));
        let mut parser = Parser::new(CodemarkLang::TypeScript).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_ts_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if (kind == "function_declaration" || kind == "method_definition")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in TS fixture"))
    }

    #[test]
    fn ts_top_level_function() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let range = find_ts_function_byte_range(&tree, &source, "validateAndCheck");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateAndCheck"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn ts_class_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let range = find_ts_function_byte_range(&tree, &source, "validateToken");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn ts_private_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let range = find_ts_function_byte_range(&tree, &source, "decode");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Python tests ---

    fn parse_py_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/python/{name}"));
        let mut parser = Parser::new(CodemarkLang::Python).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_py_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if kind == "function_definition"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Python fixture"))
    }

    #[test]
    fn py_top_level_function() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "create_default_auth_service");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("create_default_auth_service"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn py_class_method() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "validate_token");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn py_private_method() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "_decode");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("_decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn py_decorated_function() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "require_auth");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("require_auth"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Go tests ---

    fn parse_go_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/go/{name}"));
        let mut parser = Parser::new(CodemarkLang::Go).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_go_function_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if (kind == "function_declaration" || kind == "method_declaration")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Go fixture"))
    }

    #[test]
    fn go_free_function() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let range = find_go_function_range(&tree, &source, "CreateDefaultAuthService");
        let lang = CodemarkLang::Go.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("CreateDefaultAuthService"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn go_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let range = find_go_function_range(&tree, &source, "ValidateToken");
        let lang = CodemarkLang::Go.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn go_private_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let range = find_go_function_range(&tree, &source, "decode");
        let lang = CodemarkLang::Go.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Java tests ---

    fn parse_java_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/java/{name}"));
        let mut parser = Parser::new(CodemarkLang::Java).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_java_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if (node.kind() == "method_declaration" || node.kind() == "constructor_declaration")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, method_name)
            .unwrap_or_else(|| panic!("method '{method_name}' not found in Java fixture"))
    }

    #[test]
    fn java_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_range(&tree, &source, "validateToken");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_private_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_range(&tree, &source, "decode");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_static_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_range(&tree, &source, "createDefault");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("createDefault"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- C# tests ---

    fn parse_csharp_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/csharp/{name}"));
        let mut parser = Parser::new(CodemarkLang::CSharp).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    fn find_csharp_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "method_declaration"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, method_name)
            .unwrap_or_else(|| panic!("method '{method_name}' not found in C# fixture"))
    }

    #[test]
    fn csharp_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_range(&tree, &source, "ValidateToken");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("ValidateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_private_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_range(&tree, &source, "Decode");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("Decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_static_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_range(&tree, &source, "CreateDefault");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("CreateDefault"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Dart tests ---

    fn parse_dart_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/dart/{name}"));
        let mut parser = Parser::new(CodemarkLang::Dart).unwrap();
        parser.parse_file(&fixture).unwrap()
    }

    #[test]
    fn dart_top_level_function() {
        let (tree, source) = parse_dart_fixture("auth_service.dart");
        // function_signature has the name
        let offset = source.find("createDefaultAuthService").unwrap();
        let range = (offset, offset + 10);
        let lang = CodemarkLang::Dart.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("createDefaultAuthService"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn dart_class_method() {
        let (tree, source) = parse_dart_fixture("auth_service.dart");
        let offset = source.find("Claims _decode").unwrap();
        let range = (offset, offset + 10);
        let lang = CodemarkLang::Dart.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn dart_enum() {
        let (tree, source) = parse_dart_fixture("auth_service.dart");
        let offset = source.find("enum AuthError").unwrap();
        let range = (offset, offset + 10);
        let lang = CodemarkLang::Dart.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("AuthError"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Range precision tests: method range should target method, not class ---

    #[test]
    fn swift_exact_range_targets_method_not_class() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        // Get the exact byte range of validateToken
        let range = find_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "function_declaration",
            "should target function_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn rust_exact_range_targets_method_not_impl() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let range = find_rust_function_byte_range(&tree, &source, "decode");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "should target function_item, not impl_item"
        );
        assert_eq!(result.target_name.as_deref(), Some("decode"));
    }

    #[test]
    fn ts_exact_range_targets_method_not_class() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let range = find_ts_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "method_definition",
            "should target method_definition, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn py_exact_range_targets_method_not_class() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let range = find_py_function_byte_range(&tree, &source, "validate_token");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "function_definition",
            "should target function_definition, not class_definition"
        );
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));
    }

    #[test]
    fn go_exact_range_targets_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let lang = CodemarkLang::Go.tree_sitter_language();

        let range = find_go_function_range(&tree, &source, "ValidateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration"
        );
    }

    #[test]
    fn java_exact_range_targets_method_not_class() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let lang = CodemarkLang::Java.tree_sitter_language();

        let range = find_java_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn single_line_inside_method_targets_anchored_declaration() {
        // A single line inside a method should target the enclosing method
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        // Line 50 is inside the decode function body
        let line_50_start = source.lines().take(49).map(|l| l.len() + 1).sum::<usize>();
        let line_50_end = line_50_start + source.lines().nth(49).unwrap_or("").len();

        let result =
            generate_query(&tree, source.as_bytes(), (line_50_start, line_50_end), &lang).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "single line inside method should target the enclosing declaration"
        );
    }
}
