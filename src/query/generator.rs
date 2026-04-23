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

/// A selected target node with its metadata.
pub struct TargetNode<'a> {
    pub node: Node<'a>,
    pub name: Option<String>,
    pub semantic_info: Option<SemanticInfo>,
}

/// Semantic information that can distinguish a node from others of the same type.
#[derive(Clone, Debug)]
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

/// Strategy for generating tree-sitter queries.
pub trait QueryStrategy: Send + Sync {
    /// Select the target node for a given byte range.
    fn select_target<'a>(&self, ctx: &QueryContext<'a>) -> Result<TargetNode<'a>>;

    /// Build a query from the target node.
    fn build_query<'a>(
        &self,
        ctx: &QueryContext<'a>,
        target: &TargetNode<'a>,
    ) -> Result<String>;
}

/// One entry in the structural path from root to target.
#[derive(Debug)]
struct PathEntry {
    node_type: String,
    /// Name info for query generation.
    name_info: Option<NameInfo>,
}

/// How to query for the "name" of a node.
#[derive(Debug)]
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

/// Available strategies for query generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyType {
    /// Target named declarations (functions, classes, etc.)
    Declaration,
    /// Target specific statements or expressions.
    FineGrained,
}

/// Given a parsed tree and a byte range, generate a tree-sitter query that uniquely
/// identifies the target node.
pub fn generate_query(
    tree: &Tree,
    source: &[u8],
    byte_range: (usize, usize),
    language: &Language,
    strategy_type: StrategyType,
) -> Result<GeneratedQuery> {
    match strategy_type {
        StrategyType::Declaration => generate_query_with_strategy(
            tree,
            source,
            byte_range,
            language,
            &DeclarationStrategy,
        ),
        StrategyType::FineGrained => generate_query_with_strategy(
            tree,
            source,
            byte_range,
            language,
            &FineGrainedStrategy,
        ),
    }
}

/// Generate a query using a specific strategy.
pub fn generate_query_with_strategy(
    tree: &Tree,
    source: &[u8],
    byte_range: (usize, usize),
    language: &Language,
    strategy: &impl QueryStrategy,
) -> Result<GeneratedQuery> {
    let ctx = QueryContext {
        source,
        language,
        byte_range,
        root: tree.root_node(),
        tree,
    };

    // Select the target node
    let target = strategy.select_target(&ctx)?;

    // Build the query
    let query = strategy.build_query(&ctx, &target)?;

    // Validate uniqueness
    let matches = matcher::run_query(&query, tree, source, language)?;
    if matches.len() != 1 {
        return Err(Error::AmbiguousQuery(format!(
            "Generated query matched {} nodes, expected 1",
            matches.len()
        )));
    }

    Ok(GeneratedQuery {
        query,
        target_node_type: target.node.kind().to_string(),
        target_name: target.name.clone(),
        byte_range: (target.node.start_byte(), target.node.end_byte()),
    })
}

/// Given a specific AST node, generate a tree-sitter query for it.
pub fn generate_query_for_node(
    tree: &Tree,
    node: Node,
    source: &[u8],
    language: &Language,
) -> Result<GeneratedQuery> {
    generate_query_with_strategy(
        tree,
        source,
        (node.start_byte(), node.end_byte()),
        language,
        &DeclarationStrategy,
    )
}

/// Find the smallest named node that spans the given byte range.
fn find_target_node<'a>(root: &Node<'a>, source: &[u8], byte_range: (usize, usize)) -> Result<Node<'a>> {
    // Trim whitespace from the range
    let mut start = byte_range.0;
    let mut end = byte_range.1;

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

    // Otherwise, walk up from the deepest node to the nearest declaration.
    Ok(walk_to_named_declaration(node))
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

const DECLARATION_TYPES: &[&str] = &[
    // Swift
    "function_declaration",
    "class_declaration",
    "protocol_declaration",
    "property_declaration",
    "init_declaration",
    "deinit_declaration",
    "subscript_declaration",
    "typealias_declaration",
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
    "interface_declaration",
    "enum_declaration",
    "type_alias_declaration",
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
    "method_declaration",
    "var_declaration",
    // Java
    "method_declaration",
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "constructor_declaration",
    // C#
    "namespace_declaration",
    "record_declaration",
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "method_declaration",
    "constructor_declaration",
    "struct_declaration",
    // Dart
    "function_signature",
    "initialized_identifier",
    "enum_constant",
];

/// Walk up to the nearest named declaration node (function, class, struct, enum, etc).
fn walk_to_named_declaration(mut node: Node) -> Node {
    // If we're already on a declaration, use it
    if DECLARATION_TYPES.contains(&node.kind()) {
        return node;
    }

    // Walk up to find the nearest declaration
    while let Some(parent) = node.parent() {
        if DECLARATION_TYPES.contains(&parent.kind()) {
            return parent;
        }
        // Stop at source_file
        if parent.kind() == "source_file" {
            break;
        }
        node = parent;
    }

    // Fall back to the original node
    node
}

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
                // If it's a descendant name, match against the whole node text
                // using a word-boundary-like match to be safe.
                // This MUST be an outer predicate for the target.
                let cap = if is_target { "target" } else { &capture_name };
                outer_predicate = format!("\n{pad}  (#match? @{} \"\\\\b{}\\\\b\")", cap, escape_query_text(&info.text));
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
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

// ============================================================================
// Query Strategy Implementations
// ============================================================================

/// The default declaration-based query strategy.
///
/// This strategy targets named declarations (functions, classes, etc.)
/// and builds structural queries using ancestor chains.
pub struct DeclarationStrategy;

impl QueryStrategy for DeclarationStrategy {
    fn select_target<'a>(&self, ctx: &QueryContext<'a>) -> Result<TargetNode<'a>> {
        let target_node = find_target_node(&ctx.root, ctx.source, ctx.byte_range)?;

        Ok(TargetNode {
            node: target_node,
            name: extract_name_info(target_node, ctx.source).map(|info| info.text),
            semantic_info: None,
        })
    }

    fn build_query<'a>(
        &self,
        ctx: &QueryContext<'a>,
        target: &TargetNode<'a>,
    ) -> Result<String> {
        let path = build_structural_path(target.node, ctx.source);
        Ok(build_tier1_query(&path))
    }
}

/// A fine-grained targeting strategy that targets statements and expressions.
///
/// This strategy targets the actual node containing the user's range
/// rather than walking up to a declaration.
pub struct FineGrainedStrategy;

impl QueryStrategy for FineGrainedStrategy {
    fn select_target<'a>(&self, ctx: &QueryContext<'a>) -> Result<TargetNode<'a>> {
        // Find the deepest node containing the range
        let mut node = ctx
            .root
            .descendant_for_byte_range(ctx.byte_range.0, ctx.byte_range.1)
            .ok_or_else(|| Error::TreeSitter("no node found at byte range".into()))?;

        // For fine-grained targeting, we want to descend into blocks to find the 
        // actual statement or expression, even if the user's selection includes 
        // surrounding whitespace that technically forces a larger parent node.
        while is_body_node(node.kind()) {
            let mut found_child = false;
            let mut cursor = node.walk();

            for child in node.named_children(&mut cursor) {
                // If child overlaps with the selection, it's a better target
                if child.start_byte() < ctx.byte_range.1 && child.end_byte() > ctx.byte_range.0 {
                    node = child;
                    found_child = true;
                    break;
                }
            }

            if !found_child {
                break;
            }
        }

        // Extract semantic information for disambiguation
        let semantic_info = extract_semantic_info(node, ctx.source);
        let name = extract_name_info(node, ctx.source)
            .map(|info| info.text)
            .or_else(|| extract_identifier_from_node(node, ctx.source));

        Ok(TargetNode {
            node,
            name,
            semantic_info,
        })
    }

    fn build_query<'a>(
        &self,
        _ctx: &QueryContext<'a>,
        target: &TargetNode<'a>,
    ) -> Result<String> {
        if let Some(ref semantic) = target.semantic_info {
            match semantic {
                SemanticInfo::IfCondition(cond) => {
                    return Ok(format!(
                        "({} condition: (_) @cond) @target\n  (#eq? @cond \"{}\")",
                        target.node.kind(),
                        escape_query_text(cond)
                    ));
                }
                SemanticInfo::CallTarget(func) => {
                    return Ok(format!(
                        "({} function: (_) @func) @target\n  (#eq? @func \"{}\")",
                        target.node.kind(),
                        escape_query_text(func)
                    ));
                }
                SemanticInfo::AssignmentTarget(target_name) => {
                    return Ok(format!(
                        "({} left: (_) @left) @target\n  (#eq? @left \"{}\")",
                        target.node.kind(),
                        escape_query_text(target_name)
                    ));
                }
                SemanticInfo::ReturnValue(val) => {
                    return Ok(format!(
                        "({} value: (_) @val) @target\n  (#eq? @val \"{}\")",
                        target.node.kind(),
                        escape_query_text(val)
                    ));
                }
                SemanticInfo::BinaryOperator(op) => {
                    return Ok(format!(
                        "({} operator: \"{}\") @target",
                        target.node.kind(),
                        escape_query_text(op)
                    ));
                }
            }
        }

        if let Some(ref name) = target.name {
            // For leaf nodes (identifiers, literals), add a text match predicate
            if target.node.named_child_count() == 0 {
                return Ok(format!(
                    "({}) @target\n  (#eq? @target \"{}\")",
                    target.node.kind(),
                    escape_query_text(name)
                ));
            }
        }

        // Fallback to simple type-based query
        Ok(format!("({}) @target", target.node.kind()))
    }
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
            if let Some(func) = node.child_by_field_name("function") {
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn generate_query_for_extension_method() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let range = find_function_byte_range(&tree, &source, "invalidateCache");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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
            let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        // Should match at least 1 (may match trait decl too if query isn't precise enough)
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn rust_generic_function() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let range = find_rust_function_byte_range(&tree, &source, "validate_and_check");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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
            let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateAndCheck"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn ts_class_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let range = find_ts_function_byte_range(&tree, &source, "validateToken");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn ts_private_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let range = find_ts_function_byte_range(&tree, &source, "decode");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("create_default_auth_service"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn py_class_method() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "validate_token");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn py_private_method() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "_decode");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("_decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn py_decorated_function() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let range = find_py_function_byte_range(&tree, &source, "require_auth");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("CreateDefaultAuthService"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn go_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let range = find_go_function_range(&tree, &source, "ValidateToken");
        let lang = CodemarkLang::Go.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn go_private_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let range = find_go_function_range(&tree, &source, "decode");
        let lang = CodemarkLang::Go.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

    fn find_java_method_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
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
        let range = find_java_method_range(&tree, &source, "validateToken");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_private_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_method_range(&tree, &source, "decode");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_static_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_method_range(&tree, &source, "createDefault");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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

    fn find_csharp_method_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
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
        let range = find_csharp_method_range(&tree, &source, "ValidateToken");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("ValidateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_private_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_method_range(&tree, &source, "Decode");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("Decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_static_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_method_range(&tree, &source, "CreateDefault");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
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
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[test]
    fn dart_enum() {
        let (tree, source) = parse_dart_fixture("auth_service.dart");
        let offset = source.find("enum AuthError").unwrap();
        let range = (offset, offset + 10);
        let lang = CodemarkLang::Dart.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("AuthError"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Range precision tests: method range should target method, not class ---

    #[test]
    fn swift_exact_method_range_targets_method_not_class() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        // Get the exact byte range of validateToken
        let method_range = find_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "function_declaration",
            "should target function_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn rust_exact_method_range_targets_method_not_impl() {
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        let method_range = find_rust_function_byte_range(&tree, &source, "decode");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "should target function_item, not impl_item"
        );
        assert_eq!(result.target_name.as_deref(), Some("decode"));
    }

    #[test]
    fn ts_exact_method_range_targets_method_not_class() {
        let (tree, source) = parse_ts_fixture("auth_service.ts");
        let lang = CodemarkLang::TypeScript.tree_sitter_language();

        let method_range = find_ts_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "method_definition",
            "should target method_definition, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn py_exact_method_range_targets_method_not_class() {
        let (tree, source) = parse_py_fixture("auth_service.py");
        let lang = CodemarkLang::Python.tree_sitter_language();

        let method_range = find_py_function_byte_range(&tree, &source, "validate_token");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "function_definition",
            "should target function_definition, not class_definition"
        );
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));
    }

    #[test]
    fn go_exact_method_range_targets_method() {
        let (tree, source) = parse_go_fixture("auth_service.go");
        let lang = CodemarkLang::Go.tree_sitter_language();

        let method_range = find_go_function_range(&tree, &source, "ValidateToken");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration"
        );
    }

    #[test]
    fn java_exact_method_range_targets_method_not_class() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let lang = CodemarkLang::Java.tree_sitter_language();

        let method_range = find_java_method_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[test]
    fn single_line_inside_method_targets_method_not_class() {
        // A single line inside a method should still target the enclosing method
        let (tree, source) = parse_rust_fixture("auth_service.rs");
        let lang = CodemarkLang::Rust.tree_sitter_language();

        // Line 50 is inside the decode function body
        let line_50_start = source.lines().take(49).map(|l| l.len() + 1).sum::<usize>();
        let line_50_end = line_50_start + source.lines().nth(49).unwrap_or("").len();

        let result =
            generate_query(&tree, source.as_bytes(), (line_50_start, line_50_end), &lang, StrategyType::Declaration).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "single line inside method should target function_item, not impl_item or struct_item"
        );
    }
}
