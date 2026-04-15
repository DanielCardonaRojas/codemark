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
    field: String,
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
    let target = find_target_node(tree, byte_range)?;
    generate_query_for_node(target, source, language)
}

/// Given a specific AST node, generate a tree-sitter query for it.
#[allow(clippy::collapsible_if)]
pub fn generate_query_for_node(
    node: Node,
    source: &[u8],
    language: &Language,
) -> Result<GeneratedQuery> {
    let target = walk_to_named_declaration(node);
    let path = build_structural_path(target, source);
    let target_name = path.last().and_then(|e| e.name_info.as_ref().map(|info| info.text.clone()));

    let query = build_tier1_query(&path);

    // Validate uniqueness
    let source_text = std::str::from_utf8(source).unwrap_or("");
    let reparsed_tree = {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language).ok();
        parser.parse(source, None)
    };

    if let Some(ref tree) = reparsed_tree {
        if let Ok(matches) = matcher::run_query(&query, tree, source, language) {
            if matches.len() == 1 {
                return Ok(GeneratedQuery {
                    query,
                    target_node_type: target.kind().to_string(),
                    target_name,
                    byte_range: (target.start_byte(), target.end_byte()),
                });
            }
            // Multiple matches — try disambiguation with parameters
            if matches.len() > 1 {
                if let Some(disambiguated) =
                    try_disambiguate(&path, target, source, source_text, tree, language)
                {
                    return Ok(GeneratedQuery {
                        query: disambiguated,
                        target_node_type: target.kind().to_string(),
                        target_name,
                        byte_range: (target.start_byte(), target.end_byte()),
                    });
                }
            }
        }
    }

    // Return the query even if not unique — resolution will handle multiple matches
    Ok(GeneratedQuery {
        query,
        target_node_type: target.kind().to_string(),
        target_name,
        byte_range: (target.start_byte(), target.end_byte()),
    })
}

/// Find the smallest named node that spans the given byte range.
///
/// Strategy: find the deepest node at the range, then walk up to the nearest
/// declaration. But prefer a **smaller** declaration that fits within the
/// user's range over a larger one that contains it. This ensures that selecting
/// lines 42-67 (exactly a method) targets the method, not the enclosing class.
fn find_target_node(tree: &Tree, byte_range: (usize, usize)) -> Result<Node<'_>> {
    let root = tree.root_node();
    let node = root
        .descendant_for_byte_range(byte_range.0, byte_range.1)
        .ok_or_else(|| Error::TreeSitter("no node found at byte range".into()))?;

    // First, try to find a declaration that is contained within the user's range.
    // This handles the case where the user selected an entire method body.
    if let Some(inner) = find_declaration_within(root, byte_range) {
        return Ok(inner);
    }

    // Otherwise, walk up from the deepest node to the nearest declaration.
    Ok(walk_to_named_declaration(node))
}

/// Find the largest declaration node whose span is contained within the given byte range.
/// This handles the case where the user selects exactly a method — we want the method,
/// not a smaller declaration inside it, and not the enclosing class.
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
            // Prefer the largest declaration that fits — this is the most meaningful
            // structural unit the user intended to select
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
    // C#
    "namespace_declaration",
    "record_declaration",
    // Dart
    "function_signature",
    "class_member",
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
    // Try the "name" field first
    if let Some(name_node) = node.child_by_field_name("name") {
        // For Swift user_type nodes (extensions), we need nested matching
        if name_node.kind() == "user_type" {
            let mut cursor = name_node.walk();
            for child in name_node.named_children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return Some(NameInfo {
                        field: "name".to_string(),
                        direct_type: "user_type".to_string(),
                        inner_type: Some("type_identifier".to_string()),
                        text: node_text(child, source),
                    });
                }
            }
        }
        return Some(NameInfo {
            field: "name".to_string(),
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
            field: "type".to_string(),
            direct_type: type_node.kind().to_string(),
            inner_type: None,
            text: node_text(type_node, source),
        });
    }

    // For TS export_statement: get the name from the inner declaration
    if node.kind() == "export_statement"
        && let Some(decl) = node.child_by_field_name("declaration")
    {
        return extract_name_info(decl, source);
    }

    // For Python decorated_definition: get the name from the inner definition
    if node.kind() == "decorated_definition"
        && let Some(def) = node.child_by_field_name("definition")
    {
        return extract_name_info(def, source);
    }

    // For TS method_definition: name is in "name" field as property_identifier
    // (already handled by the generic "name" field check above)

    // For Swift enum_entry, try to find the name pattern
    if node.kind() == "enum_entry" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "simple_identifier" {
                return Some(NameInfo {
                    field: "name".to_string(),
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
        let mut predicate = String::new();
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
                s.push_str(&format!(
                    "\n{pad}  {}: ({} ({inner_type}) @{capture_name})",
                    info.field, info.direct_type
                ));
            } else {
                s.push_str(&format!(
                    "\n{pad}  {}: ({}) @{capture_name}",
                    info.field, info.direct_type
                ));
            }
            predicate =
                format!("\n{pad}  (#eq? @{capture_name} \"{}\")", escape_query_text(&info.text));
        }

        if is_target {
            // Add predicate before closing, then close with @target
            s.push_str(&predicate);
            s.push_str(") @target");
        } else {
            // Add predicate, then nest the child
            s.push_str(&predicate);
            let child_str = build_node(path, idx + 1, depth, indent + 1, counter);
            s.push('\n');
            s.push_str(&child_str);
            s.push(')');
        }

        s
    }

    build_node(path, 0, depth, 0, &mut capture_counter)
}

fn escape_query_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Try to disambiguate by adding parameter type info.
#[allow(clippy::collapsible_if)]
fn try_disambiguate(
    path: &[PathEntry],
    target: Node,
    source: &[u8],
    _source_text: &str,
    tree: &Tree,
    language: &Language,
) -> Option<String> {
    if target.kind() != "function_declaration" {
        return None;
    }

    // Find the first parameter's type annotation
    let param_type = extract_first_param_type(target, source)?;

    // Build query with parameter disambiguation
    let base_query = build_tier1_query(path);
    // Insert parameter constraint before @target
    let disambiguated = base_query.replace(
        ") @target",
        &format!(
            "\n  (parameter\n    (simple_identifier)\n    (type_annotation (user_type (type_identifier) @param_type)))\n) @target\n(#eq? @param_type \"{param_type}\")"
        ),
    );

    // Verify this now gives exactly one match
    if let Ok(matches) = matcher::run_query(&disambiguated, tree, source, language) {
        if matches.len() == 1 {
            return Some(disambiguated);
        }
    }

    None
}

fn extract_first_param_type(func_node: Node, source: &[u8]) -> Option<String> {
    let mut cursor = func_node.walk();
    for child in func_node.named_children(&mut cursor) {
        if child.kind() == "parameter" {
            // Look for type_annotation
            let mut param_cursor = child.walk();
            for param_child in child.named_children(&mut param_cursor) {
                if param_child.kind() == "type_annotation" {
                    let text = node_text(param_child, source);
                    // Strip the leading ": "
                    let clean = text.trim().trim_start_matches(':').trim();
                    return Some(clean.to_string());
                }
            }
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
            if node.kind() == "function_declaration" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let text = &source[name_node.byte_range()];
                    if text == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
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
            if node.kind() == "function_item" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
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
            if kind == "function_declaration" || kind == "method_definition" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
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
            if kind == "function_definition" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
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
            if kind == "function_declaration" || kind == "method_declaration" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
                }
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

    fn find_java_method_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "method_declaration" || node.kind() == "constructor_declaration" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
                }
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
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_private_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_method_range(&tree, &source, "decode");
        let lang = CodemarkLang::Java.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn java_static_method() {
        let (tree, source) = parse_java_fixture("AuthService.java");
        let range = find_java_method_range(&tree, &source, "createDefault");
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

    fn find_csharp_method_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "method_declaration" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
                }
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
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("ValidateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_private_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_method_range(&tree, &source, "Decode");
        let lang = CodemarkLang::CSharp.tree_sitter_language();
        let result = generate_query(&tree, source.as_bytes(), range, &lang).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("Decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[test]
    fn csharp_static_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs");
        let range = find_csharp_method_range(&tree, &source, "CreateDefault");
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
    fn swift_exact_method_range_targets_method_not_class() {
        let (tree, source) = parse_fixture("auth_service.swift");
        let lang = CodemarkLang::Swift.tree_sitter_language();

        // Get the exact byte range of validateToken
        let method_range = find_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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

        let result = generate_query(&tree, source.as_bytes(), method_range, &lang).unwrap();
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
            generate_query(&tree, source.as_bytes(), (line_50_start, line_50_end), &lang).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "single line inside method should target function_item, not impl_item or struct_item"
        );
    }
}
