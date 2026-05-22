/// Maps tree-sitter node types to human-readable labels for query summaries.
///
/// This module provides a classification function that converts raw tree-sitter
/// node type names (like "function_item" or "class_declaration") into concise,
/// human-readable labels (like "function" or "class") for use in UI headlines.
/// Maps a tree-sitter node type to a human-readable label.
///
/// Returns `None` for node types that don't have a canonical label or are
/// considered anonymous/unnamed constructs.
///
/// # Examples
///
/// ```
/// use codemark_core::query::classifier::classify_node_type;
///
/// assert_eq!(classify_node_type("function_item"), Some("function"));
/// assert_eq!(classify_node_type("class_declaration"), Some("class"));
/// assert_eq!(classify_node_type("identifier"), None);
/// ```
pub fn classify_node_type(node_type: &str) -> Option<&'static str> {
    match node_type {
        // Functions / Methods
        "function_declaration" | "function_item" | "function_definition" => Some("function"),
        "method_definition" | "method_declaration" => Some("method"),
        "constructor_declaration" => Some("constructor"),

        // Classes / Structs / Interfaces
        "class_declaration" | "struct_declaration" | "struct_item" => Some("class"),
        "interface_declaration" | "protocol_declaration" | "trait_item" => Some("interface"),
        "impl_item" => Some("impl"),

        // Enums
        "enum_declaration" | "enum_item" | "enum_entry" | "enum_constant" => Some("enum"),

        // Types
        "type_declaration" | "type_alias_declaration" | "type_spec" | "type_item" => Some("type"),

        // Modules / Namespaces
        "mod_item" | "module" => Some("module"),
        "namespace_declaration" => Some("namespace"),

        // Variables / Properties
        "variable_declaration"
        | "lexical_declaration"
        | "property_declaration"
        | "const_item"
        | "static_item"
        | "var_declaration" => Some("variable"),

        // Other named declarations
        "macro_definition" => Some("macro"),
        "subscript_declaration" => Some("subscript"),
        "init_declaration" => Some("init"),
        "deinit_declaration" => Some("deinit"),

        // Statements that might be targeted
        "if_statement" | "if_expression" => Some("if statement"),
        "for_statement" => Some("for loop"),
        "while_statement" => Some("while loop"),
        "match_statement" | "match_expression" | "switch_statement" => Some("match"),
        "return_statement" => Some("return"),
        "assignment_expression" | "assignment_statement" => Some("assignment"),

        // Expressions
        "call_expression" => Some("call"),
        "binary_expression" => Some("expression"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_classifications() {
        assert_eq!(classify_node_type("function_item"), Some("function"));
        assert_eq!(classify_node_type("function_declaration"), Some("function"));
        assert_eq!(classify_node_type("function_definition"), Some("function"));
        assert_eq!(classify_node_type("method_definition"), Some("method"));
        assert_eq!(classify_node_type("method_declaration"), Some("method"));
        assert_eq!(classify_node_type("constructor_declaration"), Some("constructor"));
    }

    #[test]
    fn test_class_classifications() {
        assert_eq!(classify_node_type("class_declaration"), Some("class"));
        assert_eq!(classify_node_type("struct_declaration"), Some("class"));
        assert_eq!(classify_node_type("struct_item"), Some("class"));
        assert_eq!(classify_node_type("interface_declaration"), Some("interface"));
        assert_eq!(classify_node_type("protocol_declaration"), Some("interface"));
        assert_eq!(classify_node_type("trait_item"), Some("interface"));
        assert_eq!(classify_node_type("impl_item"), Some("impl"));
    }

    #[test]
    fn test_enum_classifications() {
        assert_eq!(classify_node_type("enum_declaration"), Some("enum"));
        assert_eq!(classify_node_type("enum_item"), Some("enum"));
        assert_eq!(classify_node_type("enum_entry"), Some("enum"));
        assert_eq!(classify_node_type("enum_constant"), Some("enum"));
    }

    #[test]
    fn test_type_classifications() {
        assert_eq!(classify_node_type("type_declaration"), Some("type"));
        assert_eq!(classify_node_type("type_alias_declaration"), Some("type"));
        assert_eq!(classify_node_type("type_spec"), Some("type"));
        assert_eq!(classify_node_type("type_item"), Some("type"));
    }

    #[test]
    fn test_variable_classifications() {
        assert_eq!(classify_node_type("variable_declaration"), Some("variable"));
        assert_eq!(classify_node_type("lexical_declaration"), Some("variable"));
        assert_eq!(classify_node_type("property_declaration"), Some("variable"));
        assert_eq!(classify_node_type("const_item"), Some("variable"));
        assert_eq!(classify_node_type("static_item"), Some("variable"));
        assert_eq!(classify_node_type("var_declaration"), Some("variable"));
    }

    #[test]
    fn test_module_classifications() {
        assert_eq!(classify_node_type("mod_item"), Some("module"));
        assert_eq!(classify_node_type("namespace_declaration"), Some("namespace"));
    }

    #[test]
    fn test_statement_classifications() {
        assert_eq!(classify_node_type("if_statement"), Some("if statement"));
        assert_eq!(classify_node_type("if_expression"), Some("if statement"));
        assert_eq!(classify_node_type("for_statement"), Some("for loop"));
        assert_eq!(classify_node_type("while_statement"), Some("while loop"));
        assert_eq!(classify_node_type("match_statement"), Some("match"));
        assert_eq!(classify_node_type("switch_statement"), Some("match"));
        assert_eq!(classify_node_type("return_statement"), Some("return"));
        assert_eq!(classify_node_type("assignment_expression"), Some("assignment"));
    }

    #[test]
    fn test_unknown_node_types() {
        assert_eq!(classify_node_type("identifier"), None);
        assert_eq!(classify_node_type("string_literal"), None);
        assert_eq!(classify_node_type("comment"), None);
        assert_eq!(classify_node_type("unknown_type"), None);
    }
}
