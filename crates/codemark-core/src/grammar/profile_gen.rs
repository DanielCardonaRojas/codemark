//! Proposing a [`Profile`] by observing a grammar's AST over real source.
//!
//! Hand-authoring a profile means reading a raw AST dump and guessing which
//! node kinds matter. The kinds are grammar-specific (`function_item` in Rust,
//! `method` in Ruby, `function_declaration` in Lua), and a wrong guess fails
//! silently — which is why [`crate::parser::profile::lint`] exists. This module
//! removes the guessing step.
//!
//! **Why sample files rather than the grammar's node-type list.** A grammar
//! enumerates every kind it can ever produce, with no signal about which are
//! structurally interesting. `tree-sitter-lua` defines 47 named kinds; three
//! belong in `landmark_kinds`. Parsing real source tells us which kinds actually
//! carry declarations, how often, and — critically for `containers` — which ones
//! nest other declarations and which child field holds their name. That last
//! fact can only be observed; assuming `"name"` is how the committed Lua fixture
//! ended up pointing `containers` at a kind that does not exist.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use tree_sitter::Node;

use crate::error::{Error, Result};
use crate::parser::languages::{Language, Parser};
use crate::parser::profile::Profile;

/// Field names that typically hold a declaration's identifier. Checked in order,
/// so an explicit `name` wins over a positional fallback.
const NAME_FIELDS: &[&str] = &["name", "declarator", "key", "alias", "type", "field", "path"];

/// Field names that typically hold a declaration's nested body. A node with one
/// of these can contain other declarations, which is what makes it a candidate
/// container.
const BODY_FIELDS: &[&str] = &["body", "block", "consequence", "definition"];

/// Kind-name fragments that mark a node as a declaration even when the grammar
/// gives it no name field — an anonymous function expression, for instance.
/// Deliberately narrow: these are the words grammars converge on.
const DECLARATION_MARKERS: &[&str] =
    &["declaration", "definition", "_item", "signature", "method", "class", "struct", "function"];

/// Kinds that are statements/expressions rather than declarations. They often
/// carry a body (an `if` has `consequence`), so they'd otherwise look like
/// landmarks. They still earn a `node_labels` entry.
const CONTROL_FLOW_MARKERS: &[&str] = &[
    "if_",
    "for_",
    "while_",
    "switch_",
    "match_",
    "return_",
    "assignment",
    "call",
    "loop_",
    "try_",
    "catch_",
    "repeat_",
    "do_",
    "else",
    "elseif",
    "break",
    "goto",
    "label",
];

/// Why the generator classified a kind the way it did, so a human can audit the
/// proposal instead of taking it on faith.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KindEvidence {
    pub kind: String,
    /// How many times this kind appeared across the samples.
    pub occurrences: usize,
    /// Whether it was proposed as a landmark.
    pub landmark: bool,
    /// The field observed holding its name, if any.
    pub name_field: Option<String>,
    /// Whether it was observed containing another landmark.
    pub nests_landmarks: bool,
    /// Plain-language reason, shown in the evidence table.
    pub reason: String,
}

/// A proposed profile plus the evidence behind each decision.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileProposal {
    pub profile: Profile,
    /// Ranked by occurrence count, most frequent first.
    pub evidence: Vec<KindEvidence>,
    /// Sample files that parsed with errors. A grammar/source mismatch makes the
    /// proposal unreliable, so the caller should surface this.
    pub samples_with_errors: Vec<String>,
}

/// Observations accumulated for one node kind across all samples.
#[derive(Default)]
struct KindStats {
    occurrences: usize,
    /// Field name → times it held this node's name child.
    name_fields: HashMap<String, usize>,
    /// A name was found below this node rather than on it (see [`collect`]).
    /// Enough to make it a landmark, never enough to make it a container.
    has_nested_name: bool,
    has_body: bool,
    /// Observed as a strict ancestor of another landmark candidate.
    nests_landmarks: bool,
}

/// Parse `samples` with `language` and propose a [`Profile`] from what the trees
/// actually contain.
///
/// `samples` is `(display_name, source)`; the name is only used to report parse
/// errors back to the caller.
pub fn propose_profile(
    language: &Language,
    samples: &[(String, String)],
) -> Result<ProfileProposal> {
    if samples.is_empty() {
        return Err(Error::Input(
            "no sample files given — the profile is derived from parsed source, so at least one \
             sample is required"
                .to_string(),
        ));
    }

    let mut parser = Parser::new(language.clone())?;
    let mut stats: BTreeMap<String, KindStats> = BTreeMap::new();
    let mut samples_with_errors = Vec::new();

    for (name, source) in samples {
        let tree = parser.parse(source.as_bytes())?;
        if tree.root_node().has_error() {
            samples_with_errors.push(name.clone());
        }
        collect(tree.root_node(), &mut stats);
    }

    if stats.is_empty() {
        return Err(Error::Input(
            "the samples produced no named AST nodes — check that they are source files for this \
             grammar"
                .to_string(),
        ));
    }

    if !samples_with_errors.is_empty() {
        // A proposal built on a broken parse is unreliable, so make it visible
        // in the log as well as in the returned value.
        tracing::warn!(
            target: "codemark::languages",
            count = samples_with_errors.len(),
            "some samples did not parse cleanly; the proposal may be incomplete"
        );
    }

    // Pass 1: decide landmarks from each kind's own shape.
    let landmarks: BTreeSet<String> =
        stats.iter().filter(|(k, s)| is_landmark(k, s)).map(|(k, _)| k.clone()).collect();

    // Pass 2: a container is a landmark observed wrapping another landmark, and
    // only if we actually saw which field held its name — a container whose name
    // field is guessed is the exact mistake this module exists to prevent.
    let mut containers = Vec::new();
    for kind in &landmarks {
        let s = &stats[kind];
        if !s.nests_landmarks {
            continue;
        }
        if let Some(field) = dominant_name_field(s) {
            containers.push((kind.clone(), field));
        }
    }

    let mut node_labels = HashMap::new();
    for kind in stats.keys() {
        if let Some(label) = label_for(kind, landmarks.contains(kind)) {
            node_labels.insert(kind.clone(), label.to_string());
        }
    }

    let mut evidence: Vec<KindEvidence> = stats
        .iter()
        .map(|(kind, s)| {
            let landmark = landmarks.contains(kind);
            KindEvidence {
                kind: kind.clone(),
                occurrences: s.occurrences,
                landmark,
                name_field: dominant_name_field(s),
                nests_landmarks: s.nests_landmarks,
                reason: reason_for(kind, s, landmark),
            }
        })
        .collect();
    // Most frequent first; ties broken by name so the output is deterministic.
    evidence.sort_by(|a, b| b.occurrences.cmp(&a.occurrences).then(a.kind.cmp(&b.kind)));

    tracing::debug!(
        target: "codemark::languages",
        language = %language.name(),
        samples = samples.len(),
        kinds_seen = stats.len(),
        landmarks = landmarks.len(),
        containers = containers.len(),
        "proposed a grammar profile"
    );

    Ok(ProfileProposal {
        profile: Profile {
            landmark_kinds: landmarks.into_iter().collect(),
            node_labels,
            containers,
        },
        evidence,
        samples_with_errors,
    })
}

/// Walk the tree, recording each named node's shape and whether it wraps another
/// declaration candidate.
fn collect(node: Node, stats: &mut BTreeMap<String, KindStats>) {
    if node.is_named() {
        let kind = node.kind().to_string();
        let entry = stats.entry(kind.clone()).or_default();
        entry.occurrences += 1;

        for i in 0..node.child_count() {
            let Some(field) = node.field_name_for_child(i as u32) else { continue };
            if NAME_FIELDS.contains(&field) {
                *entry.name_fields.entry(field.to_string()).or_insert(0) += 1;
            }
            if BODY_FIELDS.contains(&field) {
                entry.has_body = true;
            }
        }

        // Some grammars don't put the name on the declaration itself: Lua's
        // `variable_declaration` wraps `assignment_statement > variable_list`,
        // and only the innermost node carries `name`. Such a declaration is
        // still a landmark, so look a short way down through *unfielded*
        // children for one. Bounded to two levels, and it only ever sets
        // `has_nested_name` — never `name_fields`, because a container's name
        // field must be a field on the container itself for breadcrumbs to
        // read it.
        if entry.name_fields.is_empty() && nested_name_within(node, 2) {
            entry.has_nested_name = true;
        }

        // Does this node wrap another declaration? Judged by observed *shape*
        // (a name and a body) rather than by the kind's name, because plenty of
        // grammars name declarations with no telltale word in them — Ruby's
        // `module` and `class` are bare. A name-based test made `module` look
        // like it nested nothing, so it never became a container.
        //
        // The final landmark set isn't known until every sample is walked, so
        // this uses the same per-node test `is_landmark` applies later.
        if node_has_declaration_shape(node) && has_declaration_descendant(node) {
            entry.nests_landmarks = true;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, stats);
    }
}

/// Whether a name field appears within `depth` levels below `node`, descending
/// only through children the parent gave no field name — i.e. structural
/// wrappers like Lua's `assignment_statement`/`variable_list`. Stopping at
/// fielded children keeps this from reaching into a nested *declaration's* own
/// name and mislabelling the outer node.
fn nested_name_within(node: Node, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if !child.is_named() || node.field_name_for_child(i as u32).is_some() {
            continue;
        }
        for j in 0..child.child_count() {
            if child.field_name_for_child(j as u32).is_some_and(|f| NAME_FIELDS.contains(&f)) {
                return true;
            }
        }
        if nested_name_within(child, depth - 1) {
            return true;
        }
    }
    false
}

/// Whether any *strict* descendant looks like a declaration.
fn has_declaration_descendant(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && node_has_declaration_shape(child) {
            return true;
        }
        if has_declaration_descendant(child) {
            return true;
        }
    }
    false
}

/// Whether this *node* has the shape of a declaration: a name field and a body.
///
/// Shape rather than kind name, because grammars don't agree on naming — Ruby
/// calls them `class`/`module`/`method`, Rust `*_item`, Lua `*_declaration`.
/// A kind whose name carries a declaration word counts too, so an anonymous
/// `function_definition` with a body still registers.
///
/// Mirrors the per-node half of [`is_landmark`]; that one additionally consults
/// accumulated stats, which aren't available mid-walk.
fn node_has_declaration_shape(node: Node) -> bool {
    let kind = node.kind();
    if is_control_flow(kind) {
        return false;
    }
    let mut has_name = false;
    let mut has_body = false;
    for i in 0..node.child_count() {
        let Some(field) = node.field_name_for_child(i as u32) else { continue };
        if NAME_FIELDS.contains(&field) {
            has_name = true;
        }
        if BODY_FIELDS.contains(&field) {
            has_body = true;
        }
    }
    if has_name && has_body {
        return true;
    }
    // Mirrors `is_landmark`'s remaining tiers: a declaration-shaped kind with a
    // body but no name of its own (an anonymous function expression), with a
    // name but no body (a type alias, a const), or with its name nested in a
    // wrapper (Lua's `variable_declaration`, whose name sits under
    // `assignment_statement > variable_list`).
    let declaration_word = DECLARATION_MARKERS.iter().any(|m| kind.contains(m));
    declaration_word && (has_body || has_name || nested_name_within(node, 2))
}

/// Whether `kind` names a statement/expression rather than a declaration.
///
/// Matched on whole underscore-separated segments, not substrings: a plain
/// `contains` makes `callback_declaration` match `call` and
/// `entry_declaration` match `try_`, which would drop real declarations from
/// `landmark_kinds` and label them as control flow.
fn is_control_flow(kind: &str) -> bool {
    let segments: Vec<&str> = kind.split('_').filter(|s| !s.is_empty()).collect();
    CONTROL_FLOW_MARKERS.iter().any(|m| {
        // Markers are written with a trailing `_` where they prefix a
        // construct (`if_`, `for_`); compare on the bare word either way.
        let word = m.trim_end_matches('_');
        segments.contains(&word)
    })
}

/// A landmark is a node that introduces a named or callable construct.
///
/// Two tiers, because grammars model declarations differently:
///
/// 1. It has both a name field and a body (`function_declaration`) — the clean
///    case.
/// 2. It has a body and its kind reads as a declaration, even with no name field
///    (`function_definition`, an anonymous function expression whose name lives
///    on the enclosing assignment). Dropping these would lose real declarations.
///
/// Control-flow kinds are excluded: an `if_statement` has a `consequence` body,
/// so the body test alone would wrongly admit it.
fn is_landmark(kind: &str, s: &KindStats) -> bool {
    if is_control_flow(kind) {
        return false;
    }
    let named = !s.name_fields.is_empty();
    if named && s.has_body {
        return true;
    }
    if s.has_body && DECLARATION_MARKERS.iter().any(|m| kind.contains(m)) {
        return true;
    }
    // A declaration-shaped kind that holds a name but no body: a type alias, a
    // const. Real landmarks, and they anchor queries the same way.
    if named && DECLARATION_MARKERS.iter().any(|m| kind.contains(m)) {
        return true;
    }
    // Declaration-shaped, with its name nested in a wrapper rather than on
    // itself (Lua's `variable_declaration`, and the `let`/`const` shape several
    // grammars use). A landmark, but it can't be a container — `containers`
    // needs a field the breadcrumb walker can read off the node directly.
    s.has_nested_name && DECLARATION_MARKERS.iter().any(|m| kind.contains(m))
}

/// The field most often observed holding this kind's name, preferring the
/// conventional ordering in [`NAME_FIELDS`] when counts tie.
fn dominant_name_field(s: &KindStats) -> Option<String> {
    s.name_fields
        .iter()
        .max_by_key(|(field, count)| {
            let rank = NAME_FIELDS.iter().position(|f| f == field).unwrap_or(usize::MAX);
            (**count, std::cmp::Reverse(rank))
        })
        .map(|(field, _)| field.clone())
}

/// Map a node kind to a label from the vocabulary the built-in profiles use.
///
/// Deliberately constrained to that vocabulary: `classifier::get_node_icon` keys
/// off the label, so an invented label parses but loses its icon — which the
/// lint reports as a warning. Returning `None` leaves the kind out of
/// `node_labels` entirely, where the caller's fallback
/// (`kind.replace('_', " ")`) applies.
fn label_for(kind: &str, landmark: bool) -> Option<&'static str> {
    // Control flow and common expressions first: these are the labels that make
    // query summaries readable, and they are not landmarks.
    // A sub-part of a construct (`for_generic_clause`, `else_clause`) is not the
    // construct itself; labelling it would put a spurious "for loop" in query
    // summaries for a node that is only part of one.
    //
    // Deliberately narrow: `_item` and `_list` are *not* here, because Rust's
    // declarations are `function_item`/`struct_item` and some grammars name a
    // declaration `..._list`. Only suffixes that always mean "a fragment of an
    // enclosing construct" qualify.
    const PART_SUFFIXES: &[&str] = &["_clause", "_content", "_part"];
    if PART_SUFFIXES.iter().any(|s| kind.ends_with(s)) {
        return None;
    }

    let control: &[(&str, &str)] = &[
        ("if_", "if statement"),
        ("elseif", "if statement"),
        ("for_", "for loop"),
        ("while_", "while loop"),
        ("repeat_", "while loop"),
        ("switch_", "match"),
        ("match_", "match"),
        ("return_", "return"),
        ("assignment", "assignment"),
    ];
    // Whole-segment matching, same reason as `is_control_flow`: a substring
    // test labels `callback_declaration` a "call".
    let segments: Vec<&str> = kind.split('_').filter(|s| !s.is_empty()).collect();
    for (needle, label) in control {
        if segments.contains(&needle.trim_end_matches('_')) {
            return Some(label);
        }
    }
    if segments.contains(&"call") {
        return Some("call");
    }

    if !landmark {
        return None;
    }

    // Landmark labels, most specific first so `method_declaration` doesn't match
    // the broader `declaration` rules.
    let declarations: &[(&str, &str)] = &[
        ("constructor", "constructor"),
        ("method", "method"),
        ("interface", "interface"),
        ("protocol", "interface"),
        ("trait", "interface"),
        ("namespace", "namespace"),
        ("module", "module"),
        ("macro", "macro"),
        ("enum", "enum"),
        ("struct", "class"),
        ("class", "class"),
        ("impl", "impl"),
        ("function", "function"),
        ("type", "type"),
        ("variable", "variable"),
        ("const", "variable"),
        ("field", "variable"),
        ("property", "variable"),
    ];
    for (needle, label) in declarations {
        if kind.contains(needle) {
            return Some(label);
        }
    }
    None
}

fn reason_for(kind: &str, s: &KindStats, landmark: bool) -> String {
    if is_control_flow(kind) {
        return "control flow or expression — labelled but not a landmark".to_string();
    }
    if !landmark {
        return "no body or name field observed".to_string();
    }
    let name = match dominant_name_field(s) {
        Some(f) => format!("name in field '{f}'"),
        // Distinguish "the name is nested in a wrapper" from "there is no name
        // at all" — they look the same in the Name field column, but only the
        // latter is an anonymous definition.
        None if s.has_nested_name => "name nested below (not a container)".to_string(),
        None => "no name field (anonymous definition)".to_string(),
    };
    let body = if s.has_body { ", has a body" } else { "" };
    let nests = if s.nests_landmarks { ", nests other declarations" } else { "" };
    format!("{name}{body}{nests}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn propose(lang: Language, src: &str) -> ProfileProposal {
        propose_profile(&lang, &[("sample".to_string(), src.to_string())]).unwrap()
    }

    #[test]
    fn requires_at_least_one_sample() {
        assert!(propose_profile(&Language::Rust, &[]).is_err());
    }

    // The generator is derived from observed structure, so the built-in grammars
    // are the cheapest ground truth: we already know the right answer for them.
    #[test]
    fn proposes_rust_landmarks_and_containers() {
        let src = r#"
mod auth {
    pub struct User { name: String }
    impl User {
        fn validate(&self) -> bool {
            if self.name.is_empty() { return false; }
            true
        }
    }
    trait Check { fn run(&self); }
}
"#;
        let p = propose(Language::Rust, src);
        let lm = &p.profile.landmark_kinds;
        for expected in ["function_item", "struct_item", "impl_item", "mod_item", "trait_item"] {
            assert!(lm.iter().any(|k| k == expected), "missing {expected} in {lm:?}");
        }
        // Control flow must not be a landmark even though `if` carries a body.
        assert!(!lm.iter().any(|k| k == "if_expression"), "{lm:?}");

        // Containers are the kinds observed wrapping other declarations.
        let names: Vec<&str> = p.profile.containers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"mod_item"), "{:?}", p.profile.containers);
        assert!(names.contains(&"impl_item"), "{:?}", p.profile.containers);
    }

    #[test]
    fn nesting_is_detected_by_shape_not_by_kind_name() {
        // Found while seeding Ruby: its declarations are bare words (`class`,
        // `module`, `method`), so a kind-name test saw `module` as nesting
        // nothing and never proposed it as a container. Rust's `mod_item`
        // carries a marker word, so this stands in for the same shape — what
        // must drive the decision is the observed name+body, not the spelling.
        let src = "mod outer { mod inner { fn f() {} } }";
        let p = propose(Language::Rust, src);
        let names: Vec<&str> = p.profile.containers.iter().map(|(k, _)| k.as_str()).collect();
        assert!(names.contains(&"mod_item"), "{:?}", p.profile.containers);
    }

    #[test]
    fn container_name_field_is_observed_not_assumed() {
        // Rust's `impl_item` holds its name in the `type` field, not `name`.
        // Assuming "name" here is precisely the bug the committed Lua fixture
        // shipped, so this asserts the field comes from the parse.
        let src = r#"
impl Service {
    fn go(&self) {}
}
"#;
        let p = propose(Language::Rust, src);
        let field =
            p.profile.containers.iter().find(|(k, _)| k == "impl_item").map(|(_, f)| f.as_str());
        assert_eq!(field, Some("type"), "{:?}", p.profile.containers);
    }

    #[test]
    fn labels_come_from_the_builtin_vocabulary() {
        let src = r#"
struct S { x: i32 }
fn f() {
    if true { return; }
    g();
}
"#;
        let p = propose(Language::Rust, src);
        let labels = &p.profile.node_labels;
        assert_eq!(labels.get("function_item").map(String::as_str), Some("function"));
        assert_eq!(labels.get("struct_item").map(String::as_str), Some("class"));
        assert_eq!(labels.get("if_expression").map(String::as_str), Some("if statement"));
        assert_eq!(labels.get("call_expression").map(String::as_str), Some("call"));
    }

    #[test]
    fn sub_parts_of_a_construct_are_not_labelled_as_the_construct() {
        // `for_generic_clause` is part of a for loop, not a for loop. Labelling
        // it would put a spurious "for loop" in query summaries for a node that
        // only ever appears inside one.
        assert_eq!(label_for("for_generic_clause", false), None);
        assert_eq!(label_for("else_clause", false), None);
        // The construct itself still gets its label.
        assert_eq!(label_for("for_statement", false), Some("for loop"));
    }

    #[test]
    fn control_flow_matches_whole_segments_not_substrings() {
        // `callback_declaration` contains "call" and `entry_declaration`
        // contains "try_", but both are declarations. Substring matching would
        // drop them from landmarks and mislabel them.
        assert!(!is_control_flow("callback_declaration"));
        assert!(!is_control_flow("entry_declaration"));
        assert!(!is_control_flow("pseudo_class_selector"));
        // The point is that it is no longer labelled "call"; it has no
        // declaration keyword either, so it gets no label rather than a wrong one.
        assert_ne!(label_for("callback_declaration", true), Some("call"));

        // Real control flow still matches, with or without the trailing `_`.
        assert!(is_control_flow("if_statement"));
        assert!(is_control_flow("for_statement"));
        assert!(is_control_flow("do_block"));
        assert!(is_control_flow("call_expression"));
        assert_eq!(label_for("call_expression", false), Some("call"));
    }

    #[test]
    fn evidence_is_ranked_and_explains_each_kind() {
        let src = "fn a() {}\nfn b() {}\nstruct S;\n";
        let p = propose(Language::Rust, src);
        // Ranked by occurrence, descending.
        for w in p.evidence.windows(2) {
            assert!(w[0].occurrences >= w[1].occurrences, "{:?}", p.evidence);
        }
        let f = p.evidence.iter().find(|e| e.kind == "function_item").unwrap();
        assert_eq!(f.occurrences, 2);
        assert!(f.landmark);
        assert!(!f.reason.is_empty());
    }

    #[test]
    fn reports_samples_that_failed_to_parse() {
        // Python source fed to the Rust grammar produces ERROR nodes; the caller
        // needs to know the proposal is built on a bad parse.
        let p = propose(Language::Rust, "def f(:\n  ???");
        assert_eq!(p.samples_with_errors, vec!["sample".to_string()]);
    }

    #[test]
    fn proposal_is_deterministic() {
        let src = "mod m { fn a() {} struct S; }";
        let a = propose(Language::Rust, src);
        let b = propose(Language::Rust, src);
        assert_eq!(a.profile.landmark_kinds, b.profile.landmark_kinds);
        assert_eq!(a.profile.containers, b.profile.containers);
    }

    /// The Phase 1 regression harness: the generator run against the committed
    /// Lua grammar must reproduce the committed profile.
    ///
    /// This is the gate that matters, because Lua is a *dynamic* grammar — the
    /// path a real user takes — and because the committed fixture was itself
    /// corrected against the grammar, so it is known-good ground truth rather
    /// than a restatement of the generator's own heuristics.
    ///
    /// Skipped (not failed) when the fixture is absent, matching the other
    /// grammar-fixture tests, so a checkout without the wasm blob still passes.
    #[cfg(feature = "wasm")]
    #[test]
    fn reproduces_the_committed_lua_profile() {
        use crate::parser::languages::DynamicLanguage;
        use std::sync::Arc;

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/grammars/lua");
        let wasm_path = fixture.join("grammar.wasm");
        if !wasm_path.exists() {
            eprintln!("skipping: lua grammar fixture not found");
            return;
        }

        let language = Language::Dynamic(Arc::new(DynamicLanguage {
            name: "lua".into(),
            extensions: vec!["lua".into()],
            wasm_path,
            profile: Profile::default(),
        }));

        let source = std::fs::read_to_string(fixture.join("sample.lua")).unwrap();
        let proposal = propose_profile(&language, &[("sample.lua".to_string(), source)]).unwrap();
        assert!(proposal.samples_with_errors.is_empty(), "sample must parse cleanly");

        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture.join("manifest.json")).unwrap())
                .unwrap();
        let committed: Profile =
            serde_json::from_value(manifest.get("profile").unwrap().clone()).unwrap();

        // Compare as sets: the generator's ordering is its own business, the
        // membership is the claim.
        let got: BTreeSet<&str> =
            proposal.profile.landmark_kinds.iter().map(String::as_str).collect();
        let want: BTreeSet<&str> = committed.landmark_kinds.iter().map(String::as_str).collect();
        assert_eq!(got, want, "landmark_kinds differ from the committed profile");

        let got_c: BTreeSet<(&str, &str)> =
            proposal.profile.containers.iter().map(|(k, f)| (k.as_str(), f.as_str())).collect();
        let want_c: BTreeSet<(&str, &str)> =
            committed.containers.iter().map(|(k, f)| (k.as_str(), f.as_str())).collect();
        assert_eq!(got_c, want_c, "containers differ from the committed profile");

        // Labels are the generator's third output and feed `@sticky` captures
        // and icon selection, so leaving them unchecked would let query labels
        // drift from the curated fixture while this test stayed green.
        let got_l: BTreeSet<(&str, &str)> =
            proposal.profile.node_labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let want_l: BTreeSet<(&str, &str)> =
            committed.node_labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(got_l, want_l, "node_labels differ from the committed profile");
    }

    // A generated profile must satisfy the Phase 0 lint — otherwise the
    // generator would emit exactly the dead entries the lint exists to catch.
    #[test]
    fn generated_profile_is_lint_clean() {
        use crate::parser::profile::{LintSeverity, lint};
        let src = r#"
mod auth {
    struct User;
    impl User { fn go(&self) { if true { return; } } }
}
"#;
        let p = propose(Language::Rust, src);
        let value = serde_json::to_value(&p.profile).unwrap();
        let findings = lint(&value, &Language::Rust.tree_sitter_language());
        let errors: Vec<_> =
            findings.iter().filter(|f| f.severity == LintSeverity::Error).collect();
        assert!(errors.is_empty(), "generated profile has lint errors: {errors:?}");
    }
}
