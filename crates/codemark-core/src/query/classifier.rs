/// Maps tree-sitter node types to human-readable labels for query summaries.
///
/// Classification consults the per-language [`Profile`] first, then falls back
/// to a global default table for node types the profile doesn't cover.
use crate::parser::profile::Profile;

/// Classify a node type using the language profile first, then falling back to
/// a global default table for node types not covered by the profile.
///
/// # Examples
///
/// ```
/// use codemark_core::query::classifier::classify_node_type;
/// use codemark_core::parser::languages::Language;
///
/// let profile = Language::Rust.profile();
/// assert_eq!(classify_node_type("function_item", Some(profile)).as_deref(), Some("function"));
/// assert_eq!(classify_node_type("identifier", Some(profile)).as_deref(), None);
/// ```
pub fn classify_node_type(node_type: &str, profile: Option<&Profile>) -> Option<String> {
    if let Some(p) = profile
        && let Some(label) = p.node_labels.get(node_type)
    {
        return Some(label.clone());
    }
    classify_node_type_global(node_type).map(|s| s.to_string())
}
/// Used as a fallback when the per-language profile doesn't have an entry.
fn classify_node_type_global(node_type: &str) -> Option<&'static str> {
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
        "for_statement" | "for" | "for_expression" => Some("for loop"),
        "while_statement" | "while" | "while_expression" => Some("while loop"),
        "match_statement" | "match_expression" | "switch_statement" => Some("match"),
        "return_statement" => Some("return"),
        "assignment_expression" | "assignment_statement" => Some("assignment"),

        // Expressions
        "call_expression" => Some("call"),
        "binary_expression" => Some("expression"),

        _ => None,
    }
}

/// Returns a NERD font icon for a given node label.
pub fn get_node_icon(label: &str) -> &'static str {
    match label {
        "function" | "method" | "constructor" => "", // nf-cod-symbol_method
        "class" | "struct" | "impl" => "",           // nf-cod-symbol_class
        "interface" | "protocol" | "trait" => "",    // nf-cod-symbol_interface
        "enum" => "",                                // nf-cod-symbol_enum
        "module" | "namespace" => "",                // nf-cod-symbol_module
        "variable" | "property" | "constant" => "",  // nf-cod-symbol_variable
        "type" => "",                                // nf-cod-symbol_parameter
        "macro" => "",                               // nf-cod-symbol_constant
        "if statement" | "match" => "",               // nf-cod-split_horizontal
        "for loop" | "while loop" => "",              // nf-cod-sync
        "call" => "",                                 // nf-cod-symbol_event
        _ => "",                                      // nf-cod-file
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_classifications() {
        assert_eq!(classify_node_type("function_item", None), Some("function".into()));
        assert_eq!(classify_node_type("function_declaration", None), Some("function".into()));
        assert_eq!(classify_node_type("function_definition", None), Some("function".into()));
        assert_eq!(classify_node_type("method_definition", None), Some("method".into()));
        assert_eq!(classify_node_type("method_declaration", None), Some("method".into()));
        assert_eq!(classify_node_type("constructor_declaration", None), Some("constructor".into()));
    }

    #[test]
    fn test_class_classifications() {
        assert_eq!(classify_node_type("class_declaration", None), Some("class".into()));
        assert_eq!(classify_node_type("struct_declaration", None), Some("class".into()));
        assert_eq!(classify_node_type("struct_item", None), Some("class".into()));
        assert_eq!(classify_node_type("interface_declaration", None), Some("interface".into()));
        assert_eq!(classify_node_type("protocol_declaration", None), Some("interface".into()));
        assert_eq!(classify_node_type("trait_item", None), Some("interface".into()));
        assert_eq!(classify_node_type("impl_item", None), Some("impl".into()));
    }

    #[test]
    fn test_enum_classifications() {
        assert_eq!(classify_node_type("enum_declaration", None), Some("enum".into()));
        assert_eq!(classify_node_type("enum_item", None), Some("enum".into()));
        assert_eq!(classify_node_type("enum_entry", None), Some("enum".into()));
        assert_eq!(classify_node_type("enum_constant", None), Some("enum".into()));
    }

    #[test]
    fn test_type_classifications() {
        assert_eq!(classify_node_type("type_declaration", None), Some("type".into()));
        assert_eq!(classify_node_type("type_alias_declaration", None), Some("type".into()));
        assert_eq!(classify_node_type("type_spec", None), Some("type".into()));
        assert_eq!(classify_node_type("type_item", None), Some("type".into()));
    }

    #[test]
    fn test_variable_classifications() {
        assert_eq!(classify_node_type("variable_declaration", None), Some("variable".into()));
        assert_eq!(classify_node_type("lexical_declaration", None), Some("variable".into()));
        assert_eq!(classify_node_type("property_declaration", None), Some("variable".into()));
        assert_eq!(classify_node_type("const_item", None), Some("variable".into()));
        assert_eq!(classify_node_type("static_item", None), Some("variable".into()));
        assert_eq!(classify_node_type("var_declaration", None), Some("variable".into()));
    }

    #[test]
    fn test_module_classifications() {
        assert_eq!(classify_node_type("mod_item", None), Some("module".into()));
        assert_eq!(classify_node_type("namespace_declaration", None), Some("namespace".into()));
    }

    #[test]
    fn test_statement_classifications() {
        assert_eq!(classify_node_type("if_statement", None), Some("if statement".into()));
        assert_eq!(classify_node_type("if_expression", None), Some("if statement".into()));
        assert_eq!(classify_node_type("for_statement", None), Some("for loop".into()));
        assert_eq!(classify_node_type("while_statement", None), Some("while loop".into()));
        assert_eq!(classify_node_type("match_statement", None), Some("match".into()));
        assert_eq!(classify_node_type("switch_statement", None), Some("match".into()));
        assert_eq!(classify_node_type("return_statement", None), Some("return".into()));
        assert_eq!(classify_node_type("assignment_expression", None), Some("assignment".into()));
    }

    #[test]
    fn test_unknown_node_types() {
        assert_eq!(classify_node_type("identifier", None), None);
        assert_eq!(classify_node_type("string_literal", None), None);
        assert_eq!(classify_node_type("comment", None), None);
        assert_eq!(classify_node_type("unknown_type", None), None);
    }
}
