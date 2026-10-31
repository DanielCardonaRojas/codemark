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
            // The declared labels, so a container naming a concept no kind in
            // this profile carries can be reported. That is the concept-form
            // equivalent of naming a node kind the grammar doesn't define: the
            // entry can never match, and `containers` has no fallback.
            let declared_labels: HashSet<&str> = obj
                .get("node_labels")
                .and_then(|v| v.as_object())
                .map(|m| m.values().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            for (i, entry) in containers.iter().enumerate() {
                match entry {
                    // Concept form: `"class"`.
                    serde_json::Value::String(label) => {
                        if !declared_labels.contains(label.as_str()) {
                            findings.push(LintFinding {
                                severity: LintSeverity::Error,
                                path: format!("profile.containers[{i}]"),
                                message: format!(
                                    "no node kind in this profile is labelled '{label}', so this \
                                     container can never match{}",
                                    suggest(
                                        label,
                                        &declared_labels
                                            .iter()
                                            .map(|s| s.to_string())
                                            .collect::<HashSet<_>>()
                                    )
                                ),
                            });
                        }
                        if !KNOWN_LABELS.contains(label.as_str()) {
                            findings.push(LintFinding {
                                severity: LintSeverity::Warning,
                                path: format!("profile.containers[{i}]"),
                                message: format!(
                                    "label '{label}' is outside the set the built-in profiles use"
                                ),
                            });
                        }
                    }
                    // Legacy pair form: `["class_declaration", "name"]`. Still
                    // read so an older manifest loads, so still linted — but
                    // only the kind, since the name field is no longer part of
                    // the schema.
                    serde_json::Value::Array(pair) => {
                        if let Some(kind) = pair.first().and_then(|v| v.as_str()) {
                            check_kind(kind, format!("profile.containers[{i}][0]"), &mut findings);
                            findings.push(LintFinding {
                                severity: LintSeverity::Warning,
                                path: format!("profile.containers[{i}]"),
                                message: format!(
                                    "legacy [kind, field] form — '{kind}' still works, but \
                                     `containers` now takes labels from node_labels"
                                ),
                            });
                        }
                    }
                    _ => {}
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

    /// Landmarks the UI treats specially, as **labels from [`Self::node_labels`]**.
    ///
    /// Drives two presentations of one idea:
    ///
    /// | Presentation | Needs | A container with no name |
    /// |---|---|---|
    /// | Sticky header | the *line*, pinned while scrolling | works |
    /// | Breadcrumb trail | the *name*, for a path to the target | skipped |
    ///
    /// So a kind with no name field is still a valid container. A Zig struct is
    /// an anonymous value bound by its enclosing `variable_declaration` — you
    /// want it pinned while scrolling its methods, and the breadcrumb renderer
    /// is what decides to leave it out.
    ///
    /// Accepts both the current form and the legacy `[kind, name_field]` pairs,
    /// so a manifest written for an older codemark keeps working. See
    /// [`Containers`].
    pub containers: Containers,
}

/// `containers` in either the concept form or the legacy pair form.
///
/// ```json
/// "containers": ["class", "method"]                       // concepts
/// "containers": [["class_declaration", "name"]]           // legacy pairs
/// ```
///
/// Keying on labels rather than node kinds removes a three-way repetition: a
/// kind previously had to be listed in `landmark_kinds`, in `node_labels` and
/// again in `containers`. It also lets one cross-language rule ("pin types and
/// functions") be written once.
///
/// The legacy form is still read because `Profile` is `#[serde(default)]` with
/// no `deny_unknown_fields` precisely so an older manifest loads on a newer
/// binary. Its name field is dropped on the way in: nothing has ever read it
/// (the breadcrumb walker extracts the whole source line), and under the
/// concept form there is no per-kind slot for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Containers {
    /// Labels, e.g. `["class", "method"]`.
    labels: Vec<String>,
    /// Node kinds from a legacy manifest, kept so breadcrumbs keep working
    /// before that manifest is migrated.
    legacy_kinds: Vec<String>,
}

impl Containers {
    /// Build from labels.
    pub fn from_labels(labels: Vec<String>) -> Self {
        Self { labels, legacy_kinds: Vec::new() }
    }

    /// The labels this profile marks as containers.
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Node kinds carried over from a legacy `[kind, field]` manifest.
    pub fn legacy_kinds(&self) -> &[String] {
        &self.legacy_kinds
    }

    /// Whether anything is listed at all. An empty `containers` means no
    /// breadcrumbs and no sticky headers, with no fallback anywhere.
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.legacy_kinds.is_empty()
    }

    /// Resolve to the set of node kinds this profile treats as containers.
    ///
    /// Labels are mapped back through `node_labels`; legacy kinds pass through
    /// unchanged. A profile part-way through migration can carry both.
    pub fn resolve_kinds(&self, node_labels: &HashMap<String, String>) -> Vec<String> {
        let mut kinds: Vec<String> = self.legacy_kinds.clone();
        for (kind, label) in node_labels {
            if self.labels.iter().any(|l| l == label) && !kinds.iter().any(|k| k == kind) {
                kinds.push(kind.clone());
            }
        }
        kinds.sort();
        kinds
    }
}

impl serde::Serialize for Containers {
    /// Always emits the concept form, so `profile init` and `profile show`
    /// write the new shape even when the input was a legacy manifest. A legacy
    /// kind that has no label would otherwise be lost silently, so it is kept
    /// as a one-element pair — visible, and still readable by an older binary.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.labels.len() + self.legacy_kinds.len()))?;
        for label in &self.labels {
            seq.serialize_element(label)?;
        }
        for kind in &self.legacy_kinds {
            seq.serialize_element(&(kind,))?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for Containers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        /// One entry: a bare label, or a legacy `[kind, field]` pair. A pair
        /// with a null or absent field is accepted too — that is how a legacy
        /// manifest spells a container with no name.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Entry {
            Label(String),
            // The second element is the legacy name field. Read so the pair
            // still deserializes, then discarded: nothing ever consumed it.
            Pair(String, #[allow(dead_code)] Option<String>),
            KindOnly((String,)),
        }

        let mut labels = Vec::new();
        let mut legacy_kinds = Vec::new();
        for entry in Vec::<Entry>::deserialize(d)? {
            match entry {
                Entry::Label(l) => labels.push(l),
                Entry::Pair(kind, _) => legacy_kinds.push(kind),
                Entry::KindOnly((kind,)) => legacy_kinds.push(kind),
            }
        }
        Ok(Self { labels, legacy_kinds })
    }
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
        ("enum_variant".into(), "enum".into()),
        ("trait_item".into(), "interface".into()),
        ("impl_item".into(), "impl".into()),
        ("type_item".into(), "type".into()),
        ("const_item".into(), "variable".into()),
        ("static_item".into(), "variable".into()),
        ("mod_item".into(), "module".into()),
        ("macro_definition".into(), "macro".into()),
        ("if_expression".into(), "if statement".into()),
        ("for_expression".into(), "for loop".into()),
        ("while_expression".into(), "while loop".into()),
        ("match_expression".into(), "match".into()),
        ("return_expression".into(), "return".into()),
        ("assignment_expression".into(), "assignment".into()),
        ("call_expression".into(), "call".into()),
        ("binary_expression".into(), "expression".into()),
    ]),
    containers: Containers::from_labels(vec![
        "function".into(),
        "impl".into(),
        "interface".into(),
        "module".into(),
    ]),
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
        "enum_entry".into(),
    ],
    node_labels: HashMap::from([
        ("function_declaration".into(), "function".into()),
        ("class_declaration".into(), "class".into()),
        ("protocol_declaration".into(), "interface".into()),
        ("enum_entry".into(), "enum".into()),
        ("init_declaration".into(), "init".into()),
        ("deinit_declaration".into(), "deinit".into()),
        // A Swift extension is a container, so it needs a label for
        // `containers` to name it as a concept. `impl` is the same idea Rust's
        // `impl_item` carries: methods attached to a type declared elsewhere.
        ("subscript_declaration".into(), "subscript".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("switch_statement".into(), "match".into()),
        ("call_expression".into(), "call".into()),
    ]),
    // No `impl`: this grammar has no extension kind — `class_declaration`
    // covers class, struct, enum and extension alike, so an extension is
    // already reached through `class`.
    containers: Containers::from_labels(vec![
        "class".into(),
        "enum".into(),
        "function".into(),
        "interface".into(),
    ]),
});

static TYPESCRIPT_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "interface_declaration".into(),
        "enum_declaration".into(),
        "method_definition".into(),
        "function_declaration".into(),
        "type_alias_declaration".into(),
        "lexical_declaration".into(),
        "export_statement".into(),
    ],
    node_labels: HashMap::from([
        ("function_declaration".into(), "function".into()),
        ("method_definition".into(), "method".into()),
        ("class_declaration".into(), "class".into()),
        ("interface_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("type_alias_declaration".into(), "type".into()),
        ("lexical_declaration".into(), "variable".into()),
        ("variable_declaration".into(), "variable".into()),
        ("internal_module".into(), "namespace".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("assignment_expression".into(), "assignment".into()),
        ("call_expression".into(), "call".into()),
    ]),
    containers: Containers::from_labels(vec![
        "class".into(),
        "enum".into(),
        "function".into(),
        "interface".into(),
        "method".into(),
        "namespace".into(),
        "type".into(),
    ]),
});

static PYTHON_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_definition".into(),
        "function_definition".into(),
        "decorated_definition".into(),
    ],
    node_labels: HashMap::from([
        // Covers `async def` too: tree-sitter-python parses it as a
        // `function_definition` with an `async` child, not a distinct kind.
        ("function_definition".into(), "function".into()),
        ("class_definition".into(), "class".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("assignment".into(), "assignment".into()),
        ("call".into(), "call".into()),
    ]),
    containers: Containers::from_labels(vec!["class".into(), "function".into()]),
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
    containers: Containers::from_labels(vec!["function".into(), "method".into(), "type".into()]),
});

static JAVA_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "interface_declaration".into(),
        "enum_declaration".into(),
        "method_declaration".into(),
        "constructor_declaration".into(),
    ],
    node_labels: HashMap::from([
        ("method_declaration".into(), "method".into()),
        ("constructor_declaration".into(), "constructor".into()),
        ("class_declaration".into(), "class".into()),
        ("interface_declaration".into(), "interface".into()),
        ("enum_declaration".into(), "enum".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("method_invocation".into(), "call".into()),
    ]),
    containers: Containers::from_labels(vec![
        "class".into(),
        "enum".into(),
        "interface".into(),
        "method".into(),
    ]),
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
        ("invocation_expression".into(), "call".into()),
    ]),
    containers: Containers::from_labels(vec![
        "class".into(),
        "enum".into(),
        "interface".into(),
        "method".into(),
        "namespace".into(),
    ]),
});

static DART_PROFILE: LazyLock<Profile> = LazyLock::new(|| Profile {
    landmark_kinds: vec![
        "class_declaration".into(),
        "mixin_declaration".into(),
        "extension_declaration".into(),
        "enum_declaration".into(),
        "function_signature".into(),
        "method_signature".into(),
        "initialized_identifier".into(),
        "enum_constant".into(),
    ],
    node_labels: HashMap::from([
        ("function_signature".into(), "function".into()),
        ("method_signature".into(), "method".into()),
        ("class_declaration".into(), "class".into()),
        ("mixin_declaration".into(), "class".into()),
        ("extension_declaration".into(), "class".into()),
        ("enum_declaration".into(), "enum".into()),
        ("enum_constant".into(), "enum".into()),
        ("if_statement".into(), "if statement".into()),
        ("for_statement".into(), "for loop".into()),
        ("while_statement".into(), "while loop".into()),
        ("return_statement".into(), "return".into()),
        ("function_expression".into(), "function".into()),
    ]),
    containers: Containers::from_labels(vec![
        "class".into(),
        "enum".into(),
        "function".into(),
        "method".into(),
    ]),
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
            containers: Containers::from_labels(vec!["impl".into(), "function".into()]),
        };
        let json = serde_json::to_string(&profile).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.landmark_kinds, profile.landmark_kinds);
        assert_eq!(back.containers, profile.containers);
        assert_eq!(back.node_labels.get("function_item").map(String::as_str), Some("function"));
    }

    /// A manifest written for an older codemark must keep working. `Profile` is
    /// `#[serde(default)]` with no `deny_unknown_fields` precisely so that an
    /// older manifest loads on a newer binary; this covers the shape change.
    #[test]
    fn reads_the_legacy_container_pair_form() {
        let json = r#"{
            "landmark_kinds": ["impl_item"],
            "node_labels": { "impl_item": "impl" },
            "containers": [["impl_item", "type"], ["mod_item", "name"]]
        }"#;
        let profile: Profile = serde_json::from_str(json).unwrap();

        // The pairs resolve to the same node kinds the old walker used, so
        // breadcrumbs are unchanged before the manifest is migrated.
        let kinds = profile.containers.resolve_kinds(&profile.node_labels);
        assert!(kinds.iter().any(|k| k == "impl_item"), "{kinds:?}");
        assert!(kinds.iter().any(|k| k == "mod_item"), "{kinds:?}");

        // The name field is dropped: nothing ever read it, and the concept form
        // has no per-kind slot for it.
        assert!(profile.containers.labels().is_empty());
    }

    /// Both forms in one list, which is what a part-migrated manifest looks
    /// like. A legacy kind with no label would otherwise vanish.
    #[test]
    fn reads_a_mixed_container_list() {
        let json = r#"{
            "node_labels": { "class_declaration": "class" },
            "containers": ["class", ["weird_kind", "name"]]
        }"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        let kinds = profile.containers.resolve_kinds(&profile.node_labels);
        assert!(kinds.iter().any(|k| k == "class_declaration"), "label did not resolve: {kinds:?}");
        assert!(kinds.iter().any(|k| k == "weird_kind"), "legacy kind was lost: {kinds:?}");
    }

    /// A concept names every kind carrying that label, which is the point of
    /// keying on the classified vocabulary: one entry covers a grammar's
    /// several spellings of the same idea.
    #[test]
    fn one_concept_resolves_to_every_kind_with_that_label() {
        let profile = Profile {
            node_labels: HashMap::from([
                ("class_declaration".into(), "class".into()),
                ("struct_declaration".into(), "class".into()),
                ("if_statement".into(), "if statement".into()),
            ]),
            containers: Containers::from_labels(vec!["class".into()]),
            ..Default::default()
        };
        let kinds = profile.containers.resolve_kinds(&profile.node_labels);
        assert_eq!(kinds, vec!["class_declaration", "struct_declaration"]);
    }

    // --- lint ---------------------------------------------------------------
    //
    // Linted against the real built-in Rust grammar, so these assert against the
    // kinds and fields tree-sitter actually defines rather than a stub.

    /// Every built-in profile must be valid against the grammar it ships with.
    ///
    /// Built-in profiles never pass through `lint` — only registry installs do
    /// — so a node kind that the grammar does not define sits there unnoticed.
    /// Runtime masks it: `classify_node_type` falls back to a global table that
    /// happens to cover the right kinds, so output looks correct while the
    /// profile entry is dead weight.
    ///
    /// Found when a Rust profile was pushed through the registry's install path
    /// for the first time: four of its twenty labels named `*_statement` kinds
    /// that tree-sitter-rust spells `*_expression`.
    #[test]
    fn builtin_profiles_are_valid_against_their_own_grammars() {
        let languages = [
            Language::Rust,
            Language::Swift,
            Language::TypeScript,
            Language::Python,
            Language::Go,
            Language::Java,
            Language::CSharp,
            Language::Dart,
        ];

        let mut dead = Vec::new();
        for language in languages {
            let grammar = language.tree_sitter_language();
            let value = serde_json::to_value(language.profile()).unwrap();
            for finding in lint(&value, &grammar) {
                if finding.severity == LintSeverity::Error {
                    dead.push(format!("{language:?}: {}", finding));
                }
            }
        }

        assert!(
            dead.is_empty(),
            "built-in profiles name kinds their grammar lacks:\n  {}",
            dead.join("\n  ")
        );
    }

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

    /// The concept-form equivalent of naming a node kind the grammar doesn't
    /// define: a container label no kind in this profile carries can never
    /// match, and `containers` has no fallback, so it must be an error.
    #[test]
    fn lint_flags_a_container_label_no_kind_carries() {
        let value = serde_json::json!({
            "node_labels": { "impl_item": "impl" },
            "containers": ["impl", "class"],
        });
        let findings = lint(&value, &rust_grammar());
        let errs = errors(&findings);
        assert_eq!(errs.len(), 1, "{findings:?}");
        assert_eq!(errs[0].path, "profile.containers[1]");
        assert!(errs[0].message.contains("labelled 'class'"), "{:?}", errs[0]);
    }

    /// The legacy form still loads, so it is still linted for a kind the
    /// grammar doesn't define — and it earns a warning pointing at the new
    /// shape, so a stale manifest is visible rather than merely tolerated.
    #[test]
    fn lint_accepts_the_legacy_pair_form_with_a_warning() {
        let value = serde_json::json!({ "containers": [["impl_item", "type"]] });
        let findings = lint(&value, &rust_grammar());
        assert!(errors(&findings).is_empty(), "legacy form must still load: {findings:?}");
        assert!(
            findings
                .iter()
                .any(|f| f.severity == LintSeverity::Warning && f.message.contains("legacy")),
            "{findings:?}"
        );

        // A bogus kind in the legacy form is still an error.
        let bad = serde_json::json!({ "containers": [["impl_itme", "type"]] });
        assert_eq!(errors(&lint(&bad, &rust_grammar())).len(), 1);
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
