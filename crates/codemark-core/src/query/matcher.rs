use tree_sitter::{Language, Node, Query, QueryCursor, StreamingIterator, Tree};

use crate::error::{Error, Result};

/// Result of running a tree-sitter query against a source file.
#[derive(Debug)]
pub struct MatchResult {
    pub node_text: String,
    pub byte_range: (usize, usize),
    pub start_point: (usize, usize),
    pub end_point: (usize, usize),
    /// All captures from the match, including @sticky.* captures.
    /// Each entry is (capture_name, (byte_range, line_number)).
    pub captures: Vec<(String, (usize, usize), usize)>,
}

/// Compile and run a tree-sitter query against a tree, returning all @target matches.
pub fn run_query(
    query_str: &str,
    tree: &Tree,
    source: &[u8],
    language: &Language,
) -> Result<Vec<MatchResult>> {
    run_query_in(query_str, tree.root_node(), source, language)
}

/// Compile and run a tree-sitter query confined to the subtree rooted at
/// `scope` (the node itself included), returning all `@target` matches whose
/// target lies inside it.
///
/// The scoping primitive for anchored comments: a comment query is relative to
/// its bookmark's deepest landmark, so it only has to be unique inside that
/// node. Passing `tree.root_node()` is exactly [`run_query`].
pub fn run_query_in(
    query_str: &str,
    scope: Node,
    source: &[u8],
    language: &Language,
) -> Result<Vec<MatchResult>> {
    let query = Query::new(language, query_str)
        .map_err(|e| Error::TreeSitter(format!("invalid query: {e}")))?;

    let _target_idx = query
        .capture_index_for_name("target")
        .ok_or_else(|| Error::TreeSitter("query has no @target capture".into()))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, scope, source);
    let mut results = Vec::new();

    while let Some(m) = matches.next() {
        let mut target_node = None;
        let mut sticky_captures = Vec::new();

        for capture in m.captures {
            let capture_name = query.capture_names()[capture.index as usize].to_string();
            if capture_name == "target" {
                target_node = Some(capture.node);
            } else if capture_name.starts_with("sticky.") {
                let node = capture.node;
                let byte_range = (node.start_byte(), node.end_byte());
                let line = node.start_position().row;
                sticky_captures.push((capture_name, byte_range, line));
            }
        }

        if let Some(node) = target_node {
            // A pattern rooted inside `scope` can still capture outside it
            // only through anchors; keep the contract strict regardless.
            if node.start_byte() < scope.start_byte() || node.end_byte() > scope.end_byte() {
                continue;
            }
            let text = std::str::from_utf8(&source[node.byte_range()]).unwrap_or("").to_string();
            results.push(MatchResult {
                node_text: text,
                byte_range: (node.start_byte(), node.end_byte()),
                start_point: (node.start_position().row, node.start_position().column),
                end_point: (node.end_position().row, node.end_position().column),
                captures: sticky_captures,
            });
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::{Language as CodemarkLang, Parser};

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn run_simple_query() {
        let mut parser = Parser::new(crate::testing::grammar("swift")).unwrap();
        let source = b"func hello() { }\nfunc world() { }";
        let tree = parser.parse(source).unwrap();

        let query_str = "(function_declaration name: (simple_identifier) @name) @target";
        let results = run_query(
            query_str,
            &tree,
            source,
            &crate::testing::grammar("swift").tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].node_text.contains("hello"));
        assert!(results[1].node_text.contains("world"));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn run_query_with_predicate() {
        let mut parser = Parser::new(crate::testing::grammar("swift")).unwrap();
        let source = b"func hello() { }\nfunc world() { }";
        let tree = parser.parse(source).unwrap();

        let query_str = r#"(function_declaration
            name: (simple_identifier) @name
            (#eq? @name "hello")) @target"#;
        let results = run_query(
            query_str,
            &tree,
            source,
            &crate::testing::grammar("swift").tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].node_text.contains("hello"));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn run_query_in_confines_matches_to_scope() {
        let source = b"fn foo() { let x = 1; }\nfn bar() { let x = 2; }";
        let rust = crate::testing::grammar("rust");
        let mut parser = Parser::new(rust.clone()).unwrap();
        let tree = parser.parse(source).unwrap();
        let lang = rust.tree_sitter_language();
        let query = "(let_declaration) @target";

        assert_eq!(run_query(query, &tree, source, &lang).unwrap().len(), 2);

        let foo = tree.root_node().named_child(0).unwrap();
        assert_eq!(foo.kind(), "function_item");
        let scoped = run_query_in(query, foo, source, &lang).unwrap();
        assert_eq!(scoped.len(), 1);
        assert!(scoped[0].node_text.contains("= 1"));

        // The scope node itself can be the target.
        let itself = run_query_in("(function_item) @target", foo, source, &lang).unwrap();
        assert_eq!(itself.len(), 1);
        assert!(itself[0].node_text.starts_with("fn foo"));
    }

    fn dump_ast_with_limit(
        node: tree_sitter::Node,
        source: &str,
        depth: usize,
        limit: usize,
        field_name: Option<&str>,
    ) {
        if !node.is_named() {
            return;
        }
        let indent = "  ".repeat(depth);
        let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
        let text_preview: String =
            source[node.byte_range()].chars().take(80).collect::<String>().replace('\n', "\\n");
        println!(
            "{indent}{field_str}{} [{}-{}] {text_preview:?}",
            node.kind(),
            node.start_byte(),
            node.end_byte(),
        );
        if depth < limit {
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.is_named() {
                        dump_ast_with_limit(child, source, depth + 1, limit, cursor.field_name());
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
        }
    }

    /// Dump AST for Swift fixture to discover actual node type names.
    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn dump_swift_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/swift/complex_scenarios.swift");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(crate::testing::grammar("swift")).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, source) = parser.parse_file(&fixture, &provider).await.unwrap();

        dump_ast_with_limit(tree.root_node(), &source, 0, 10, None);
    }

    /// Dump AST for Rust fixture to discover node type names.
    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn dump_rust_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/rust/auth_service.rs");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(crate::testing::grammar("rust")).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, source) = parser.parse_file(&fixture, &provider).await.unwrap();

        dump_ast_with_limit(tree.root_node(), &source, 0, 10, None);
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn dump_typescript_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/typescript/auth_service.ts");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(crate::testing::grammar("typescript")).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, source) = parser.parse_file(&fixture, &provider).await.unwrap();
        dump_ast_with_limit(tree.root_node(), &source, 0, 10, None);
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn dump_python_ast_structure() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/python/auth_service.py");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(crate::testing::grammar("python")).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, source) = parser.parse_file(&fixture, &provider).await.unwrap();
        dump_ast_with_limit(tree.root_node(), &source, 0, 10, None);
    }

    // Macro to generate AST dump tests for any language/fixture pair
    macro_rules! dump_ast_test {
        ($(#[$attr:meta])* $name:ident, $lang:expr, $fixture:expr) => {
            #[tokio::test]
            $(#[$attr])*
            async fn $name() {
                let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join($fixture);
                if !fixture.exists() {
                    return;
                }
                let mut parser = Parser::new($lang).unwrap();
                let provider = crate::vfs::LocalFileProvider;
                let (tree, source) = parser.parse_file(&fixture, &provider).await.unwrap();
                fn dump(
                    node: tree_sitter::Node,
                    source: &str,
                    depth: usize,
                    field_name: Option<&str>,
                ) {
                    if !node.is_named() {
                        return;
                    }
                    let indent = "  ".repeat(depth);
                    let field_str = field_name.map(|f| format!("{f}: ")).unwrap_or_default();
                    let text: String = source[node.byte_range()]
                        .chars()
                        .take(80)
                        .collect::<String>()
                        .replace('\n', "\\n");
                    println!(
                        "{indent}{field_str}{} [{}-{}] {text:?}",
                        node.kind(),
                        node.start_byte(),
                        node.end_byte()
                    );
                    if depth < 10 {
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
        };
    }

    dump_ast_test!(
        #[cfg_attr(
            not(feature = "wasm"),
            ignore = "needs the wasm feature to load registry grammars"
        )]
        dump_go_ast,
        crate::testing::grammar("go"),
        "../../tests/fixtures/go/auth_service.go"
    );
    dump_ast_test!(
        #[cfg_attr(
            not(feature = "wasm"),
            ignore = "needs the wasm feature to load registry grammars"
        )]
        dump_java_ast,
        crate::testing::grammar("java"),
        "../../tests/fixtures/java/AuthService.java"
    );
    dump_ast_test!(
        #[cfg_attr(
            not(feature = "wasm"),
            ignore = "needs the wasm feature to load registry grammars"
        )]
        dump_csharp_ast,
        crate::testing::grammar("c_sharp"),
        "../../tests/fixtures/csharp/AuthService.cs"
    );
    dump_ast_test!(
        dump_dart_ast,
        CodemarkLang::Dart,
        "../../tests/fixtures/dart/auth_service.dart"
    );

    // --- Sticky capture tests ---

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn matcher_extracts_sticky_captures() {
        let source = r#"
    class Foo {
        func bar() {}
    }
    "#;
        let query = r#"
    (class_declaration
      name: (type_identifier) @name0 @sticky.class
      (class_body
        (function_declaration
          name: (simple_identifier) @fn_name @sticky.method) @target))
    "#;

        let mut parser = Parser::new(crate::testing::grammar("swift")).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();

        let results = run_query(
            query,
            &tree,
            source.as_bytes(),
            &crate::testing::grammar("swift").tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].captures.len(), 2);

        // Check that we have both sticky captures
        let capture_names: Vec<&str> =
            results[0].captures.iter().map(|(name, _, _)| name.as_str()).collect();
        assert!(capture_names.contains(&"sticky.class"));
        assert!(capture_names.contains(&"sticky.method"));
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn matcher_no_sticky_captures_in_legacy_query() {
        // Legacy query without @sticky captures should return empty captures vec
        let source = r#"
    class Foo {
        func bar() {}
    }
    "#;
        let query = r#"
    (class_declaration
      name: (type_identifier) @name0
      (class_body
        (function_declaration
          name: (simple_identifier) @fn_name) @target))
    "#;

        let mut parser = Parser::new(crate::testing::grammar("swift")).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();

        let results = run_query(
            query,
            &tree,
            source.as_bytes(),
            &crate::testing::grammar("swift").tree_sitter_language(),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].captures.len(), 0);
    }
}
