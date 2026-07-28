//! Language-specific structural metadata (the "Profile").
//!
//! Replaces four hardcoded per-language lookup tables with data-driven
//! configuration:
//!
//! - [`Profile::landmark_kinds`] — replaces `DECLARATION_TYPES` in `generator.rs`
//! - [`Profile::node_labels`] — replaces `classify_node_type()` in `classifier.rs`
//! - [`Profile::containers`] — replaces `get_whitelist()` in `breadcrumbs.rs`
//!
//! For static languages a matching [`Profile`] is built at first access via
//! [`Language::profile`]. For WASM-loaded dynamic languages it will come from
//! `manifest.json` (Phase 2).

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::parser::languages::Language;

/// Language-specific structural metadata.
///
/// All fields are optional with graceful fallbacks so a partial manifest still
/// works — an empty `landmark_kinds` means no structural anchoring, a missing
/// `node_labels` entry falls back to `node_type.replace('_', " ")`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Profile {
    /// Node types that get structural anchoring in the query generator.
    ///
    /// Populated per language here, but the query generator is switched over to
    /// consult this field (in place of the hardcoded `DECLARATION_TYPES` table)
    /// in the follow-up dynamic-grammar PR. Until then it carries the intended
    /// data without yet driving generation, so the two must be kept in sync.
    pub landmark_kinds: Vec<String>,

    /// Node type → human-readable label mapping.
    /// Replaces `classify_node_type()`.
    pub node_labels: HashMap<String, String>,

    /// Container node kinds paired with their name field, e.g. `("impl_item", "type")`.
    /// These are the node types whose first source line makes a useful breadcrumb
    /// ancestor. Replaces `get_whitelist()`.
    pub containers: Vec<(String, String)>,
}

impl Language {
    /// Returns the structural [`Profile`] for this language.
    ///
    /// Static languages return a compile-time [`LazyLock`] constant matching the
    /// pre-refactor hardcoded behavior. Dynamic languages (Phase 2) will return
    /// their manifest-loaded profile.
    pub fn profile(&self) -> &Profile {
        match self {
            Language::Rust => &RUST_PROFILE,
            Language::Swift => &SWIFT_PROFILE,
            Language::TypeScript => &TYPESCRIPT_PROFILE,
            Language::Python => &PYTHON_PROFILE,
            Language::Go => &GO_PROFILE,
            Language::Java => &JAVA_PROFILE,
            Language::CSharp => &CSHARP_PROFILE,
            Language::Dart => &DART_PROFILE,
            Language::Dynamic(dl) => &dl.profile,
        }
    }
}

// ---------------------------------------------------------------------------
// Static language profiles — each mirrors the exact data that was previously
// hardcoded in match arms. Zero behavior change.
// ---------------------------------------------------------------------------

static RUST_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "function_item".into(),
        "struct_item".into(),
        "enum_item".into(),
        "trait_item".into(),
        "impl_item".into(),
        "type_item".into(),
        "const_item".into(),
        "static_item".into(),
        "mod_item".into(),
        "macro_definition".into(),
    ],
    node_labels: HashMap::from([
        ("function_item".into(), "function".into()),
        ("struct_item".into(), "class".into()),
        ("enum_item".into(), "enum".into()),
        ("enum_entry".into(), "enum".into()),
        ("enum_constant".into(), "enum".into()),
        ("trait_item".into(), "interface".into()),
        ("impl_item".into(), "impl".into()),
        ("type_item".into(), "type".into()),
        ("const_item".into(), "variable".into()),
        ("static_item".into(), "variable".into()),
        ("mod_item".into(), "module".into()),
        ("macro_definition".into(), "macro".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("match_expression".into(), "match".into()),
        ("return_statement".into(), "return".into()),
        ("assignment_expression".into(), "assignment".into()),
        ("call_expression".into(), "call".into()),
        ("binary_expression".into(), "expression".into()),
    ]),
    containers: vec![
        ("impl_item".into(), "type".into()),
        ("trait_item".into(), "name".into()),
        ("mod_item".into(), "name".into()),
        ("function_item".into(), "name".into()),
    ],
});

static SWIFT_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "function_declaration".into(),
        "protocol_declaration".into(),
        "protocol_function_declaration".into(),
        "init_declaration".into(),
        "deinit_declaration".into(),
        "subscript_declaration".into(),
        "enum_declaration".into(),
        "enum_entry".into(),
        "struct_declaration".into(),
    ],
    node_labels: HashMap::from([
        ("function_declaration".into(), "function".into()),
        ("class_declaration".into(), "class".into()),
        ("struct_declaration".into(), "class".into()),
        ("protocol_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("enum_entry".into(), "enum".into()),
        ("init_declaration".into(), "init".into()),
        ("deinit_declaration".into(), "deinit".into()),
        ("subscript_declaration".into(), "subscript".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("switch_statement".into(), "match".into()),
        ("return_statement".into(), "return".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("class_declaration".into(), "name".into()),
        ("extension_declaration".into(), "name".into()),
        ("protocol_declaration".into(), "name".into()),
        ("function_declaration".into(), "name".into()),
        ("struct_declaration".into(), "name".into()),
        ("enum_declaration".into(), "name".into()),
    ],
});

static TYPESCRIPT_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "interface_declaration".into(),
        "enum_declaration".into(),
        "method_declaration".into(),
        "method_definition".into(),
        "function_declaration".into(),
        "type_alias_declaration".into(),
        "lexical_declaration".into(),
        "export_statement".into(),
    ],
    node_labels: HashMap::from([
        ("function_declaration".into(), "function".into()),
        ("method_definition".into(), "method".into()),
        ("method_declaration".into(), "method".into()),
        ("constructor_declaration".into(), "constructor".into()),
        ("class_declaration".into(), "class".into()),
        ("interface_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("type_alias_declaration".into(), "type".into()),
        ("lexical_declaration".into(), "variable".into()),
        ("variable_declaration".into(), "variable".into()),
        ("namespace_declaration".into(), "namespace".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("assignment_expression".into(), "assignment".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("namespace_declaration".into(), "name".into()),
        ("class_declaration".into(), "name".into()),
        ("interface_declaration".into(), "name".into()),
        ("method_definition".into(), "key".into()),
        ("function_declaration".into(), "name".into()),
        ("type_alias_declaration".into(), "name".into()),
        ("enum_declaration".into(), "name".into()),
    ],
});

static PYTHON_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_definition".into(),
        "function_definition".into(),
        "decorated_definition".into(),
    ],
    node_labels: HashMap::from([
        ("function_definition".into(), "function".into()),
        ("class_definition".into(), "class".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("assignment_statement".into(), "assignment".into()),
        ("call".into(), "call".into()),
    ]),
    containers: vec![
        ("class_definition".into(), "name".into()),
        ("function_definition".into(), "name".into()),
        ("async_function_definition".into(), "name".into()),
    ],
});

static GO_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "type_declaration".into(),
        "type_spec".into(),
        "var_declaration".into(),
        "function_declaration".into(),
        "method_declaration".into(),
    ],
    node_labels: HashMap::from([
        ("function_declaration".into(), "function".into()),
        ("method_declaration".into(), "method".into()),
        ("type_declaration".into(), "type".into()),
        ("type_spec".into(), "type".into()),
        ("var_declaration".into(), "variable".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("return_statement".into(), "return".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("type_declaration".into(), "type".into()),
        ("function_declaration".into(), "name".into()),
        ("method_declaration".into(), "name".into()),
    ],
});

static JAVA_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "interface_declaration".into(),
        "enum_declaration".into(),
        "method_declaration".into(),
        "constructor_declaration".into(),
        "property_declaration".into(),
    ],
    node_labels: HashMap::from([
        ("method_declaration".into(), "method".into()),
        ("constructor_declaration".into(), "constructor".into()),
        ("class_declaration".into(), "class".into()),
        ("interface_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("property_declaration".into(), "variable".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("class_declaration".into(), "name".into()),
        ("interface_declaration".into(), "name".into()),
        ("enum_declaration".into(), "name".into()),
        ("method_declaration".into(), "name".into()),
    ],
});

static CSHARP_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "interface_declaration".into(),
        "struct_declaration".into(),
        "enum_declaration".into(),
        "method_declaration".into(),
        "constructor_declaration".into(),
        "namespace_declaration".into(),
        "record_declaration".into(),
        "property_declaration".into(),
        "method_elem".into(),
    ],
    node_labels: HashMap::from([
        ("method_declaration".into(), "method".into()),
        ("constructor_declaration".into(), "constructor".into()),
        ("class_declaration".into(), "class".into()),
        ("struct_declaration".into(), "class".into()),
        ("interface_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("namespace_declaration".into(), "namespace".into()),
        ("property_declaration".into(), "variable".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("switch_statement".into(), "match".into()),
        ("return_statement".into(), "return".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("class_declaration".into(), "name".into()),
        ("interface_declaration".into(), "name".into()),
        ("struct_declaration".into(), "name".into()),
        ("enum_declaration".into(), "name".into()),
        ("method_declaration".into(), "name".into()),
        ("namespace_declaration".into(), "name".into()),
    ],
});

static DART_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_definition".into(),
        "mixin_definition".into(),
        "extension_definition".into(),
        "enum_definition".into(),
        "function_signature".into(),
        "method_declaration".into(),
        "initialized_identifier".into(),
        "enum_constant".into(),
    ],
    node_labels: HashMap::from([
        ("function_signature".into(), "function".into()),
        ("method_declaration".into(), "method".into()),
        ("class_definition".into(), "class".into()),
        ("mixin_definition".into(), "class".into()),
        ("extension_definition".into(), "class".into()),
        ("enum_definition".into(), "enum".into()),
        ("enum_constant".into(), "enum".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: vec![
        ("class_definition".into(), "name".into()),
        ("mixin_definition".into(), "name".into()),
        ("extension_definition".into(), "name".into()),
        ("enum_definition".into(), "name".into()),
        ("function_signature".into(), "name".into()),
        ("method_declaration".into(), "name".into()),
    ],
});
