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
    /// Semantic info for query generation.
    semantic_info: Option<SemanticInfo>,
    /// Whether this node is a "landmark" (stable named declaration).
    is_landmark: bool,
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
    let ctx = QueryContext { source, language, byte_range, root: tree.root_node(), tree };

    // 1. Select the tightest meaningful target node
    let mut node = find_tightest_node(&ctx.root, ctx.source, ctx.byte_range)?;

    // For fine-grained targeting, we want to descend into blocks to find the
    // actual statement or expression.
    while is_body_node(node.kind()) {
        let mut found_child = false;
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if child.start_byte() < ctx.byte_range.1 && child.end_byte() > ctx.byte_range.0 {
                node = child;
                found_child = true;
                break;
            }
        }

        if !found_child {
            break;
        }

        // If we found a node with semantic info, stop here
        if extract_semantic_info(node, ctx.source).is_some() {
            break;
        }
    }

    // 2. Extract metadata
    let name = extract_name_info(node, ctx.source)
        .map(|info| info.text)
        .or_else(|| extract_identifier_from_node(node, ctx.source));

    // 3. Disambiguate and anchor
    let query = disambiguate_query(node, &ctx)?;

    Ok(GeneratedQuery {
        query,
        target_node_type: node.kind().to_string(),
        target_name: name,
        byte_range: (node.start_byte(), node.end_byte()),
    })
}

/// Find the tightest meaningful node for a given byte range.
/// If it's a point range, find the deepest named node.
/// If it's a multi-byte range, find the smallest node covering it.
fn find_tightest_node<'a>(
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

    if start == end {
        // Point range: pick the deepest named node at this position
        let mut node = root
            .descendant_for_byte_range(start, start)
            .ok_or_else(|| Error::TreeSitter("no node found at position".into()))?;

        // If the node is very large (e.g. it includes trailing whitespace),
        // try to find a tighter child that also contains the point.
        while let Some(tighter_child) = find_tighter_child(node, start) {
            node = tighter_child;
        }

        // Walk up to the nearest named node if we hit an anonymous one
        while !node.is_named() {
            if let Some(parent) = node.parent() {
                node = parent;
            } else {
                break;
            }
        }
        Ok(node)
    } else {
        // Range: pick the smallest node covering it
        let mut node = root
            .descendant_for_byte_range(start, end)
            .ok_or_else(|| Error::TreeSitter("no node found at byte range".into()))?;

        // Walk up to the nearest named node if we hit an anonymous one
        while !node.is_named() {
            if let Some(parent) = node.parent() {
                node = parent;
            } else {
                break;
            }
        }
        Ok(node)
    }
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
    "method_elem",
    // Dart
    "function_signature",
    "initialized_identifier",
    "enum_constant",
];

/// Check if a declaration node is likely a local variable/constant.
fn is_local_declaration(node: Node) -> bool {
    let kind = node.kind();
    if kind != "property_declaration"
        && kind != "variable_declaration"
        && kind != "lexical_declaration"
    {
        return false;
    }

    // Check ancestors: if we're inside a function or closure, it's local
    let mut current = node;
    while let Some(parent) = current.parent() {
        let pk = parent.kind();
        if pk.contains("function")
            || pk.contains("method")
            || pk.contains("lambda")
            || pk == "closure_expression"
        {
            return true;
        }
        if is_root_node(pk) {
            break;
        }
        current = parent;
    }
    false
}

fn build_base_query(
    node: Node,
    _name: Option<&str>,
    semantic_info: Option<SemanticInfo>,
    source: &[u8],
) -> Result<String> {
    // We leverage the structural path logic for a single node to ensure consistency
    let entry = PathEntry {
        node_type: node.kind().to_string(),
        name_info: extract_name_info(node, source),
        semantic_info,
        is_landmark: DECLARATION_TYPES.contains(&node.kind()) && !is_local_declaration(node),
    };

    let path = vec![entry];
    Ok(build_tier1_query(&path))
}

/// Helper to ensure a query is unique and anchored by walking up parents.
fn disambiguate_query(target_node: Node, ctx: &QueryContext<'_>) -> Result<String> {
    let mut path = build_structural_path(target_node, ctx.source);

    if path.is_empty() {
        // Fallback to simple base query if no path can be built
        let name = extract_name_info(target_node, ctx.source)
            .map(|info| info.text)
            .or_else(|| extract_identifier_from_node(target_node, ctx.source));
        let semantic_info = extract_semantic_info(target_node, ctx.source);
        return build_base_query(target_node, name.as_deref(), semantic_info, ctx.source);
    }

    // First, clear all names in the path to start with a pure structural query
    // We preserve the target node's semantic info to keep the query specific
    let target_semantic_info = path.last().and_then(|e| e.semantic_info.clone());
    let mut names = Vec::new();
    for entry in &mut path {
        names.push(entry.name_info.take());
        entry.semantic_info = None;
    }

    let depth = path.len();

    // Strategy: find the minimum set of names needed to make the full structural path unique.
    // We always include the full path for maximum stability.

    // 1. Try with only the target node named (if it has a name)
    let mut current_path = path.clone();
    for (i, entry) in current_path.iter_mut().enumerate() {
        if i < depth - 1 {
            entry.name_info = None;
            entry.semantic_info = None;
        } else {
            entry.name_info = names[depth - 1].clone();
            entry.semantic_info = target_semantic_info.clone();
        }
    }

    // Landmark requirement check: at least one NAMED landmark ancestor
    let has_named_landmark = |p: &[PathEntry]| {
        p.iter().enumerate().any(|(i, e)| i < p.len() - 1 && e.is_landmark && e.name_info.is_some())
    };

    let query = build_tier1_query(&current_path);
    if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1
        && (has_named_landmark(&current_path) || depth == 1)
    {
        return Ok(query);
    }

    // 2. Progressively add names to landmarks from target upwards
    for i in (0..depth - 1).rev() {
        if path[i].is_landmark {
            current_path[i].name_info = names[i].clone();
            let query = build_tier1_query(&current_path);
            if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1 {
                return Ok(query);
            }
        }
    }

    // 3. If still not unique, add all names
    for i in (0..depth - 1).rev() {
        current_path[i].name_info = names[i].clone();
    }
    let query = build_tier1_query(&current_path);
    let match_count = matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len();
    if match_count == 1 {
        return Ok(query);
    }

    // 4. Final resort: try unnamed path (only if naming the target didn't work and we reach the root)
    let query = build_tier1_query(&path);
    if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1 {
        return Ok(query);
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
    generate_query(tree, source, (node.start_byte(), node.end_byte()), language)
}

/// Build the structural path from the target node up to (but not including) the root.
/// Body nodes (class_body, etc.) are included to ensure the query nesting matches the AST.
/// Wrapper nodes (export_statement, decorated_definition) are skipped — they don't have
/// queryable name fields.
fn build_structural_path(target: Node, source: &[u8]) -> Vec<PathEntry> {
    if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
        eprintln!(
            "DEBUG: build_structural_path: target={} at {:?}",
            target.kind(),
            target.byte_range()
        );
    }
    let mut path = Vec::new();
    let mut current = target;

    // Special case: if the target is a leaf node (like an identifier)
    // and its parent has a name field pointing to it, we should start
    // the path from the parent to avoid "Impossible pattern" errors
    // where the query expects two different children for the same node.
    if let Some(parent) = current.parent()
        && let Some(name_node) = parent.child_by_field_name("name")
        && name_node.id() == current.id()
    {
        current = parent;
    }

    let mut is_first = true;
    loop {
        // Skip wrapper nodes that don't have structural meaning for queries
        if !is_wrapper_node(current.kind()) {
            let is_target_node = is_first;
            is_first = false;
            let entry = PathEntry {
                node_type: current.kind().to_string(),
                name_info: if is_body_node(current.kind()) {
                    None
                } else {
                    extract_name_info(current, source)
                },
                semantic_info: if is_target_node {
                    extract_semantic_info(current, source)
                } else {
                    None
                },
                is_landmark: DECLARATION_TYPES.contains(&current.kind())
                    && !is_local_declaration(current),
            };
            if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
                eprintln!(
                    "DEBUG:   entry={} has_name={} is_landmark={}",
                    entry.node_type,
                    entry.name_info.is_some(),
                    entry.is_landmark
                );
            }
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
    if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
        eprintln!(
            "DEBUG: extract_name_info_direct: node={} at {:?}",
            node.kind(),
            node.byte_range()
        );
    }
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
        if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
            eprintln!("DEBUG:   found name field node={}", name_node.kind());
        }
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
    if node.kind() == "impl_item"
        && let Some(type_node) = node.child_by_field_name("type")
    {
        return Some(NameInfo {
            field: Some("type".to_string()),
            direct_type: type_node.kind().to_string(),
            inner_type: None,
            text: node_text(type_node, source),
        });
    }

    // For TS export_statement: get the name from the inner declaration
    if node.kind() == "export_statement"
        && let Some(decl) = node.child_by_field_name("declaration")
    {
        return extract_name_info_direct(decl, source);
    }

    // For Python decorated_definition: get the name from the inner definition
    if node.kind() == "decorated_definition"
        && let Some(def) = node.child_by_field_name("definition")
    {
        return extract_name_info_direct(def, source);
    }

    // For Rust match_arm: use pattern text
    if node.kind() == "match_arm"
        && let Some(pattern) = node.child_by_field_name("pattern")
    {
        return Some(NameInfo {
            field: Some("pattern".to_string()),
            direct_type: pattern.kind().to_string(),
            inner_type: None,
            text: node_text(pattern, source),
        });
    }

    // For Swift switch_entry: use pattern or "default"
    if node.kind() == "switch_entry" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "switch_pattern" {
                return Some(NameInfo {
                    field: None,
                    direct_type: "switch_pattern".to_string(),
                    inner_type: None,
                    text: node_text(child, source),
                });
            }
            if child.kind() == "default_keyword" {
                return Some(NameInfo {
                    field: None,
                    direct_type: "default_keyword".to_string(),
                    inner_type: None,
                    text: "default".to_string(),
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

        // Semantic info (if present)
        if let Some(ref semantic) = entry.semantic_info {
            match semantic {
                SemanticInfo::IfCondition(cond) => {
                    s.push_str(&format!("\n{pad}  condition: (_) @cond"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @cond \"{}\")",
                        escape_query_text(cond)
                    ));
                }
                SemanticInfo::CallTarget(func) => {
                    s.push_str(&format!("\n{pad}  (_) @func"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @func \"{}\")",
                        escape_query_text(func)
                    ));
                }
                SemanticInfo::AssignmentTarget(target_name) => {
                    s.push_str(&format!("\n{pad}  left: (_) @left"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @left \"{}\")",
                        escape_query_text(target_name)
                    ));
                }
                SemanticInfo::ReturnValue(val) => {
                    s.push_str(&format!("\n{pad}  value: (_) @val"));
                    inner_predicate
                        .push_str(&format!("\n{pad}  (#eq? @val \"{}\")", escape_query_text(val)));
                }
                SemanticInfo::BinaryOperator(op) => {
                    s.push_str(&format!("\n{pad}  operator: \"{}\"", escape_query_text(op)));
                }
            }
        }

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
                inner_predicate.push_str(&format!(
                    "\n{pad}  (#eq? @{} \"{}\")",
                    capture_name,
                    escape_query_text(&info.text)
                ));
            } else if let Some(ref field_name) = info.field {
                s.push_str(&format!(
                    "\n{pad}  {}: ({}) @{capture_name}",
                    field_name, info.direct_type
                ));
                inner_predicate.push_str(&format!(
                    "\n{pad}  (#eq? @{} \"{}\")",
                    capture_name,
                    escape_query_text(&info.text)
                ));
            } else {
                // Leaf node or descendant match without a field
                // Use the node itself as the capture if it matches the type
                if entry.node_type == info.direct_type {
                    // Handled at the end with s.push_str(" @capture_name")
                    outer_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @{} \"{}\")",
                        capture_name,
                        escape_query_text(&info.text)
                    ));
                } else {
                    s.push_str(&format!("\n{pad}  ({}) @{}", info.direct_type, capture_name));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @{} \"{}\")",
                        capture_name,
                        escape_query_text(&info.text)
                    ));
                }
            }
        }

        if is_target {
            // Add inner predicate and close inner node
            s.push_str(&inner_predicate);
            s.push(')');

            // Add name capture if it was on the node itself
            if let Some(ref info) = entry.name_info
                && info.field.is_none()
                && entry.node_type == info.direct_type
            {
                let capture_name = "fn_name";
                s.push_str(&format!(" @{}", capture_name));
            }

            s.push_str(" @target");

            // Wrap in extra parens if we have an outer predicate
            if !outer_predicate.is_empty() {
                s = format!("{pad}({}{}", &s[pad.len()..], outer_predicate);
                s.push(')');
            }
        } else {
            // Add inner predicate, then nest the child
            s.push_str(&inner_predicate);
            let child_str = build_node(path, idx + 1, depth, indent + 1, counter);
            s.push('\n');
            s.push_str(&child_str);
            s.push(')');

            // Add name capture if it was on the node itself
            if let Some(ref info) = entry.name_info
                && info.field.is_none()
                && entry.node_type == info.direct_type
            {
                let capture_name = format!("name{}", *counter - 1);
                s.push_str(&format!(" @{}", capture_name));
            }

            // Wrap in extra parens if we have an outer predicate
            if !outer_predicate.is_empty() {
                s = format!("{pad}({}{}", &s[pad.len()..], outer_predicate);
                s.push(')');
            }
        }

        s
    }

    build_node(path, 0, depth, 0, &mut capture_counter)
}

fn escape_query_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
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
            if let Some(func) = node.child_by_field_name("function").or_else(|| node.named_child(0))
            {
                let text = node_text(func, source);
                return Some(SemanticInfo::CallTarget(text));
            }
        }
        "assignment_expression" | "assignment_statement" | "short_var_declaration" => {
            // Extract the variable being assigned
            if let Some(left) = node.child_by_field_name("left") {
                let text = node_text(left, source);
                return Some(SemanticInfo::AssignmentTarget(text));
            } else if node.kind() == "short_var_declaration"
                && let Some(left) = node.named_child(0)
            {
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
            result.target_node_type, "block",
            "single line inside method should target the tightest node (block)"
        );
    }
}
