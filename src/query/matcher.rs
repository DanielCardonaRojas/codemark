use tree_sitter::{Language, Query, QueryCursor, StreamingIterator, Tree};

use crate::error::{Error, Result};

/// Result of running a tree-sitter query against a source file.
#[derive(Debug)]
pub struct MatchResult {
    pub node_text: String,
    pub byte_range: (usize, usize),
    pub start_point: (usize, usize),
    pub end_point: (usize, usize),
}

/// Compile and run a tree-sitter query against a tree, returning all @target matches.
pub fn run_query(
    query_str: &str,
    tree: &Tree,
    source: &[u8],
    language: &Language,
) -> Result<Vec<MatchResult>> {
    let query = Query::new(language, query_str)
        .map_err(|e| Error::TreeSitter(format!("invalid query: {e}")))?;

    let target_idx = query
        .capture_index_for_name("target")
        .ok_or_else(|| Error::TreeSitter("query has no @target capture".into()))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source);
    let mut results = Vec::new();

    while let Some(m) = matches.next() {
        for capture in m.captures {
            if capture.index == target_idx {
                let node = capture.node;
                let text = std::str::from_utf8(&source[node.byte_range()])
                    .unwrap_or("")
                    .to_string();
                results.push(MatchResult {
                    node_text: text,
                    byte_range: (node.start_byte(), node.end_byte()),
                    start_point: (node.start_position().row, node.start_position().column),
                    end_point: (node.end_position().row, node.end_position().column),
                });
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::{Language as CodemarkLang, Parser};

    #[test]
    fn run_simple_query() {
        let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
        let source = b"func hello() { }\nfunc world() { }";
        let tree = parser.parse(source).unwrap();

        let query_str = "(function_declaration name: (simple_identifier) @name) @target";
        let results = run_query(
            query_str,
            &tree,
            source,
            &CodemarkLang::Swift.tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].node_text.contains("hello"));
        assert!(results[1].node_text.contains("world"));
    }

    #[test]
    fn run_query_with_predicate() {
        let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
        let source = b"func hello() { }\nfunc world() { }";
        let tree = parser.parse(source).unwrap();

        let query_str = r#"(function_declaration
            name: (simple_identifier) @name
            (#eq? @name "hello")) @target"#;
        let results = run_query(
            query_str,
            &tree,
            source,
            &CodemarkLang::Swift.tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].node_text.contains("hello"));
    }

    /// Dump AST for Swift fixture to discover actual node type names.
    #[test]
    fn dump_swift_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
        let (tree, source) = parser.parse_file(&fixture).unwrap();

        fn dump(node: tree_sitter::Node, source: &str, depth: usize, field_name: Option<&str>) {
            if !node.is_named() {
                return;
            }
            let indent = "  ".repeat(depth);
            let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
            let text_preview: String = source[node.byte_range()]
                .chars()
                .take(80)
                .collect::<String>()
                .replace('\n', "\\n");
            println!(
                "{indent}{field_str}{} [{}-{}] {text_preview:?}",
                node.kind(),
                node.start_byte(),
                node.end_byte(),
            );
            if depth < 3 {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.is_named() {
                            dump(child, source, depth + 1, cursor.field_name());
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
        }

        dump(tree.root_node(), &source, 0, None);
    }

    /// Dump AST for Rust fixture to discover node type names.
    #[test]
    fn dump_rust_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/rust/auth_service.rs");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(CodemarkLang::Rust).unwrap();
        let (tree, source) = parser.parse_file(&fixture).unwrap();

        fn dump(node: tree_sitter::Node, source: &str, depth: usize, field_name: Option<&str>) {
            if !node.is_named() {
                return;
            }
            let indent = "  ".repeat(depth);
            let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
            let text_preview: String = source[node.byte_range()]
                .chars()
                .take(80)
                .collect::<String>()
                .replace('\n', "\\n");
            println!(
                "{indent}{field_str}{} [{}-{}] {text_preview:?}",
                node.kind(),
                node.start_byte(),
                node.end_byte(),
            );
            if depth < 3 {
                let mut cursor = node.walk();
                if cursor.goto_first_child() {
                    loop {
                        let child = cursor.node();
                        if child.is_named() {
                            dump(child, source, depth + 1, cursor.field_name());
                        }
                        if !cursor.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
        }

        dump(tree.root_node(), &source, 0, None);
    }

    #[test]
    fn dump_typescript_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/typescript/auth_service.ts");
        if !fixture.exists() { return; }
        let mut parser = Parser::new(CodemarkLang::TypeScript).unwrap();
        let (tree, source) = parser.parse_file(&fixture).unwrap();
        fn dump(node: tree_sitter::Node, source: &str, depth: usize, field_name: Option<&str>) {
            if !node.is_named() { return; }
            let indent = "  ".repeat(depth);
            let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
            let text: String = source[node.byte_range()].chars().take(80).collect::<String>().replace('\n', "\\n");
            println!("{indent}{field_str}{} [{}-{}] {text:?}", node.kind(), node.start_byte(), node.end_byte());
            if depth < 3 {
                let mut cursor = node.walk();
                if cursor.goto_first_child() { loop {
                    let child = cursor.node();
                    if child.is_named() { dump(child, source, depth + 1, cursor.field_name()); }
                    if !cursor.goto_next_sibling() { break; }
                }}
            }
        }
        dump(tree.root_node(), &source, 0, None);
    }

    #[test]
    fn dump_python_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/python/auth_service.py");
        if !fixture.exists() { return; }
        let mut parser = Parser::new(CodemarkLang::Python).unwrap();
        let (tree, source) = parser.parse_file(&fixture).unwrap();
        fn dump(node: tree_sitter::Node, source: &str, depth: usize, field_name: Option<&str>) {
            if !node.is_named() { return; }
            let indent = "  ".repeat(depth);
            let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
            let text: String = source[node.byte_range()].chars().take(80).collect::<String>().replace('\n', "\\n");
            println!("{indent}{field_str}{} [{}-{}] {text:?}", node.kind(), node.start_byte(), node.end_byte());
            if depth < 3 {
                let mut cursor = node.walk();
                if cursor.goto_first_child() { loop {
                    let child = cursor.node();
                    if child.is_named() { dump(child, source, depth + 1, cursor.field_name()); }
                    if !cursor.goto_next_sibling() { break; }
                }}
            }
        }
        dump(tree.root_node(), &source, 0, None);
    }

    // Macro to generate AST dump tests for any language/fixture pair
    macro_rules! dump_ast_test {
        ($name:ident, $lang:expr, $fixture:expr) => {
            #[test]
            fn $name() {
                let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join($fixture);
                if !fixture.exists() { return; }
                let mut parser = Parser::new($lang).unwrap();
                let (tree, source) = parser.parse_file(&fixture).unwrap();
                fn dump(node: tree_sitter::Node, source: &str, depth: usize, field_name: Option<&str>) {
                    if !node.is_named() { return; }
                    let indent = "  ".repeat(depth);
                    let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
                    let text: String = source[node.byte_range()].chars().take(80).collect::<String>().replace('\n', "\\n");
                    println!("{indent}{field_str}{} [{}-{}] {text:?}", node.kind(), node.start_byte(), node.end_byte());
                    if depth < 3 {
                        let mut cursor = node.walk();
                        if cursor.goto_first_child() { loop {
                            let child = cursor.node();
                            if child.is_named() { dump(child, source, depth + 1, cursor.field_name()); }
                            if !cursor.goto_next_sibling() { break; }
                        }}
                    }
                }
                dump(tree.root_node(), &source, 0, None);
            }
        };
    }

    dump_ast_test!(dump_go_ast, CodemarkLang::Go, "tests/fixtures/go/auth_service.go");
    dump_ast_test!(dump_java_ast, CodemarkLang::Java, "tests/fixtures/java/AuthService.java");
    dump_ast_test!(dump_csharp_ast, CodemarkLang::CSharp, "tests/fixtures/csharp/AuthService.cs");
    dump_ast_test!(dump_dart_ast, CodemarkLang::Dart, "tests/fixtures/dart/auth_service.dart");
}
