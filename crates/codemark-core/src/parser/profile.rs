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
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::parser::languages::Language;

/// How much a [`LintFinding`] matters.
///
/// [`LintSeverity::Error`] means the profile entry can never match, so that part
/// of it is dead weight — a registry install refuses on one. A
/// [`LintSeverity::Warning`] still works but probably isn't what the author
/// meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LintSeverity {
    Error,
    Warning,
}

/// One problem found in a profile, located by a JSON-ish path so the author can
/// find it in their `manifest.json`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LintFinding {
    pub severity: LintSeverity,
    /// Where it is, e.g. `profile.containers[2].0`.
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for LintFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.severity {
            LintSeverity::Error => "error",
            LintSeverity::Warning => "warning",
        };
        write!(f, "{level}: {}: {}", self.path, self.message)
    }
}

/// Profile keys this build understands. A key outside this set deserializes to
/// nothing (every field is `#[serde(default)]`), so it is reported rather than
/// silently dropped — the failure that let a documented-but-unimplemented
/// `semantic_fields` sit in a committed fixture unnoticed.
const KNOWN_PROFILE_KEYS: &[&str] = &["landmark_kinds", "node_labels", "containers"];

/// Labels the built-in profiles use. `get_node_icon` keys off the label, so a
/// novel label parses fine but silently loses its icon.
static KNOWN_LABELS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "function",
        "method",
        "class",
        "interface",
        "enum",
        "impl",
        "module",
        "namespace",
        "type",
        "variable",
        "macro",
        "constructor",
        "init",
        "deinit",
        "subscript",
        "if statement",
        "for loop",
        "while loop",
        "match",
        "return",
        "assignment",
        "call",
        "expression",
    ]
    .into_iter()
    .collect()
});

/// Check a raw `profile` value against the grammar it targets.
///
/// Deserializing a [`Profile`] cannot catch any of this: every field defaults,
/// unknown keys are ignored (deliberately — see `ignores_unknown_profile_keys`),
/// and a node kind is just a string until something tries to match it. Valid
/// kinds and field names are properties of the *loaded grammar*, so this takes
/// the `tree_sitter::Language` and asks it.
///
/// `value` is the `profile` object itself, not the whole manifest.
pub fn lint(value: &serde_json::Value, grammar: &tree_sitter::Language) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    let Some(obj) = value.as_object() else {
        findings.push(LintFinding {
            severity: LintSeverity::Error,
            path: "profile".to_string(),
            message: format!("expected a JSON object, found {}", json_type_name(value)),
        });
        return findings;
    };

    // Check the shape before the contents. A field of the wrong type (a bare
    // string where `landmark_kinds` wants an array) makes the whole profile
    // fail to deserialize at discovery, but the per-field walks below just skip
    // it — so without this, `validate` would report a clean bill of health for
    // a manifest the registry reader rejects outright.
    if let Err(e) = serde_json::from_value::<Profile>(value.clone()) {
        findings.push(LintFinding {
            severity: LintSeverity::Error,
            path: "profile".to_string(),
            message: format!("does not match the profile schema, so it will not load: {e}"),
        });
        return findings;
    }

    for key in obj.keys() {
        if !KNOWN_PROFILE_KEYS.contains(&key.as_str()) {
            findings.push(LintFinding {
                severity: LintSeverity::Warning,
                path: format!("profile.{key}"),
                message: format!(
                    "unknown profile key — it is ignored at load time (known keys: {})",
                    KNOWN_PROFILE_KEYS.join(", ")
                ),
            });
        }
    }

    let named_kinds = named_node_kinds(grammar);
    let check_kind = |kind: &str, path: String, findings: &mut Vec<LintFinding>| {
        if !named_kinds.contains(kind) {
            findings.push(LintFinding {
                severity: LintSeverity::Error,
                path,
                message: format!(
                    "node kind '{kind}' is not defined by this grammar, so it can never match{}",
                    suggest(kind, &named_kinds)
                ),
            });
        }
    };

    if let Some(kinds) = obj.get("landmark_kinds").and_then(|v| v.as_array()) {
        for (i, k) in kinds.iter().enumerate() {
            if let Some(k) = k.as_str() {
                check_kind(k, format!("profile.landmark_kinds[{i}]"), &mut findings);
            }
        }
    }

    if let Some(labels) = obj.get("node_labels").and_then(|v| v.as_object()) {
        for (kind, label) in labels {
            check_kind(kind, format!("profile.node_labels.{kind}"), &mut findings);
            if let Some(label) = label.as_str()
                && !KNOWN_LABELS.contains(label)
            {
                findings.push(LintFinding {
                    severity: LintSeverity::Warning,
                    path: format!("profile.node_labels.{kind}"),
                    message: format!(
                        "label '{label}' is outside the set the built-in profiles use, so it will \
                         not get an icon"
                    ),
                });
            }
        }
    }

    match obj.get("containers").and_then(|v| v.as_array()) {
        Some(containers) if !containers.is_empty() => {
            let fields = grammar_field_names(grammar);
            for (i, pair) in containers.iter().enumerate() {
                let Some(pair) = pair.as_array() else { continue };
                let (Some(kind), Some(field)) =
                    (pair.first().and_then(|v| v.as_str()), pair.get(1).and_then(|v| v.as_str()))
                else {
                    continue;
                };
                check_kind(kind, format!("profile.containers[{i}].0"), &mut findings);
                // Tree-sitter's static `Language` exposes field names globally,
                // not per node kind, so this only catches a field the grammar
                // has never heard of; one that exists on *some other* kind
                // (`("mod_item", "value")`) passes.
                //
                // That gap is currently cosmetic: `extract_breadcrumbs` matches
                // on the kind alone and never reads this field, so a wrong one
                // changes nothing at runtime. It is still worth validating —
                // the field is part of the schema, the generator derives it
                // from an observed parse, and breadcrumbs may yet use it to
                // extract the name rather than the whole line.
                if !fields.contains(field) {
                    findings.push(LintFinding {
                        severity: LintSeverity::Error,
                        path: format!("profile.containers[{i}].1"),
                        message: format!(
                            "'{field}' is not a field in this grammar, so the container entry for \
                             '{kind}' names something that doesn't exist{}",
                            suggest(field, &fields)
                        ),
                    });
                }
            }
        }
        _ => {
            // Unlike `landmark_kinds`, which falls back to the built-in
            // DECLARATION_TYPES union, `containers` has no fallback at all:
            // `extract_breadcrumbs` builds its whitelist purely from this, so an
            // empty list means breadcrumbs are silently blank.
            findings.push(LintFinding {
                severity: LintSeverity::Warning,
                path: "profile.containers".to_string(),
                message: "no containers, so this language produces no breadcrumbs".to_string(),
            });
        }
    }

    findings
}

/// Every *named* node kind the grammar defines. Anonymous kinds (punctuation and
/// keywords) are excluded: a profile naming one would be a mistake too, but
/// listing them in a suggestion would be noise.
fn named_node_kinds(grammar: &tree_sitter::Language) -> HashSet<String> {
    (0..grammar.node_kind_count())
        .filter_map(|id| {
            let id = u16::try_from(id).ok()?;
            grammar.node_kind_is_named(id).then(|| grammar.node_kind_for_id(id))?
        })
        .filter(|k| !k.is_empty())
        .map(String::from)
        .collect()
}

/// Every field name the grammar defines. Field ids are 1-based.
fn grammar_field_names(grammar: &tree_sitter::Language) -> HashSet<String> {
    (1..=grammar.field_count())
        .filter_map(|id| grammar.field_name_for_id(u16::try_from(id).ok()?))
        .map(String::from)
        .collect()
}

/// `" (did you mean 'x'?)"` when something close exists, else empty.
///
/// Profile mistakes are overwhelmingly near-misses — a kind borrowed from a
/// sibling language (`call_expression` where the grammar says `function_call`),
/// or a plural/singular slip — so naming the near neighbor usually resolves the
/// finding without a trip to `tree-sitter parse`.
fn suggest(needle: &str, haystack: &HashSet<String>) -> String {
    let mut best: Option<(usize, &str)> = None;
    for candidate in haystack {
        let d = edit_distance(needle, candidate);
        // Allow roughly a quarter of the word to differ, at least 2 edits, so
        // short names don't match everything.
        let budget = (needle.len() / 4).max(2);
        if d <= budget && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, candidate));
        }
    }
    match best {
        Some((_, c)) => format!(" (did you mean '{c}'?)"),
        None => String::new(),
    }
}

/// Levenshtein distance, two rows at a time.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Language-specific structural metadata.
///
/// All fields are optional with graceful fallbacks so a partial manifest still
/// works — an empty `landmark_kinds` means no structural anchoring, a missing
/// `node_labels` entry falls back to `node_type.replace('_', " ")`.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Profile {
    /// Node types that act as structural anchors for the query generator.
    ///
    /// This list is used by the engine to disambiguate tree-sitter queries and ensure
    /// bookmarks are stable across refactors. It includes all major declarations (e.g.,
    /// classes, functions, constants, macros).
    /// Consulted by `is_landmark_kind`.
    pub landmark_kinds: Vec<String>,

    /// Node type → human-readable label mapping.
    /// Replaces `classify_node_type()`.
    pub node_labels: HashMap<String, String>,

    /// Container node kinds paired with their name field, e.g. `("impl_item", "type")`.
    ///
    /// Unlike `landmark_kinds` (which is for machine-level query stability), `containers`
    /// is used strictly for the human-facing UI to build contextual breadcrumb paths
    /// (e.g., `Class > Method`). Node types here are usually a strict subset of
    /// `landmark_kinds`, limited only to nodes that nest other executable code.
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

#[cfg(test)]
mod tests {
    use super::*;

    // A manifest may specify only the fields it cares about; the rest must fall
    // back to empty defaults rather than failing to deserialize. This is what
    // lets a partial `manifest.json` register instead of being skipped.
    #[test]
    fn deserializes_partial_profile() {
        let json = r#"{ "landmark_kinds": ["local_function_declaration"] }"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.landmark_kinds, vec!["local_function_declaration".to_string()]);
        assert!(profile.node_labels.is_empty());
        assert!(profile.containers.is_empty());
    }

    #[test]
    fn deserializes_empty_profile() {
        let profile: Profile = serde_json::from_str("{}").unwrap();
        assert!(profile.landmark_kinds.is_empty());
        assert!(profile.node_labels.is_empty());
        assert!(profile.containers.is_empty());
    }

    // Unknown keys (e.g. a future `semantic_fields`) are ignored, not fatal, so
    // newer manifests still load on older binaries.
    #[test]
    fn ignores_unknown_profile_keys() {
        let json =
            r#"{ "landmark_kinds": ["x"], "semantic_fields": { "if_statement": "condition" } }"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.landmark_kinds, vec!["x".to_string()]);
    }

    // `Profile` round-trips so the generator can emit one and the CLI can show it.
    #[test]
    fn serializes_round_trip() {
        let profile = Profile {
            landmark_kinds: vec!["function_item".into()],
            node_labels: HashMap::from([("function_item".into(), "function".into())]),
            containers: vec![("impl_item".into(), "type".into())],
        };
        let json = serde_json::to_string(&profile).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.landmark_kinds, profile.landmark_kinds);
        assert_eq!(back.containers, profile.containers);
        assert_eq!(back.node_labels.get("function_item").map(String::as_str), Some("function"));
    }

    // --- lint ---------------------------------------------------------------
    //
    // Linted against the real built-in Rust grammar, so these assert against the
    // kinds and fields tree-sitter actually defines rather than a stub.

    fn rust_grammar() -> tree_sitter::Language {
        Language::Rust.tree_sitter_language()
    }

    fn errors(findings: &[LintFinding]) -> Vec<&LintFinding> {
        findings.iter().filter(|f| f.severity == LintSeverity::Error).collect()
    }

    #[test]
    fn lint_accepts_a_correct_profile() {
        let value = serde_json::json!({
            "landmark_kinds": ["function_item", "struct_item"],
            "node_labels": { "function_item": "function" },
            "containers": [["impl_item", "type"]],
        });
        assert!(errors(&lint(&value, &rust_grammar())).is_empty());
    }

    #[test]
    fn lint_flags_a_node_kind_the_grammar_does_not_define() {
        // `function_declaration` is Swift/TS; Rust says `function_item`. This is
        // the most common real mistake: a kind borrowed from another language.
        let value = serde_json::json!({ "landmark_kinds": ["function_declaration"] });
        let findings = lint(&value, &rust_grammar());
        let errs = errors(&findings);
        assert_eq!(errs.len(), 1, "{findings:?}");
        assert_eq!(errs[0].path, "profile.landmark_kinds[0]");
        assert!(errs[0].message.contains("not defined by this grammar"));
    }

    #[test]
    fn lint_suggests_a_near_miss_kind() {
        // A one-character slip should name the intended kind.
        let value = serde_json::json!({ "landmark_kinds": ["function_itm"] });
        let findings = lint(&value, &rust_grammar());
        assert!(
            errors(&findings)[0].message.contains("did you mean 'function_item'?"),
            "{findings:?}"
        );
    }

    #[test]
    fn lint_flags_a_container_field_the_grammar_does_not_have() {
        // The kind is real but the name-field isn't, so the container would
        // silently produce no breadcrumb.
        let value = serde_json::json!({ "containers": [["impl_item", "nombre"]] });
        let findings = lint(&value, &rust_grammar());
        let errs = errors(&findings);
        assert_eq!(errs.len(), 1, "{findings:?}");
        assert_eq!(errs[0].path, "profile.containers[0].1");
    }

    // Documents a known limit rather than asserting desirable behavior: the
    // static `Language` API exposes field names globally, not per kind, so a
    // field that is real *somewhere* in the grammar passes even when the paired
    // kind doesn't carry it.
    //
    // Harmless today — `extract_breadcrumbs` reads only the kind, so the field
    // can't affect output — but it would bite if breadcrumbs ever used the
    // field to extract the name. If tree-sitter exposes per-kind fields, this
    // should become an error and the test should flip.
    #[test]
    fn lint_cannot_catch_a_field_valid_on_a_different_kind() {
        // `value` is a real Rust field, but not one `mod_item` has.
        let value = serde_json::json!({ "containers": [["mod_item", "value"]] });
        let findings = lint(&value, &rust_grammar());
        assert!(errors(&findings).is_empty(), "known limit — see the comment above: {findings:?}");
    }

    #[test]
    fn lint_warns_on_unknown_profile_key() {
        // The exact drift that shipped in the Lua fixture: a key that
        // deserialization silently discards.
        let value = serde_json::json!({
            "containers": [["impl_item", "type"]],
            "semantic_fields": { "if_statement": "condition" },
        });
        let findings = lint(&value, &rust_grammar());
        assert!(errors(&findings).is_empty(), "unknown keys are a warning, not an error");
        assert!(
            findings.iter().any(|f| f.path == "profile.semantic_fields"
                && f.severity == LintSeverity::Warning),
            "{findings:?}"
        );
    }

    #[test]
    fn lint_warns_when_containers_is_empty_or_absent() {
        // No fallback exists for containers, so blank breadcrumbs are silent.
        for value in [serde_json::json!({}), serde_json::json!({ "containers": [] })] {
            let findings = lint(&value, &rust_grammar());
            assert!(
                findings
                    .iter()
                    .any(|f| f.path == "profile.containers" && f.severity == LintSeverity::Warning),
                "{findings:?}"
            );
        }
    }

    #[test]
    fn lint_warns_on_a_label_outside_the_known_vocabulary() {
        let value = serde_json::json!({
            "node_labels": { "function_item": "subroutine" },
            "containers": [["impl_item", "type"]],
        });
        let findings = lint(&value, &rust_grammar());
        assert!(errors(&findings).is_empty());
        assert!(
            findings.iter().any(|f| f.message.contains("will not get an icon")),
            "{findings:?}"
        );
    }

    #[test]
    fn lint_rejects_a_profile_that_is_not_an_object() {
        let findings = lint(&serde_json::json!("landmark_kinds"), &rust_grammar());
        assert_eq!(errors(&findings).len(), 1);
    }

    #[test]
    fn lint_rejects_a_field_of_the_wrong_type() {
        // A bare string where an array belongs makes the whole profile fail to
        // deserialize at discovery. Walking the fields individually would just
        // skip it, so `validate` would call a manifest valid that the registry
        // reader refuses to load.
        for bad in [
            serde_json::json!({ "landmark_kinds": "function_item" }),
            serde_json::json!({ "node_labels": ["function_item"] }),
            serde_json::json!({ "containers": { "impl_item": "type" } }),
        ] {
            assert!(
                serde_json::from_value::<Profile>(bad.clone()).is_err(),
                "fixture should be undeserializable: {bad}"
            );
            let findings = lint(&bad, &rust_grammar());
            assert!(
                errors(&findings).iter().any(|f| f.message.contains("profile schema")),
                "{bad} produced {findings:?}"
            );
        }
    }
}
