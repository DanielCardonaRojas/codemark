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
    let mut breadcrumbs = Vec::new();
    let whitelist = get_whitelist(language);

    let mut current = target_node.parent();
    while let Some(node) = current {
        if whitelist.contains(&node.kind()) {
            // Extract the full source line to preserve leading indentation
            let start_pos = node.start_position();
            let line_text = source.lines().nth(start_pos.row).unwrap_or("").trim_end();

            breadcrumbs.push(Breadcrumb { line: start_pos.row + 1, text: line_text.to_string() });

            if breadcrumbs.len() >= max_count {
                break;
            }
        }
        current = node.parent();
    }

    // Breadcrumbs were collected from child to parent, so reverse them to get parent to child order
    breadcrumbs.reverse();
    breadcrumbs
}

/// Get the language-specific whitelist of node types that make useful breadcrumbs.
fn get_whitelist(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["mod_item", "impl_item", "trait_item", "function_item"],
        Language::Swift => &[
            "class_declaration",
            "extension_declaration",
            "protocol_declaration",
            "function_declaration",
            "struct_declaration",
            "enum_declaration",
        ],
        Language::TypeScript => &[
            "namespace_declaration",
            "class_declaration",
            "interface_declaration",
            "method_definition",
            "function_declaration",
            "type_alias_declaration",
            "enum_declaration",
        ],
        Language::Python => {
            &["class_definition", "function_definition", "async_function_definition"]
        }
        Language::Go => &["type_declaration", "function_declaration", "method_declaration"],
        Language::Java => &[
            "class_declaration",
            "interface_declaration",
            "enum_declaration",
            "method_declaration",
        ],
        Language::CSharp => &[
            "class_declaration",
            "interface_declaration",
            "struct_declaration",
            "enum_declaration",
            "method_declaration",
            "namespace_declaration",
        ],
        Language::Dart => &[
            "class_definition",
            "mixin_definition",
            "extension_definition",
            "enum_definition",
            "function_signature",
            "method_declaration",
        ],
    }
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
