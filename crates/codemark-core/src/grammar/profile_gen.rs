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
use crate::parser::profile::{Containers, Profile};

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

/// Control-flow kinds matched on the *whole* kind name rather than a segment.
///
/// `case`, `until` and `loop` cannot be segment markers: each appears as a
/// segment of a kind that is not the construct it names.
///
/// | Segment | Collides with | What that kind really is |
/// |---|---|---|
/// | `case` | PHP `enum_case`, Scala `full_enum_case` | an enum member |
/// | `until` | Ada `delay_until_statement` | a timed delay |
/// | `loop` | Ada `loop_parameter_specification`, `loop_label` | a loop's header/label, not the loop |
///
/// For `case` the cost is worse than a bad label: a segment match would demote
/// the enum members out of `landmark_kinds` entirely. So the dispatch statement,
/// its branch wrapper, and the loop forms are matched on the whole kind name.
const CONTROL_FLOW_EXACT: &[&str] = &[
    "case_statement",
    "case_item",
    "switch_case",
    "case_block",
    // Negated while (Bash, Ruby), Rust's unconditional loop, Lua's
    // repeat-until, Ada's bare `loop`.
    "until",
    "until_statement",
    "until_modifier",
    "loop",
    "loop_statement",
    "loop_expression",
    "repeat_statement",
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

    // Observed once, not per kind: `case_statement` means "a branch" in grammars
    // that also have `switch_statement`, and "the whole dispatch" in those that
    // don't.
    let has_switch_statement = stats.contains_key("switch_statement");

    let mut node_labels = HashMap::new();
    for kind in stats.keys() {
        if let Some(label) = label_for(kind, landmarks.contains(kind), has_switch_statement) {
            node_labels.insert(kind.clone(), label.to_string());
        }
    }

    // Pass 2: which *concepts* the UI should treat specially.
    //
    // A container is a landmark with a body. Having a body is what makes a kind
    // worth pinning: it is something you can scroll inside, so the header stays
    // useful as the body runs off the top of the viewport. A one-liner — a C#
    // `field_declaration`, a TypeScript `method_signature` — has nothing to
    // scroll past and would only add noise.
    //
    // Deliberately *not* required: a name field. A sticky header needs only the
    // line, and the breadcrumb renderer skips a container it cannot name. Zig's
    // `struct_declaration` is the case that forces this — an anonymous value
    // bound by its enclosing `variable_declaration`, with no name of its own,
    // that you certainly want pinned while reading its methods.
    //
    // Also not required: that it was *observed* nesting another landmark. The
    // old rule needed that to learn the name field; with no field to learn, a
    // kind that can hold a body can hold a declaration, and requiring the sample
    // to prove it just means thin samples silently lose containers.
    let mut container_labels: Vec<String> = Vec::new();
    for kind in &landmarks {
        if !stats[kind].has_body {
            continue;
        }
        if let Some(label) = node_labels.get(kind)
            && !container_labels.contains(label)
        {
            container_labels.push(label.clone());
        }
    }
    container_labels.sort();
    let containers = Containers::from_labels(container_labels);

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
        containers = containers.labels().len(),
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
    if CONTROL_FLOW_EXACT.contains(&kind) {
        return true;
    }
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
///
/// `has_switch_statement` reports whether the same grammar also produced a
/// `switch_statement`; it disambiguates `case_statement`, which is a whole
/// dispatch in some grammars and a single branch in others.
fn label_for(kind: &str, landmark: bool, has_switch_statement: bool) -> Option<&'static str> {
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
    // Scala spells its package declaration `package_clause`, which is a real
    // declaration enclosing the whole file rather than a fragment of one — the
    // only `_clause` kind in the registry that is. Exempting it keeps the suffix
    // rule useful for `else_clause`, `throws_clause` and the rest.
    if kind != "package_clause" && PART_SUFFIXES.iter().any(|s| kind.ends_with(s)) {
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

    // A single branch of a dispatch construct — or the `{ }` that wraps the
    // branches — is a fragment of one, like `else_clause`: labelling it would
    // put a spurious "match" in the summary for a node that is only part of a
    // match. C# spells these `switch_body` (the braces) and `switch_section`
    // (one branch); the `switch_` segment marker below would otherwise catch
    // both and report three "match" nodes for a single switch. PHP's
    // `match_expression` decomposes further still, into four such fragments.
    //
    // Only fragment names go here. Scala's `match_type` (a type-level match)
    // and Ruby's `match_pattern` (`value => pattern`) are whole constructs and
    // are correctly labelled by the `match_` marker — checked in grammar.js for
    // both before adding anything to this list.
    if matches!(
        kind,
        "case_item"
            | "switch_case"
            | "case_block"
            | "case_pattern"
            | "switch_body"
            | "switch_section"
            | "switch_block"
            | "match_block"
            | "match_condition_list"
            | "match_conditional_expression"
            | "match_default_expression"
    ) {
        return None;
    }
    // `case_statement` is two different nodes depending on the grammar: in Bash
    // it is the whole dispatch, but in PHP and C it is a single branch of a
    // `switch_statement`. `has_switch_statement` tells them apart — a grammar
    // with both kinds is the PHP/C shape, where the dispatch is already labelled
    // via the `switch_` marker and the branch must stay unlabelled.
    if kind == "case_statement" {
        return if has_switch_statement { None } else { Some("match") };
    }
    // Loop kinds matched whole, not by segment, so Ada's
    // `delay_until_statement` and `loop_parameter_specification` — a delay and a
    // loop header, neither a loop — keep their fallback name.
    if matches!(
        kind,
        "until"
            | "until_statement"
            | "until_modifier"
            | "loop"
            | "loop_statement"
            | "loop_expression"
            | "repeat_statement"
    ) {
        return Some("while loop");
    }
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

    // Declaration words that cannot be substring markers, because the same word
    // names an expression in other grammars. Matched on the whole kind name.
    //
    // | Word | Safe kind | Would also catch |
    // |---|---|---|
    // | `object` | Scala `object_definition` | C# `anonymous_object_creation_expression`, PHP `object_creation_expression`, TS `object_pattern`/`object_type` |
    // | `record` | C# `record_declaration` | — but kept exact for symmetry |
    // | `package` | Scala `package_clause` | Scala `package_identifier` (a name, not a declaration) |
    // | `init` | Swift `init_declaration` | C# `init` (a property accessor keyword) |
    //
    // Without these four, the three grammars that use them leave container kinds
    // unlabelled — which under a concept-keyed `containers` would silently drop
    // them from the whitelist. See codemark#249.
    let exact_declarations: &[(&str, &str)] = &[
        ("object_definition", "module"),
        ("package_object", "module"),
        ("record_declaration", "class"),
        ("record_struct_declaration", "class"),
        ("package_clause", "namespace"),
        ("package_declaration", "namespace"),
        ("init_declaration", "init"),
        ("deinit_declaration", "deinit"),
        // Rust spells a module `mod_item`, which contains neither "module" nor
        // any other marker in the substring table below — so without this it is
        // a landmark with no label, and a concept-keyed `containers` would drop
        // it. Checked against all nine registry grammars: none has `mod` as a
        // whole segment, so this cannot collide.
        ("mod_item", "module"),
    ];
    for (needle, label) in exact_declarations {
        if kind == *needle {
            return Some(label);
        }
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

        // Containers are the *concepts* of landmarks that carry a body.
        let c = p.profile.containers.labels();
        assert!(c.iter().any(|l| l == "module"), "{c:?}");
        assert!(c.iter().any(|l| l == "impl"), "{c:?}");
        assert!(c.iter().any(|l| l == "function"), "{c:?}");
    }

    /// Nesting must be judged by observed shape, not by the kind's spelling.
    ///
    /// Found while seeding Ruby: its declarations are bare words — `class`,
    /// `module`, `method` — and the old check asked whether the kind *name*
    /// contained a marker like `declaration` or `_item`. `module` contains
    /// none, so it registered as nesting nothing, never became a container, and
    /// bookmarks inside it lost the outermost breadcrumb.
    ///
    /// Asserted on the predicate rather than through `propose_profile`, because
    /// no grammar available to this test suite can express the bug: every
    /// declaration kind in Rust (`mod_item`, `struct_item`) and in the Lua
    /// fixture (`function_declaration`, `variable_declaration`) happens to
    /// contain a marker word, so an end-to-end version passes with the old
    /// check restored — verified by reverting it. Ruby, which does express it,
    /// is a registry grammar and not committed here.
    #[test]
    fn nesting_is_detected_by_shape_not_by_kind_name() {
        // Ruby's declaration kinds — the ones that exposed the bug — carry none
        // of the marker words the old check looked for. Pin that, so the case
        // below is understood to be unreachable by a spelling test.
        for bare in ["module", "class", "method"] {
            let markers: Vec<_> =
                DECLARATION_MARKERS.iter().filter(|m| bare.contains(**m)).collect();
            if bare == "module" {
                assert!(markers.is_empty(), "`module` is the marker-free case: {markers:?}");
            }
        }

        // Walk a real tree and assert the predicate fires on a node chosen by
        // its *fields*, not its name. `mod_item` has a name and a body; what
        // matters is that `node_has_declaration_shape` reaches its decision
        // from those fields, which is what lets Ruby's `module` work too.
        let mut parser = Parser::new(Language::Rust).unwrap();
        let tree = parser.parse(b"mod outer { mod inner { fn f() {} } }").unwrap();
        let outer = tree.root_node().child(0).unwrap();
        assert_eq!(outer.kind(), "mod_item");
        assert!(node_has_declaration_shape(outer), "name + body must read as a declaration");
        assert!(has_declaration_descendant(outer), "outer wraps inner");

        // And end to end, it becomes a container.
        let p = propose(Language::Rust, "mod outer { mod inner { fn f() {} } }");
        let c = p.profile.containers.labels();
        assert!(c.iter().any(|l| l == "module"), "{c:?}");
    }

    /// A container needs a body, not a name.
    ///
    /// The old rule required an observed name field, which is why Zig's
    /// `struct_declaration` could never be one: a Zig struct is an anonymous
    /// value bound by its enclosing `variable_declaration`. A sticky header
    /// needs only the line, so the name is the breadcrumb renderer's problem.
    #[test]
    fn a_container_needs_a_body_not_a_name() {
        // Rust's `impl_item` holds its name under `type`, never `name` — the
        // asymmetry that made a per-kind name field error-prone in the first
        // place. As a concept it is simply "impl".
        let p = propose(Language::Rust, "impl Service {\n    fn go(&self) {}\n}\n");
        let c = p.profile.containers.labels();
        assert!(c.iter().any(|l| l == "impl"), "{c:?}");

        // A kind with a body but no name of its own still qualifies. Rust has
        // no anonymous container to test with, so this asserts the predicate
        // directly: having a body is sufficient, a name field is not required.
        let with_body_no_name =
            KindStats { has_body: true, name_fields: HashMap::new(), ..Default::default() };
        assert!(
            is_landmark("struct_declaration", &with_body_no_name),
            "a bodied declaration must be a landmark even with no name field"
        );
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
        assert_eq!(label_for("for_generic_clause", false, false), None);
        assert_eq!(label_for("else_clause", false, false), None);
        // The construct itself still gets its label.
        assert_eq!(label_for("for_statement", false, false), Some("for loop"));
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
        assert_ne!(label_for("callback_declaration", true, false), Some("call"));

        // Real control flow still matches, with or without the trailing `_`.
        assert!(is_control_flow("if_statement"));
        assert!(is_control_flow("for_statement"));
        assert!(is_control_flow("do_block"));
        assert!(is_control_flow("call_expression"));
        assert_eq!(label_for("call_expression", false, false), Some("call"));
    }

    #[test]
    fn case_is_control_flow_only_in_its_statement_form() {
        // Shell's dispatch construct and its branch wrapper are control flow.
        assert!(is_control_flow("case_statement"));
        assert!(is_control_flow("case_item"));
        assert!(is_control_flow("switch_case"));
        // Bash has no `switch_statement`, so `case_statement` is the dispatch.
        assert_eq!(label_for("case_statement", false, false), Some("match"));

        // PHP and C use `case_statement` for a *single branch* of a
        // `switch_statement`. Labelling it would put one "match" per branch in
        // the summary, so the presence of `switch_statement` suppresses it; the
        // dispatch itself is still labelled via the `switch_` marker.
        assert_eq!(label_for("case_statement", false, true), None);
        assert_eq!(label_for("switch_statement", false, true), Some("match"));

        // A single branch is a fragment of a match, not a match — same reason
        // `else_clause` gets no label.
        assert_eq!(label_for("case_item", false, false), None);
        assert_eq!(label_for("switch_case", false, false), None);

        // So is the `{ }` wrapping the branches. C# has `switch_body` and
        // `switch_section`, PHP `switch_block`, TypeScript `switch_body`: the
        // `switch_` segment marker would catch all of them and report three
        // "match" nodes for one switch. Verified against the registry's
        // grammars — none of these names is ever a declaration.
        for kind in ["switch_body", "switch_section", "switch_block"] {
            assert_eq!(label_for(kind, false, true), None, "{kind} is a fragment of a match");
        }
        // The dispatch itself is still labelled.
        assert_eq!(label_for("switch_expression", false, false), Some("match"));

        // PHP's `match_expression` decomposes into four fragments, which would
        // otherwise report five "match" nodes for one expression.
        for kind in [
            "match_block",
            "match_condition_list",
            "match_conditional_expression",
            "match_default_expression",
        ] {
            assert_eq!(label_for(kind, false, false), None, "{kind} is a fragment of a match");
        }
        assert_eq!(label_for("match_expression", false, false), Some("match"));

        // But not every `match_*` kind is a fragment. Scala's `match_type` is a
        // type-level match and Ruby's `match_pattern` is a `value => pattern`
        // expression; both are whole constructs that should keep the label.
        assert_eq!(label_for("match_type", false, false), Some("match"));
        assert_eq!(label_for("match_pattern", false, false), Some("match"));

        // The collision this exists to prevent: PHP's `enum_case` and Scala's
        // `full_enum_case` are real enum-member declarations. A segment-level
        // `case` marker would demote them out of `landmark_kinds` and label
        // them "match".
        for kind in ["enum_case", "full_enum_case", "simple_enum_case"] {
            assert!(!is_control_flow(kind), "{kind} is a declaration, not control flow");
            assert_ne!(label_for(kind, true, false), Some("match"), "{kind} mislabelled");
        }
    }

    #[test]
    fn declaration_words_that_also_name_expressions_match_whole_kinds() {
        // The four that were leaving container kinds unlabelled (codemark#249).
        assert_eq!(label_for("object_definition", true, false), Some("module"));
        assert_eq!(label_for("record_declaration", true, false), Some("class"));
        assert_eq!(label_for("package_clause", true, false), Some("namespace"));
        assert_eq!(label_for("init_declaration", true, false), Some("init"));

        // The collision this guards against: `object` as a substring marker
        // would label every object literal and pattern a "module".
        for kind in [
            "anonymous_object_creation_expression",
            "implicit_object_creation_expression",
            "object_creation_expression",
            "object_pattern",
            "object_type",
        ] {
            assert_ne!(label_for(kind, true, false), Some("module"), "{kind} mislabelled");
        }

        // Scala's `package_identifier` is the name inside a package clause, not
        // a declaration; C#'s bare `init` is a property accessor keyword.
        assert_ne!(label_for("package_identifier", true, false), Some("namespace"));
        assert_ne!(label_for("init", true, false), Some("init"));
    }

    #[test]
    fn until_and_loop_are_while_loops() {
        // Bash's and Ruby's negated while, Rust's unconditional loop, and
        // Lua's repeat-until.
        assert!(is_control_flow("until_statement"));
        assert_eq!(label_for("until_statement", false, false), Some("while loop"));
        assert_eq!(label_for("until_modifier", false, false), Some("while loop"));
        assert_eq!(label_for("until", false, false), Some("while loop"));
        assert_eq!(label_for("loop_statement", false, false), Some("while loop"));
        assert_eq!(label_for("loop_expression", false, false), Some("while loop"));
        assert_eq!(label_for("repeat_statement", false, false), Some("while loop"));
    }

    #[test]
    fn until_and_loop_match_whole_kinds_not_segments() {
        // The collision that forces whole-kind matching. Ada's
        // `delay_until_statement` is a timed delay and
        // `loop_parameter_specification` is a loop's header — neither is a
        // loop, so neither may be called one.
        for kind in ["delay_until_statement", "loop_parameter_specification"] {
            assert!(!is_control_flow(kind), "{kind} is not control flow");
            assert_eq!(label_for(kind, false, false), None, "{kind} mislabelled as a loop");
        }

        // `loop_label` is control flow, but on the strength of its `label`
        // segment, not `loop` — so it must not be called a loop either.
        assert_ne!(label_for("loop_label", false, false), Some("while loop"));
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

        // The committed fixture is deliberately left in the legacy
        // `[kind, field]` form — it is the only committed artifact proving an
        // older manifest still loads, so migrating it would delete the
        // regression test.
        //
        // The two therefore cannot be compared directly: the generator now
        // proposes a *concept* that covers every kind carrying that label,
        // while the fixture names one kind. Lua labels both
        // `function_declaration` and `function_definition` as "function", so
        // the concept resolves to a superset. What must hold is that nothing
        // the fixture names has been lost.
        let got_c: BTreeSet<String> = proposal
            .profile
            .containers
            .resolve_kinds(&proposal.profile.node_labels)
            .into_iter()
            .collect();
        let want_c: BTreeSet<String> =
            committed.containers.resolve_kinds(&committed.node_labels).into_iter().collect();
        assert!(
            want_c.is_subset(&got_c),
            "the proposal dropped a container the committed profile has: want {want_c:?} \
             within got {got_c:?}"
        );
        // And specifically: the anonymous-function kind is now included, which
        // the old name-field rule excluded for having no name of its own.
        assert!(
            got_c.contains("function_definition"),
            "an anonymous function has a body, so it should be a container: {got_c:?}"
        );

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
