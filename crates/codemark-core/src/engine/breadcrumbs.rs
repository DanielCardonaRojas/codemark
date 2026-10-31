use crate::parser::languages::Language;
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

/// A single breadcrumb entry representing a structural ancestor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Breadcrumb {
    /// The 1-based line number where this ancestor starts.
    pub line: usize,
    /// The text of the first line of this ancestor (e.g. the signature).
    pub text: String,
}

/// Extract breadcrumbs for a node by walking up the AST.
pub fn extract_breadcrumbs(
    target_node: Node,
    source: &str,
    language: Language,
    max_count: usize,
) -> Vec<Breadcrumb> {
    if max_count == 0 {
        return Vec::new();
    }
    let profile = language.profile();
    let whitelist = profile.containers.resolve_kinds(&profile.node_labels);
    let mut breadcrumbs = Vec::new();

    let mut current = target_node.parent();
    while let Some(node) = current {
        if whitelist.iter().any(|k| k == node.kind()) {
            // Extract the full source line to preserve leading indentation
            let start_pos = node.start_position();
            let line_text = source.lines().nth(start_pos.row).unwrap_or("").trim_end();

            // Two whitelisted ancestors can start on the same line when a
            // container wrapper encloses its own spec — Go's `type_declaration`
            // around `type_spec` in `type S struct {`, for instance. The line
            // text is identical, so emitting both would repeat the header and
            // spend two of the `max_count` slots on one level of nesting.
            // Walking child to parent, such a wrapper is the entry just pushed.
            let duplicates_previous =
                breadcrumbs.last().is_some_and(|b: &Breadcrumb| b.line == start_pos.row + 1);

            if !duplicates_previous {
                breadcrumbs
                    .push(Breadcrumb { line: start_pos.row + 1, text: line_text.to_string() });

                if breadcrumbs.len() >= max_count {
                    break;
                }
            }
        }
        current = node.parent();
    }

    // Breadcrumbs were collected from child to parent, so reverse them to get parent to child order
    breadcrumbs.reverse();
    breadcrumbs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::Parser;

    #[test]
    fn test_extract_rust_breadcrumbs() {
        let source = r#"
mod auth {
    impl User {
        fn validate(&self) {
            // target here
        }
    }
}
"#;
        let mut parser = Parser::new(Language::Rust).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        // Target is the block inside validate
        let target = tree.root_node().descendant_for_byte_range(60, 61).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Rust, 3);
        assert_eq!(bc.len(), 3);
        assert_eq!(bc[0].text, "mod auth {");
        assert_eq!(bc[1].text, "    impl User {");
        assert_eq!(bc[2].text, "        fn validate(&self) {");
    }

    /// Go's `type` container label maps to both `type_declaration` and its
    /// child `type_spec`, which start on the same line in `type S struct {`.
    /// Both are whitelisted ancestors, so the walker used to emit the header
    /// twice and burn two breadcrumb slots on one level of nesting.
    #[test]
    fn test_go_type_header_is_not_repeated() {
        let source = "package main\n\ntype S struct {\n\tName string\n}\n";
        let mut parser = Parser::new(Language::Go).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let idx = source.find("Name").unwrap();
        let target = tree.root_node().descendant_for_byte_range(idx, idx + 1).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Go, 3);
        assert_eq!(bc, vec![Breadcrumb { line: 3, text: "type S struct {".to_string() }]);
    }

    /// A method inside a Go type must still report both levels: collapsing is
    /// keyed on the line, so it must not swallow a genuinely distinct ancestor.
    #[test]
    fn test_go_func_inside_type_keeps_both_levels() {
        let source =
            "package main\n\ntype S struct{}\n\nfunc (s S) Name() string {\n\treturn \"\"\n}\n";
        let mut parser = Parser::new(Language::Go).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let idx = source.find("return").unwrap();
        let target = tree.root_node().descendant_for_byte_range(idx, idx + 1).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Go, 3);
        assert_eq!(bc.len(), 1, "unexpected trail: {bc:#?}");
        assert_eq!(bc[0].text, "func (s S) Name() string {");
    }

    /// `async def` needs no label of its own: tree-sitter-python parses it as a
    /// `function_definition` with an `async` child, so the existing label already
    /// covers it. The profile used to carry an `async_function_definition` entry
    /// that no node kind ever matched.
    #[test]
    fn test_python_async_def_is_a_container() {
        let source = "class C:\n    async def fetch(self):\n        return 1\n";
        let mut parser = Parser::new(Language::Python).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let idx = source.find("return").unwrap();
        let target = tree.root_node().descendant_for_byte_range(idx, idx + 1).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Python, 3);
        assert_eq!(bc.len(), 2, "async def dropped out of the trail: {bc:#?}");
        assert_eq!(bc[0].text, "class C:");
        assert_eq!(bc[1].text, "    async def fetch(self):");
    }

    /// Swift maps both `enum_declaration` and `enum_entry` to the `enum` label,
    /// but they sit on different lines, so both are real breadcrumbs. Guards
    /// against the duplicate fix being written as a node-kind exclusion.
    #[test]
    fn test_swift_enum_entry_on_its_own_line_is_kept() {
        let source = "enum Color {\n    case red, green\n}\n";
        let mut parser = Parser::new(Language::Swift).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let idx = source.find("red").unwrap();
        let target = tree.root_node().descendant_for_byte_range(idx, idx + 1).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Swift, 3);
        assert_eq!(bc.len(), 2, "same-line collapsing swallowed a distinct ancestor: {bc:#?}");
        assert_eq!(bc[0].text, "enum Color {");
        assert_eq!(bc[1].text, "    case red, green");
    }

    #[test]
    fn test_extract_swift_breadcrumbs() {
        let source = r#"
class AuthService {
    func login() {
        // target
    }
}
"#;
        let mut parser = Parser::new(Language::Swift).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let target = tree.root_node().descendant_for_byte_range(45, 46).unwrap();

        let bc = extract_breadcrumbs(target, source, Language::Swift, 3);
        assert_eq!(bc.len(), 2);
        assert_eq!(bc[0].text, "class AuthService {");
        assert_eq!(bc[1].text, "    func login() {");
    }
}
