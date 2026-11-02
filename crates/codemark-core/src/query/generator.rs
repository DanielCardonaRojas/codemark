use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter::{Language, Node, Tree};

use crate::error::{Error, Result};
use crate::parser::profile::Profile;
use crate::query::matcher;

/// A generated tree-sitter query with metadata about the target.
#[derive(Debug)]
pub struct GeneratedQuery {
    pub query: String,
    pub target_node_type: String,
    pub target_name: Option<String>,
    pub byte_range: (usize, usize),
}

/// Context information passed to query strategies.
#[derive(Clone, Debug)]
pub struct QueryContext<'a> {
    pub source: &'a [u8],
    pub language: &'a Language,
    /// Structural metadata for the language being queried. Drives landmark
    /// anchoring so dynamic (WASM) grammars honor their manifest `Profile`.
    pub profile: &'a Profile,
    pub byte_range: (usize, usize),
    pub root: Node<'a>,
    pub tree: &'a Tree,
}

/// Semantic information that can distinguish a node from others of the same type.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum SemanticInfo {
    /// For if statements: the condition being tested.
    IfCondition(String),
    /// For call expressions: the function being called.
    CallTarget(String),
    /// For assignments: the variable being assigned.
    AssignmentTarget(String),
    /// For return statements: the value being returned.
    ReturnValue(String),
    /// For binary expressions: the operator.
    BinaryOperator(String),
}

/// One entry in the structural path from root to target.
#[derive(Debug, Clone)]
struct PathEntry {
    node_type: String,
    /// Name info for query generation.
    name_info: Option<NameInfo>,
    /// Semantic info for query generation.
    semantic_info: Option<SemanticInfo>,
    /// Whether this node is a "landmark" (stable named declaration).
    is_landmark: bool,
    /// Sticky breadcrumb tag (e.g., Some("class"), Some("method")) for @sticky captures.
    sticky_tag: Option<String>,
    /// Whether the name node sits *after* the child this path descends into.
    ///
    /// Sibling patterns in a tree-sitter query are an ordered sequence, so they
    /// must appear in the order the grammar produces them. Rust puts the
    /// visibility modifier before the name:
    ///
    /// ```text
    /// struct_item [ -=visibility_modifier  -=struct  name=type_identifier  body=... ]
    /// ```
    ///
    /// so `(struct_item name: (type_identifier) @n (visibility_modifier) @t)` is
    /// an "Impossible pattern" while the reverse order compiles. Emitting the
    /// name first unconditionally made every such bookmark unreachable.
    name_after_child: bool,
}

/// One nesting level on the way down to a name, with the field it occupies.
#[derive(Debug, Clone)]
struct NameHop {
    /// The field this node fills in its parent, when the parent declares one.
    field: Option<String>,
    kind: String,
}

impl NameHop {
    /// Positional — the parent gives this child no field of its own.
    fn positional(kind: impl Into<String>) -> Self {
        Self { field: None, kind: kind.into() }
    }
}

/// How to query for the "name" of a node.
#[derive(Debug, Clone)]
struct NameInfo {
    /// The field name used in the parent (usually "name", but "type" for Rust impl_item)
    /// If None, the name is matched as a descendant without a specific field.
    field: Option<String>,
    /// The direct name node type (e.g., "simple_identifier" or "user_type")
    direct_type: String,
    /// The hops nested inside `direct_type` on the way to the name, outermost
    /// first — `user_type > type_identifier` is one fieldless `type_identifier`.
    ///
    /// Usually one level, but a wrapper can sit two deep: TS
    /// `export_statement > lexical_declaration > variable_declarator >
    /// identifier` needs two hops, because capturing the declarator would
    /// compare `Foo = 1` against `Foo` and match nothing.
    ///
    /// Each hop keeps the field it occupies in *its own* parent, so the
    /// identifier under a `variable_declarator` renders as `name: (identifier)`.
    /// A bare positional `(identifier)` would also bind the declarator's
    /// `value`, letting a query for `export const Foo = 1` match
    /// `export const Bar = Foo`.
    inner_types: Vec<NameHop>,
    /// The text value to match
    text: String,
}

/// Given a parsed tree and a byte range, generate a tree-sitter query that uniquely
/// identifies the target node.
pub fn generate_query(
    tree: &Tree,
    source: &[u8],
    byte_range: (usize, usize),
    language: &Language,
    profile: &Profile,
) -> Result<GeneratedQuery> {
    let ctx = QueryContext { source, language, profile, byte_range, root: tree.root_node(), tree };

    // 1. Select the tightest meaningful target node
    let mut node = find_tightest_node(&ctx.root, ctx.source, ctx.byte_range)?;

    // For fine-grained targeting, we want to descend into blocks to find the
    // actual statement or expression.
    while is_body_node(node.kind()) {
        let mut found_child = false;
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if child.start_byte() < ctx.byte_range.1 && child.end_byte() > ctx.byte_range.0 {
                node = child;
                found_child = true;
                break;
            }
        }

        if !found_child {
            break;
        }

        // If we found a node with semantic info, stop here
        if extract_semantic_info(node, ctx.source).is_some() {
            break;
        }
    }

    // 2. Extract metadata
    let name = extract_name_info(node, ctx.source)
        .map(|info| info.text)
        .or_else(|| extract_identifier_from_node(node, ctx.source));

    // 3. Disambiguate and anchor
    let query = disambiguate_query(node, &ctx)?;

    Ok(GeneratedQuery {
        query,
        target_node_type: node.kind().to_string(),
        target_name: name,
        byte_range: (node.start_byte(), node.end_byte()),
    })
}

/// Find the tightest meaningful node for a given byte range.
/// If it's a point range, find the deepest named node.
/// If it's a multi-byte range, find the smallest node covering it.
fn find_tightest_node<'a>(
    root: &Node<'a>,
    source: &[u8],
    byte_range: (usize, usize),
) -> Result<Node<'a>> {
    let mut start = byte_range.0;
    let mut end = byte_range.1;

    // Trim whitespace
    while start < end && (source[start] as char).is_whitespace() {
        start += 1;
    }
    while end > start && (source[end - 1] as char).is_whitespace() {
        end -= 1;
    }

    if start == end {
        // Point range: pick the deepest named node at this position
        let mut node = root
            .descendant_for_byte_range(start, start)
            .ok_or_else(|| Error::TreeSitter("no node found at position".into()))?;

        // If the node is very large (e.g. it includes trailing whitespace),
        // try to find a tighter child that also contains the point.
        while let Some(tighter_child) = find_tighter_child(node, start) {
            node = tighter_child;
        }

        // Walk up to the nearest named node if we hit an anonymous one
        while !node.is_named() {
            if let Some(parent) = node.parent() {
                node = parent;
            } else {
                break;
            }
        }
        Ok(node)
    } else {
        // Range: pick the smallest node covering it
        let mut node = root
            .descendant_for_byte_range(start, end)
            .ok_or_else(|| Error::TreeSitter("no node found at byte range".into()))?;

        // Walk up to the nearest named node if we hit an anonymous one
        while !node.is_named() {
            if let Some(parent) = node.parent() {
                node = parent;
            } else {
                break;
            }
        }
        Ok(node)
    }
}

/// Helper to find a smaller child node that still contains the point.
fn find_tighter_child(node: Node, point: usize) -> Option<Node> {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.start_byte() <= point && child.end_byte() > point {
                return Some(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

const DECLARATION_TYPES: &[&str] = &[
    // Common / Shared types
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "method_declaration",
    "constructor_declaration",
    "property_declaration",
    "type_alias_declaration",
    // Swift
    "function_declaration",
    "protocol_declaration",
    "init_declaration",
    "deinit_declaration",
    "subscript_declaration",
    "enum_entry",
    "protocol_function_declaration",
    // Rust
    "function_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "impl_item",
    "type_item",
    "const_item",
    "static_item",
    "mod_item",
    "macro_definition",
    // TypeScript
    "method_definition",
    "lexical_declaration",
    "export_statement",
    // Python
    "function_definition",
    "class_definition",
    "decorated_definition",
    // Go
    "type_declaration",
    "type_spec",
    "var_declaration",
    // Java (shared above)
    // C#
    "namespace_declaration",
    "record_declaration",
    "struct_declaration",
    "method_elem",
    // Dart
    "function_signature",
    "initialized_identifier",
    "enum_constant",
];

/// Whether `node_kind` should be treated as a structural landmark for query
/// anchoring.
///
/// Consults the language [`Profile::landmark_kinds`] first so dynamic (WASM)
/// grammars honor their manifest, then falls back to the built-in
/// [`DECLARATION_TYPES`] union. For static languages the profile's
/// `landmark_kinds` mirrors the historical per-language subset, and the union
/// fallback preserves exact pre-refactor behavior for any kind the profile
/// omits.
fn is_landmark_kind(node_kind: &str, profile: &Profile) -> bool {
    profile.landmark_kinds.iter().any(|k| k == node_kind) || DECLARATION_TYPES.contains(&node_kind)
}

/// Map tree-sitter node types to breadcrumb category tags for @sticky captures.
/// Returns the category name (e.g., "class", "function", "method") for landmark nodes.
fn get_sticky_tag(node_kind: &str) -> Option<&'static str> {
    match node_kind {
        // Classes / Structs / Interfaces
        "class_declaration" | "struct_declaration" | "interface_declaration" => Some("class"),
        "impl_item" | "trait_item" => Some("impl"),

        // Functions / Methods
        "function_declaration" | "function_item" | "function_definition" => Some("function"),
        "method_definition" | "method_declaration" => Some("method"),

        // Modules / Namespaces
        "mod_item" => Some("module"),
        "namespace_declaration" => Some("namespace"),

        // Protocols / Traits
        "protocol_declaration" => Some("protocol"),

        // Enums
        "enum_declaration" | "enum_item" => Some("enum"),

        // Types
        "type_declaration" | "type_alias_declaration" | "type_spec" => Some("type"),

        _ => None,
    }
}

/// Check if a declaration node is likely a local variable/constant.
fn is_local_declaration(node: Node) -> bool {
    let kind = node.kind();
    if kind != "property_declaration"
        && kind != "variable_declaration"
        && kind != "lexical_declaration"
    {
        return false;
    }

    // Check ancestors: if we're inside a function or closure, it's local
    let mut current = node;
    while let Some(parent) = current.parent() {
        let pk = parent.kind();
        if pk.contains("function")
            || pk.contains("method")
            || pk.contains("lambda")
            || pk == "closure_expression"
        {
            return true;
        }
        if is_root_node(pk) {
            break;
        }
        current = parent;
    }
    false
}

fn build_base_query(
    node: Node,
    _name: Option<&str>,
    semantic_info: Option<SemanticInfo>,
    source: &[u8],
    profile: &Profile,
) -> Result<String> {
    // We leverage the structural path logic for a single node to ensure consistency
    let entry = PathEntry {
        node_type: node.kind().to_string(),
        name_info: extract_name_info(node, source),
        semantic_info,
        is_landmark: is_landmark_kind(node.kind(), profile) && !is_local_declaration(node),
        sticky_tag: get_sticky_tag(node.kind()).map(|s| s.to_string()),
        // A single-entry path nests no child, so nothing can be mis-ordered.
        name_after_child: false,
    };

    let path = vec![entry];
    Ok(build_tier1_query(&path))
}

/// Helper to ensure a query is unique and anchored by walking up parents.
fn disambiguate_query(target_node: Node, ctx: &QueryContext<'_>) -> Result<String> {
    // Scoped to this call so the memoized answers cannot outlive the grammar
    // they were measured against. See [`FieldProbe`].
    let probe = FieldProbe::new(ctx.language);
    let mut path = build_structural_path(target_node, ctx.source, ctx.profile, Some(&probe));

    if path.is_empty() {
        // Fallback to simple base query if no path can be built
        let name = extract_name_info(target_node, ctx.source)
            .map(|info| info.text)
            .or_else(|| extract_identifier_from_node(target_node, ctx.source));
        let semantic_info = extract_semantic_info(target_node, ctx.source);
        return build_base_query(
            target_node,
            name.as_deref(),
            semantic_info,
            ctx.source,
            ctx.profile,
        );
    }

    // First, clear all names in the path to start with a pure structural query
    // We preserve the target node's semantic info to keep the query specific
    let target_semantic_info = path.last().and_then(|e| e.semantic_info.clone());
    let mut names = Vec::new();
    for entry in &mut path {
        names.push(entry.name_info.take());
        entry.semantic_info = None;
    }

    let depth = path.len();

    // Strategy: find the minimum set of names needed to make the full structural path unique.
    // We always include the full path for maximum stability.

    // 1. Try with only the target node named (if it has a name)
    let mut current_path = path.clone();
    for (i, entry) in current_path.iter_mut().enumerate() {
        if i < depth - 1 {
            entry.name_info = None;
            entry.semantic_info = None;
        } else {
            entry.name_info = names[depth - 1].clone();
            entry.semantic_info = target_semantic_info.clone();
        }
    }

    // Landmark requirement check: at least one NAMED landmark ancestor
    let has_named_landmark = |p: &[PathEntry]| {
        p.iter().enumerate().any(|(i, e)| i < p.len() - 1 && e.is_landmark && e.name_info.is_some())
    };

    let query = build_tier1_query(&current_path);
    if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1
        && (has_named_landmark(&current_path) || depth == 1)
    {
        return Ok(query);
    }

    // 2. Progressively add names to landmarks from target upwards
    for i in (0..depth - 1).rev() {
        if path[i].is_landmark {
            current_path[i].name_info = names[i].clone();
            let query = build_tier1_query(&current_path);
            if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1 {
                return Ok(query);
            }
        }
    }

    // 3. If still not unique, add all names
    for i in (0..depth - 1).rev() {
        current_path[i].name_info = names[i].clone();
    }
    let query = build_tier1_query(&current_path);
    let match_count = matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len();
    if match_count == 1 {
        return Ok(query);
    }

    // 4. Final resort: try unnamed path (only if naming the target didn't work and we reach the root)
    let query = build_tier1_query(&path);
    if matcher::run_query(&query, ctx.tree, ctx.source, ctx.language)?.len() == 1 {
        return Ok(query);
    }

    // If still not unique, use the base_query if it was unique,
    // otherwise return the best we could do (which might still be ambiguous)
    let final_query = build_tier1_query(&path);
    let final_matches = matcher::run_query(&final_query, ctx.tree, ctx.source, ctx.language)?;

    if final_matches.len() != 1 {
        return Err(Error::AmbiguousQuery(format!(
            "Generated query matched {} nodes, expected 1. Try selecting a more specific range.",
            final_matches.len()
        )));
    }

    Ok(final_query)
}

/// Given a specific AST node, generate a tree-sitter query for it.
pub fn generate_query_for_node(
    tree: &Tree,
    node: Node,
    source: &[u8],
    language: &Language,
    profile: &Profile,
) -> Result<GeneratedQuery> {
    generate_query(tree, source, (node.start_byte(), node.end_byte()), language, profile)
}

/// Build the structural path from the target node up to (but not including) the root.
/// Body nodes (class_body, etc.) are included to ensure the query nesting matches the AST.
/// Wrapper nodes (export_statement, decorated_definition) are skipped — they don't have
/// queryable name fields.
fn build_structural_path(
    target: Node,
    source: &[u8],
    profile: &Profile,
    probe: Option<&FieldProbe<'_>>,
) -> Vec<PathEntry> {
    if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
        eprintln!(
            "DEBUG: build_structural_path: target={} at {:?}",
            target.kind(),
            target.byte_range()
        );
    }
    let mut path = Vec::new();
    let mut current = target;

    // Special case: if the target is a leaf node (like an identifier) and its
    // parent names it, start the path from the parent to avoid "Impossible
    // pattern" errors where the query expects two different children for the
    // same node.
    //
    // Asks the extractor which child it would name rather than looking only at
    // a `name` field: Rust's `impl_item` names itself through `type`, so
    // bookmarking the `Claims` in `impl Claims` produced
    // `(impl_item type: (type_identifier) @n ((type_identifier) @target ...))`
    // — the same single child demanded twice.
    if let Some(parent) = current.parent()
        && let Some(info) = extract_name_info(parent, source)
        // Only a *field-named* child is a true duplicate. A recovered name
        // (`field: None`) renders as `(intermediate (identifier) @n)`, which
        // reaches a level deeper than the lifted node would, so lifting there
        // discards the predicate rather than de-duplicating it.
        && info.field.is_some()
        && info.inner_types.is_empty()
        && let Some(name_node) = locate_name_node(parent, &info, probe)
        && name_node.id() == current.id()
    {
        current = parent;
    }

    let mut is_first = true;
    // The node the previous iteration emitted, i.e. the child this entry will
    // nest in the query. `None` for the target itself, which has no child.
    let mut child_on_path: Option<Node> = None;
    loop {
        // Skip wrapper nodes that don't have structural meaning for queries
        if !is_wrapper_node(current.kind()) {
            let is_target_node = is_first;
            is_first = false;
            let mut name_info = if is_body_node(current.kind()) {
                None
            } else {
                // The target is on the path below `current`, so any
                // intermediate containing it is already its own entry.
                extract_name_info_toward(current, source, Some(target), probe)
            };
            // Drop the name when it *is* the child the path descends into: the
            // nested pattern already constrains that subtree, so naming it here
            // asks for the same single child twice. Lua's
            // `function_declaration` names a `dot_index_expression` that the
            // path also walks through on the way to `M.compute`.
            //
            // Only for a field-named child. A *recovered* name (`field: None`)
            // renders as `(function_signature (identifier) @n)`, which reaches
            // one level deeper than the path child's own pattern and so is not
            // a duplicate — dropping it would strip the predicate that tells
            // six Dart methods apart.
            if let Some(ref info) = name_info
                && info.field.is_some()
                && info.inner_types.is_empty()
                && let Some(child) = child_on_path
                && locate_name_node(current, info, probe).is_some_and(|n| n.id() == child.id())
            {
                name_info = None;
            }
            let entry = PathEntry {
                node_type: current.kind().to_string(),
                name_after_child: name_follows_child(
                    current,
                    name_info.as_ref(),
                    child_on_path,
                    probe,
                ),
                name_info,
                semantic_info: if is_target_node {
                    extract_semantic_info(current, source)
                } else {
                    None
                },
                is_landmark: is_landmark_kind(current.kind(), profile)
                    && !is_local_declaration(current),
                sticky_tag: get_sticky_tag(current.kind()).map(|s| s.to_string()),
            };
            child_on_path = Some(current);
            if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
                eprintln!(
                    "DEBUG:   entry={} has_name={} is_landmark={}",
                    entry.node_type,
                    entry.name_info.is_some(),
                    entry.is_landmark
                );
            }
            path.push(entry);
        }

        match current.parent() {
            Some(parent) if !is_root_node(parent.kind()) => {
                current = parent;
            }
            _ => break,
        }
    }

    path.reverse(); // outermost first
    path
}

/// Does `node`'s name node come after the child the path descends into?
///
/// Sibling patterns in a tree-sitter query are an **ordered sequence**: they
/// have to appear in the order the grammar emits the children. The generator
/// writes the name pattern and then nests the next path entry, which is only
/// valid when the name actually precedes that child.
///
/// Two grammars where it does not:
///
/// ```text
/// struct_item        [ -=visibility_modifier  -=struct  name=type_identifier  body=... ]
/// function_signature [ return_type=type_identifier  name=identifier  parameters=... ]
/// ```
///
/// Bookmarking the `pub` or the return type puts the path child first, so
/// `(struct_item name: ... (visibility_modifier) @target)` cannot match and
/// tree-sitter rejects the whole query.
///
/// Compares by child index rather than byte offset: both are ordered the same
/// way, but an index is exactly the sequence tree-sitter matches against.
fn name_follows_child(
    node: Node,
    name_info: Option<&NameInfo>,
    child: Option<Node>,
    probe: Option<&FieldProbe<'_>>,
) -> bool {
    let (Some(info), Some(child)) = (name_info, child) else {
        return false;
    };
    // A name rendered as the node's own capture is not a sibling pattern at
    // all, so ordering cannot apply to it.
    if info.field.is_none() && node.kind() == info.direct_type {
        return false;
    }
    let Some(name_node) = locate_name_node(node, info, probe) else {
        return false;
    };

    let mut cursor = node.walk();
    let mut name_idx = None;
    let mut child_idx = None;
    for (i, c) in node.children(&mut cursor).enumerate() {
        if c.id() == name_node.id() {
            name_idx = Some(i);
        }
        if c.id() == child.id() {
            child_idx = Some(i);
        }
    }
    match (name_idx, child_idx) {
        (Some(n), Some(c)) => n > c,
        _ => false,
    }
}

/// The direct child of `node` that [`NameInfo`] was derived from.
///
/// Mirrors how `extract_name_info_direct` found it, so the index comparison in
/// [`name_follows_child`] is made against the node the query will actually
/// name.
fn locate_name_node<'t>(
    node: Node<'t>,
    info: &NameInfo,
    probe: Option<&FieldProbe<'_>>,
) -> Option<Node<'t>> {
    if let Some(ref field) = info.field {
        return name_child_by_own_field(node, field, probe);
    }
    // Fieldless: either a recovered intermediate (`direct_type` is the
    // intermediate kind) or a positional descendant match.
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|c| c.kind() == info.direct_type)
}

/// Extract the "name" identifier from a node if it has one.
///
/// For callers not building a structural path, where there is no descendant to
/// exclude from [`recover_nested_name`].
fn extract_name_info(node: Node, source: &[u8]) -> Option<NameInfo> {
    extract_name_info_toward(node, source, None, None)
}

/// [`extract_name_info`], told which descendant the query is being built toward.
///
/// Only [`recover_nested_name`] uses `descendant_on_path`, to avoid naming an
/// intermediate that the structural path will already emit as its own pattern.
/// Passing `None` is safe — it just forgoes that de-duplication, which only
/// matters while walking a path.
fn extract_name_info_toward<'t>(
    node: Node<'t>,
    source: &[u8],
    descendant_on_path: Option<Node<'t>>,
    probe: Option<&FieldProbe<'_>>,
) -> Option<NameInfo> {
    if node.kind() == "class_member" || node.kind() == "declaration" {
        return extract_nested_name(node, source);
    }
    extract_name_info_direct_toward(node, source, descendant_on_path, probe)
}

/// Helper to recursively search for a "name" field in a node's children.
fn extract_nested_name(node: Node, source: &[u8]) -> Option<NameInfo> {
    if let Some(mut info) = extract_name_info_direct(node, source) {
        info.field = None; // Field name is relative to child, not node
        return Some(info);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(info) = extract_nested_name(child, source) {
            return Some(info);
        }
    }
    None
}

/// [`child_by_own_field`], restricted to a child that can be *named* in a query.
///
/// A field can hold an anonymous token, and an anonymous kind is not a node
/// type: it has to be written as a quoted literal, so `(init)` is rejected
/// where `"init"` compiles. Swift's initializer is the case in the tree:
///
/// ```text
/// init_declaration [ name=init  -=(  -=parameter  -=)  body=function_body ]
/// ```
///
/// `name` really is declared here, and the child really is the `init` keyword —
/// `is_named() == false`. Emitting `name: (init)` made *every* node inside a
/// Swift initializer unbookmarkable, 27 targets in the committed fixture alone.
///
/// Quoting it instead would compile but assert nothing: a keyword is the same
/// in every initializer, so it cannot tell two of them apart. Treating the
/// kind as nameless is both correct and more useful — `init_declaration` then
/// anchors on its position and parameters, exactly as it did before anyone
/// looked for a name.
///
/// Navigation keeps using [`child_by_own_field`]: descending into an anonymous
/// child to read its text is fine, since no node type is written out.
///
/// `probe` is what makes the last check possible: being present on the
/// *parsed tree* does not mean a field is writable in a *query*. See
/// [`FieldProbe::field_is_queryable`].
fn name_child_by_own_field<'t>(
    node: Node<'t>,
    field: &str,
    probe: Option<&FieldProbe<'_>>,
) -> Option<Node<'t>> {
    let child = child_by_own_field(node, field).filter(Node::is_named)?;
    match probe {
        Some(probe) if !probe.field_is_queryable(node.kind(), field) => None,
        _ => Some(child),
    }
}

/// Asks a grammar whether `(kind field: (_))` compiles, and remembers the answer.
///
/// The authority on query syntax is the query compiler, and it disagrees with
/// the tree API. Swift reports a `name` field on a parsed `type_annotation`:
///
/// ```text
/// type_annotation [ -=:  name=dictionary_type ]
/// ```
///
/// yet `(type_annotation name: (_))` is an "Impossible pattern". The same holds
/// for `dictionary_type` and `array_type`. Nothing observable on the node
/// explains it — `grammar_name` equals `kind`, so these are not aliases or
/// supertypes — the field simply is not in the compiler's static table for that
/// kind. Compare `(array_type element: (_))`, which does compile.
///
/// So the question is asked the only way that is reliable: compile a minimal
/// probe. Walking children answers "does this *instance* have the field"
/// (#239); [`Self::field_is_queryable`] answers "may a query say so at all"
/// (#267). Both are needed, and neither implies the other.
///
/// The cache lives **with the borrowed grammar** rather than in a `static`. A
/// process-global cache has to name the grammar in its key, and nothing on
/// [`Language`] identifies one: ABI version and node-kind count are shared by
/// unrelated builds, and the pointer inside `Language` is refcounted, so a
/// reloaded WASM grammar can land on a freed address. Codemark loads grammars
/// dynamically and the TUI reloads them in place, so both failure modes are
/// reachable — and a stale `false` *silently drops a name* instead of failing
/// loudly. Borrowing sidesteps identity entirely: the cache cannot outlive the
/// grammar it describes, so there is nothing to go stale.
///
/// Scoping it per generation does mean re-probing each `generate_query` call.
/// That is the cost the measurements already cover — 20 CLI `add`s take 4.14s
/// with and without the probe — while within a call the repeated hits (one per
/// path entry, five extraction sites) still collapse to one compile each.
struct FieldProbe<'a> {
    language: &'a Language,
    /// `(kind, field)` → whether a query may name that field on that kind.
    cache: RefCell<HashMap<(String, String), bool>>,
}

impl<'a> FieldProbe<'a> {
    fn new(language: &'a Language) -> Self {
        Self { language, cache: RefCell::new(HashMap::new()) }
    }

    /// Whether `(kind field: (_))` compiles against this grammar.
    fn field_is_queryable(&self, kind: &str, field: &str) -> bool {
        let key = (kind.to_string(), field.to_string());
        if let Some(&known) = self.cache.borrow().get(&key) {
            return known;
        }

        let valid =
            tree_sitter::Query::new(self.language, &format!("({kind} {field}: (_))")).is_ok();
        self.cache.borrow_mut().insert(key, valid);
        valid
    }
}

/// The child under `field` **only if** `node` itself declares that field.
///
/// [`Node::child_by_field_name`] looks through hidden nodes, so it answers "is
/// there a `name` somewhere beneath this?" rather than "does this kind have a
/// `name` field?". Those differ, and the gap produces queries tree-sitter
/// refuses to compile. Lua is the clean example:
///
/// ```text
/// assignment_statement [ -=variable_list  operator="="  -=expression_list ]
///   variable_list      [ name=identifier ]
/// ```
///
/// `assignment_statement.child_by_field_name("name")` returns the identifier —
/// found through the fieldless `variable_list` — but the kind has no `name`
/// field of its own, so `(assignment_statement name: (identifier))` is an
/// "Impossible pattern" and tree-sitter rejects the *entire* query. The
/// bookmark is then never created.
///
/// Checking the grammar's global field table is not enough: Lua *does* define
/// `name` (field id 15), just not on this kind. The question is only ever
/// answerable per node kind, which is what walking the direct children does.
fn child_by_own_field<'t>(node: Node<'t>, field: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .enumerate()
        .find(|(i, _)| u32::try_from(*i).is_ok_and(|i| node.field_name_for_child(i) == Some(field)))
        .map(|(_, child)| child)
}

/// A name sitting under a fieldless direct child, expressed positionally.
///
/// The companion to [`child_by_own_field`]. When a kind carries no `name` field
/// itself, the name is often one level down behind an intermediate the grammar
/// leaves fieldless — Lua's `assignment_statement > variable_list > identifier`.
/// Dropping the name entirely would still compile, but the query would be
/// purely positional and break the moment a sibling statement is inserted.
///
/// Returns the intermediate as `direct_type` and the identifier as
/// `inner_types`, which [`build_tier1_query`] renders as
/// `(variable_list (identifier) @cap)` — no field, so it compiles.
///
/// `descendant_on_path` is the node the query is being built *towards*, when
/// known. An intermediate that contains it is already its own entry in the
/// structural path, so naming it here would emit a second sibling pattern for a
/// child the node has only one of — impossible, for a different reason than the
/// missing field. Both grammars hit this:
///
/// ```text
/// method_signature [ -=function_signature ]       (Dart)
///   function_signature [ name=identifier ]        <-- already a path entry
///
/// assignment_statement [ -=variable_list ... ]    (Lua, target = variable_list)
///   variable_list [ name=identifier ]             <-- already a path entry
/// ```
fn recover_nested_name<'t>(
    node: Node<'t>,
    source: &[u8],
    descendant_on_path: Option<Node<'t>>,
    probe: Option<&FieldProbe<'_>>,
) -> Option<NameInfo> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        // A bare identifier child is the caller's business, not an
        // intermediate: `extract_name_info_direct` handles that shape.
        if child.kind().contains("identifier") {
            continue;
        }
        // Would this intermediate also be emitted as its own pattern?
        if descendant_on_path.is_some_and(|d| {
            let mut up = Some(d);
            while let Some(n) = up {
                if n.id() == child.id() {
                    return true;
                }
                up = n.parent();
            }
            false
        }) {
            continue;
        }
        let Some(named) = name_child_by_own_field(child, "name", probe) else { continue };
        if !named.kind().contains("identifier") {
            continue;
        }
        return Some(NameInfo {
            field: None,
            direct_type: child.kind().to_string(),
            // `named` was found *through* the `name` field, so the hop can
            // assert it. A bare positional `(identifier)` would also match the
            // intermediate's other identifier children — a TS
            // `variable_declarator`'s `value`, letting a query for
            // `export const Foo = 1` match `export const Bar = Foo`.
            inner_types: vec![NameHop {
                field: Some("name".to_string()),
                kind: named.kind().into(),
            }],
            text: node_text(named, source),
        });
    }
    None
}

/// Direct name extraction without recursion, to avoid infinite loops.
fn extract_name_info_direct(node: Node, source: &[u8]) -> Option<NameInfo> {
    extract_name_info_direct_toward(node, source, None, None)
}

/// [`extract_name_info_direct`], carrying the descendant to exclude from
/// [`recover_nested_name`].
fn extract_name_info_direct_toward<'t>(
    node: Node<'t>,
    source: &[u8],
    descendant_on_path: Option<Node<'t>>,
    probe: Option<&FieldProbe<'_>>,
) -> Option<NameInfo> {
    if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
        eprintln!(
            "DEBUG: extract_name_info_direct: node={} at {:?}",
            node.kind(),
            node.byte_range()
        );
    }
    // If the node itself is an identifier, it is its own name.
    // Literals (boolean_literal, etc.) should not be treated as names because they
    // are often not searchable as node types in the same way.
    if node.kind().contains("identifier") {
        return Some(NameInfo {
            field: None,
            direct_type: node.kind().to_string(),
            inner_types: Vec::new(),
            text: node_text(node, source),
        });
    }

    // Try the "name" field first. `child_by_own_field` rather than
    // `child_by_field_name`: a name reachable only through a hidden node cannot
    // be written as `name:` against *this* kind.
    if let Some(name_node) = name_child_by_own_field(node, "name", probe) {
        if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
            eprintln!("DEBUG:   found name field node={}", name_node.kind());
        }
        // For Swift user_type nodes (extensions), we need nested matching
        if name_node.kind() == "user_type" {
            let mut cursor = name_node.walk();
            for child in name_node.named_children(&mut cursor) {
                if child.kind() == "type_identifier" {
                    return Some(NameInfo {
                        field: Some("name".to_string()),
                        direct_type: "user_type".to_string(),
                        inner_types: vec![NameHop::positional("type_identifier")],
                        text: node_text(child, source),
                    });
                }
            }
        }
        return Some(NameInfo {
            field: Some("name".to_string()),
            direct_type: name_node.kind().to_string(),
            inner_types: Vec::new(),
            text: node_text(name_node, source),
        });
    }

    // For Rust impl_item: use "type" field as the name
    if node.kind() == "impl_item"
        && let Some(type_node) = name_child_by_own_field(node, "type", probe)
    {
        return Some(NameInfo {
            field: Some("type".to_string()),
            direct_type: type_node.kind().to_string(),
            inner_types: Vec::new(),
            text: node_text(type_node, source),
        });
    }

    // For TS export_statement / Python decorated_definition: the name lives on
    // the inner declaration.
    //
    // The inner `NameInfo`'s `field` is relative to *that* node, so it cannot
    // be reused verbatim — `(decorated_definition name: (identifier))` names a
    // field `decorated_definition` does not declare, which is an "Impossible
    // pattern". Re-express it as a nested positional match instead:
    // `(decorated_definition (function_definition (identifier) @n))`.
    //
    // These kinds are normally skipped by `is_wrapper_node`, so this path is
    // reached only when the wrapper is itself the bookmark target.
    //
    // The nesting must reach the *identifier*, not just the inner declaration's
    // name node. `export const Foo = 1` names itself through
    // `lexical_declaration > variable_declarator > identifier`, so capturing the
    // declarator would compare its whole text (`Foo = 1`) against `Foo` and
    // match nothing. Such a query still compiles, which is why the
    // impossible-pattern sweep cannot catch it.
    for field in ["declaration", "definition"] {
        if matches!(node.kind(), "export_statement" | "decorated_definition")
            && let Some(inner) = child_by_own_field(node, field)
            && let Some(info) = extract_name_info_direct(inner, source)
        {
            // The hop keeps `info.field`: that field belongs to the *inner*
            // declaration, which is emitted as its own pattern here, so it stays
            // valid — only the wrapper may not claim it. Dropping it would leave
            // a positional `(identifier)` free to bind the declarator's `value`
            // instead of its name.
            let mut inner_types = match locate_name_node(inner, &info, probe) {
                Some(n) => vec![NameHop { field: info.field.clone(), kind: n.kind().to_string() }],
                None => {
                    vec![NameHop { field: info.field.clone(), kind: info.direct_type.clone() }]
                }
            };
            // The inner name was itself nested; carry those levels through.
            inner_types.extend(info.inner_types.iter().cloned());
            return Some(NameInfo {
                field: None,
                direct_type: inner.kind().to_string(),
                inner_types,
                text: info.text,
            });
        }
    }

    // For Rust match_arm: use pattern text
    if node.kind() == "match_arm"
        && let Some(pattern) = name_child_by_own_field(node, "pattern", probe)
    {
        return Some(NameInfo {
            field: Some("pattern".to_string()),
            direct_type: pattern.kind().to_string(),
            inner_types: Vec::new(),
            text: node_text(pattern, source),
        });
    }

    // For Swift switch_entry: use pattern or "default"
    if node.kind() == "switch_entry" {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "switch_pattern" {
                return Some(NameInfo {
                    field: None,
                    direct_type: "switch_pattern".to_string(),
                    inner_types: Vec::new(),
                    text: node_text(child, source),
                });
            }
            if child.kind() == "default_keyword" {
                return Some(NameInfo {
                    field: None,
                    direct_type: "default_keyword".to_string(),
                    inner_types: Vec::new(),
                    text: "default".to_string(),
                });
            }
        }
    }

    // Last resort. The kind has no `name` field of its own, but a name may
    // still be reachable through a fieldless intermediate child. Recover it
    // positionally — `(variable_list (identifier) @cap)` compiles where
    // `name: (identifier) @cap` does not — so the query keeps the name
    // predicate that makes a bookmark survive edits elsewhere in the file.
    //
    // Deliberately after the kind-specific branches above. Those encode a
    // chosen name for a kind (a `match_arm`'s pattern, not its value), and a
    // generic structural guess must not pre-empt a deliberate one.
    let recovered = recover_nested_name(node, source, descendant_on_path, probe);
    if let Some(ref info) = recovered
        && std::env::var("CODEMARK_DEBUG_QUERY").is_ok()
    {
        eprintln!(
            "DEBUG:   recovered nested name via {} > {:?}",
            info.direct_type, info.inner_types
        );
    }
    recovered
}

fn node_text(node: Node, source: &[u8]) -> String {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("").to_string()
}

fn is_body_node(kind: &str) -> bool {
    matches!(
        kind,
        // Swift
        "class_body"
            | "enum_class_body"
            | "protocol_body"
            | "function_body"
            | "statements"
            | "computed_getter"
            // Rust
            | "declaration_list"
            | "field_declaration_list"
            | "enum_variant_list"
            | "block"
            // TypeScript
            | "interface_body"
            | "enum_body"
            | "statement_block"
            | "object_type"
            // Python (block already listed)
            // Go
            | "interface_type"
            | "struct_type"
            // Java / C#
            | "constructor_body"
            | "enum_body_declarations"
            // Dart
            | "class_member"
            | "declaration"
    )
}

/// Wrapper nodes that should be skipped in the structural path.
fn is_wrapper_node(kind: &str) -> bool {
    matches!(kind, "export_statement" | "decorated_definition")
}

/// Root node types across languages.
fn is_root_node(kind: &str) -> bool {
    matches!(kind, "source_file" | "program" | "module" | "compilation_unit")
}

/// Build a Tier 1 (exact) S-expression query from the structural path.
fn build_tier1_query(path: &[PathEntry]) -> String {
    if path.is_empty() {
        return String::new();
    }

    let depth = path.len();
    let mut capture_counter = 0;

    // Build the query recursively: outermost node first, target node last.
    // Predicates go inside each node's pattern, before the closing paren.
    fn build_node(
        path: &[PathEntry],
        idx: usize,
        depth: usize,
        indent: usize,
        counter: &mut usize,
    ) -> String {
        let entry = &path[idx];
        let is_target = idx == depth - 1;
        let pad = "  ".repeat(indent);
        let mut s = format!("{pad}({}", entry.node_type);

        // Name field with text predicate
        let mut inner_predicate = String::new();
        let mut outer_predicate = String::new();

        // Semantic info (if present)
        if let Some(ref semantic) = entry.semantic_info {
            match semantic {
                SemanticInfo::IfCondition(cond) => {
                    s.push_str(&format!("\n{pad}  condition: (_) @cond"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @cond \"{}\")",
                        escape_query_text(cond)
                    ));
                }
                SemanticInfo::CallTarget(func) => {
                    s.push_str(&format!("\n{pad}  (_) @func"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @func \"{}\")",
                        escape_query_text(func)
                    ));
                }
                SemanticInfo::AssignmentTarget(target_name) => {
                    s.push_str(&format!("\n{pad}  left: (_) @left"));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @left \"{}\")",
                        escape_query_text(target_name)
                    ));
                }
                SemanticInfo::ReturnValue(val) => {
                    s.push_str(&format!("\n{pad}  value: (_) @val"));
                    inner_predicate
                        .push_str(&format!("\n{pad}  (#eq? @val \"{}\")", escape_query_text(val)));
                }
                SemanticInfo::BinaryOperator(op) => {
                    s.push_str(&format!("\n{pad}  operator: \"{}\"", escape_query_text(op)));
                }
            }
        }

        // Held separately so it can be placed after the nested child when the
        // grammar orders the name later; see `PathEntry::name_after_child`.
        let mut name_pattern = String::new();
        if let Some(ref info) = entry.name_info {
            let capture_name = if is_target {
                "fn_name".to_string()
            } else {
                let name = format!("name{}", *counter);
                *counter += 1;
                name
            };

            // Determine if this landmark node gets a sticky capture
            let sticky_suffix = if let Some(ref tag) = entry.sticky_tag {
                format!(" @sticky.{}", tag)
            } else {
                String::new()
            };

            if !info.inner_types.is_empty() {
                // Nested name: e.g., name: (user_type (type_identifier) @capture).
                // The capture goes on the innermost kind so the predicate
                // compares the identifier's text, not an enclosing node's.
                // Each hop re-states the field it fills in its own parent, so a
                // name cannot be satisfied by a sibling under another field.
                let (last, outer) =
                    info.inner_types.split_last().expect("inner_types is non-empty in this branch");
                let mut nested = match last.field {
                    Some(ref f) => format!("{f}: ({}) @{capture_name}{sticky_suffix}", last.kind),
                    None => format!("({}) @{capture_name}{sticky_suffix}", last.kind),
                };
                for hop in outer.iter().rev() {
                    nested = match hop.field {
                        Some(ref f) => format!("{f}: ({} {nested})", hop.kind),
                        None => format!("({} {nested})", hop.kind),
                    };
                }
                if let Some(ref field_name) = info.field {
                    name_pattern.push_str(&format!(
                        "\n{pad}  {}: ({} {nested})",
                        field_name, info.direct_type
                    ));
                } else {
                    name_pattern.push_str(&format!("\n{pad}  ({} {nested})", info.direct_type));
                }
                inner_predicate.push_str(&format!(
                    "\n{pad}  (#eq? @{} \"{}\")",
                    capture_name,
                    escape_query_text(&info.text)
                ));
            } else if let Some(ref field_name) = info.field {
                name_pattern.push_str(&format!(
                    "\n{pad}  {}: ({}) @{capture_name}{}",
                    field_name, info.direct_type, sticky_suffix
                ));
                inner_predicate.push_str(&format!(
                    "\n{pad}  (#eq? @{} \"{}\")",
                    capture_name,
                    escape_query_text(&info.text)
                ));
            } else {
                // Leaf node or descendant match without a field
                // Use the node itself as the capture if it matches the type
                if entry.node_type == info.direct_type {
                    // Handled at the end with s.push_str(" @capture_name")
                    outer_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @{} \"{}\")",
                        capture_name,
                        escape_query_text(&info.text)
                    ));
                } else {
                    name_pattern.push_str(&format!(
                        "\n{pad}  ({}) @{}{}",
                        info.direct_type, capture_name, sticky_suffix
                    ));
                    inner_predicate.push_str(&format!(
                        "\n{pad}  (#eq? @{} \"{}\")",
                        capture_name,
                        escape_query_text(&info.text)
                    ));
                }
            }
        }

        if is_target {
            // No nested child, so there is nothing for the name to be ordered
            // against.
            s.push_str(&name_pattern);
            // Add inner predicate and close inner node
            s.push_str(&inner_predicate);
            s.push(')');

            // Add name capture if it was on the node itself
            if let Some(ref info) = entry.name_info
                && info.field.is_none()
                && entry.node_type == info.direct_type
            {
                let capture_name = "fn_name";
                let sticky_suffix = if let Some(ref tag) = entry.sticky_tag {
                    format!(" @sticky.{}", tag)
                } else {
                    String::new()
                };
                s.push_str(&format!(" @{}{}", capture_name, sticky_suffix));
            }

            s.push_str(" @target");

            // Wrap in extra parens if we have an outer predicate
            if !outer_predicate.is_empty() {
                s = format!("{pad}({}{}", &s[pad.len()..], outer_predicate);
                s.push(')');
            }
        } else {
            // Sibling *node* patterns are an ordered sequence, so the name and
            // the nested child must appear in the order the grammar emits them.
            // Predicates are position-independent, so they stay where they were
            // to avoid churning the text of every already-stored query.
            let child_str = build_node(path, idx + 1, depth, indent + 1, counter);
            if entry.name_after_child {
                s.push('\n');
                s.push_str(&child_str);
                s.push_str(&name_pattern);
                s.push_str(&inner_predicate);
            } else {
                s.push_str(&name_pattern);
                s.push_str(&inner_predicate);
                s.push('\n');
                s.push_str(&child_str);
            }
            s.push(')');

            // Add name capture if it was on the node itself
            if let Some(ref info) = entry.name_info
                && info.field.is_none()
                && entry.node_type == info.direct_type
            {
                let capture_name = format!("name{}", *counter - 1);
                let sticky_suffix = if let Some(ref tag) = entry.sticky_tag {
                    format!(" @sticky.{}", tag)
                } else {
                    String::new()
                };
                s.push_str(&format!(" @{}{}", capture_name, sticky_suffix));
            }

            // Wrap in extra parens if we have an outer predicate
            if !outer_predicate.is_empty() {
                s = format!("{pad}({}{}", &s[pad.len()..], outer_predicate);
                s.push(')');
            }
        }

        s
    }

    build_node(path, 0, depth, 0, &mut capture_counter)
}

fn escape_query_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
}

/// Extract semantic information from a node for fine-grained targeting.
///
/// Each variant here is emitted as a *field constraint* (`condition:`, `left:`,
/// `value:`), so these lookups use [`child_by_own_field`] for the same reason
/// the name extraction does: a field reachable only through a hidden node
/// cannot be written against this kind. Lua's `assignment_statement` has no
/// `left` field — its operands sit under fieldless `variable_list` /
/// `expression_list` children.
fn extract_semantic_info(node: Node, source: &[u8]) -> Option<SemanticInfo> {
    match node.kind() {
        "if_statement" | "if_expression" => {
            // Extract the condition
            if let Some(cond) = child_by_own_field(node, "condition") {
                let text = node_text(cond, source);
                return Some(SemanticInfo::IfCondition(text));
            }
        }
        "call_expression" | "macro_invocation" => {
            // Extract the function being called
            if let Some(func) = node.child_by_field_name("function").or_else(|| node.named_child(0))
            {
                let text = node_text(func, source);
                return Some(SemanticInfo::CallTarget(text));
            }
        }
        "assignment_expression" | "assignment_statement" | "short_var_declaration" => {
            // Extract the variable being assigned
            if let Some(left) = child_by_own_field(node, "left") {
                let text = node_text(left, source);
                return Some(SemanticInfo::AssignmentTarget(text));
            } else if node.kind() == "short_var_declaration"
                && let Some(left) = node.named_child(0)
            {
                let text = node_text(left, source);
                return Some(SemanticInfo::AssignmentTarget(text));
            }
        }
        "return_statement" => {
            // Extract the return value
            if let Some(value) = child_by_own_field(node, "value") {
                let text = node_text(value, source);
                if !text.trim().is_empty() {
                    return Some(SemanticInfo::ReturnValue(text));
                }
            }
        }
        _ => {}
    }
    None
}

/// Extract an identifier from a node for fallback naming.
fn extract_identifier_from_node(node: Node, source: &[u8]) -> Option<String> {
    // Try to find any identifier child
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind().contains("identifier") {
            return Some(node_text(child, source));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::languages::{Language as CodemarkLang, Parser};

    // A profile-declared landmark kind that is *not* in the built-in
    // `DECLARATION_TYPES` union — models a dynamic (WASM) grammar's manifest.
    #[test]
    fn is_landmark_kind_honors_profile_only_kinds() {
        let profile = Profile {
            landmark_kinds: vec!["local_function_declaration".into()],
            ..Profile::default()
        };
        // Not in the static union, but present in the profile → landmark.
        assert!(!DECLARATION_TYPES.contains(&"local_function_declaration"));
        assert!(is_landmark_kind("local_function_declaration", &profile));
    }

    // Static languages carry a profile whose `landmark_kinds` may omit kinds
    // that the built-in union still covers — the fallback must preserve them.
    #[test]
    fn is_landmark_kind_falls_back_to_static_union() {
        let empty = Profile::default();
        // `function_item` is in DECLARATION_TYPES but not in an empty profile.
        assert!(DECLARATION_TYPES.contains(&"function_item"));
        assert!(is_landmark_kind("function_item", &empty));
    }

    #[test]
    fn is_landmark_kind_rejects_unknown_kinds() {
        let empty = Profile::default();
        assert!(!is_landmark_kind("identifier", &empty));
        assert!(!is_landmark_kind("comment", &empty));
    }

    // End-to-end guard that the profile is actually threaded through path
    // building: a profile that promotes `block` (never in DECLARATION_TYPES)
    // to a landmark must flip `is_landmark` on the `block` path entry, while
    // the default (empty) profile leaves it non-landmark. This is the behavior
    // dynamic (WASM) grammars rely on for their manifest landmarks.
    #[test]
    fn build_structural_path_uses_profile_landmarks() {
        let source = b"fn outer() {\n    let x = 1;\n}\n";
        let mut parser = Parser::new(CodemarkLang::Rust).unwrap();
        let tree = parser.parse(source).unwrap();
        // Target the `let` statement so `block` is an ancestor in the path.
        let target = tree.root_node().descendant_for_byte_range(17, 27).unwrap();

        assert!(!DECLARATION_TYPES.contains(&"block"));

        let default_profile = Profile::default();
        let default_path = build_structural_path(target, source, &default_profile, None);
        let default_block = default_path.iter().find(|e| e.node_type == "block");
        assert!(default_block.is_some(), "expected a `block` entry in the path");
        assert!(
            !default_block.unwrap().is_landmark,
            "`block` must not be a landmark under the default profile"
        );

        let custom_profile = Profile { landmark_kinds: vec!["block".into()], ..Profile::default() };
        let custom_path = build_structural_path(target, source, &custom_profile, None);
        let custom_block = custom_path.iter().find(|e| e.node_type == "block").unwrap();
        assert!(
            custom_block.is_landmark,
            "`block` must become a landmark when the profile lists it"
        );
    }

    async fn parse_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/swift/{name}"));
        let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_declaration"
                && let Some(name_node) = node.child_by_field_name("name")
            {
                let text = &source[name_node.byte_range()];
                if text == name {
                    return Some((node.start_byte(), node.end_byte()));
                }
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in fixture"))
    }

    #[tokio::test]
    async fn generate_query_for_top_level_function() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let range = find_function_byte_range(&tree, &source, "createDefaultAuthService");
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_node_type, "function_declaration");
        assert_eq!(result.target_name.as_deref(), Some("createDefaultAuthService"));

        // Verify the generated query finds exactly the right node
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].node_text.contains("createDefaultAuthService"));
    }

    #[tokio::test]
    async fn generate_query_for_class_method() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let range = find_function_byte_range(&tree, &source, "validateToken");
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_node_type, "function_declaration");
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].node_text.contains("validateToken"));
    }

    #[tokio::test]
    async fn generate_query_for_private_method() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let range = find_function_byte_range(&tree, &source, "decode");
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn generate_query_for_extension_method() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let range = find_function_byte_range(&tree, &source, "invalidateCache");
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("invalidateCache"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn generated_query_round_trips() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        // For each function in the fixture, generate a query and verify it matches
        let functions = [
            "validateToken",
            "refreshToken",
            "decode",
            "encode",
            "checkPermission",
            "invalidateCache",
            "cacheSize",
            "createDefaultAuthService",
        ];

        for func_name in functions {
            let range = find_function_byte_range(&tree, &source, func_name);
            let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
            let matches =
                matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();

            assert!(
                !matches.is_empty(),
                "query for '{func_name}' returned no matches: {}",
                result.query
            );
            // Verify the match covers the original byte range
            let m = &matches[0];
            assert!(
                m.byte_range.0 <= range.0 && m.byte_range.1 >= range.1,
                "match for '{func_name}' doesn't cover original range"
            );
        }
    }

    // --- Rust tests ---

    async fn parse_rust_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/rust/{name}"));
        let mut parser = Parser::new(CodemarkLang::Rust).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_rust_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_item"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Rust fixture"))
    }

    #[tokio::test]
    async fn rust_top_level_function() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let range = find_rust_function_byte_range(&tree, &source, "create_default_auth_service");
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_node_type, "function_item");
        assert_eq!(result.target_name.as_deref(), Some("create_default_auth_service"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn rust_impl_method() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let range = find_rust_function_byte_range(&tree, &source, "decode");
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
        assert!(matches[0].node_text.contains("fn decode"));
    }

    #[tokio::test]
    async fn rust_trait_impl_method() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        // validate_token appears both in the trait and in the impl
        let range = find_rust_function_byte_range(&tree, &source, "validate_token");
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        // Should match at least 1 (may match trait decl too if query isn't precise enough)
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn rust_generic_function() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let range = find_rust_function_byte_range(&tree, &source, "validate_and_check");
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validate_and_check"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn rust_round_trips() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let functions = [
            "new",
            "decode",
            "encode",
            "check_permission",
            "create_default_auth_service",
            "validate_and_check",
        ];

        for func_name in functions {
            let range = find_rust_function_byte_range(&tree, &source, func_name);
            let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
            let matches =
                matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();

            assert!(
                !matches.is_empty(),
                "query for '{func_name}' returned no matches: {}",
                result.query
            );
        }
    }

    // --- TypeScript tests ---

    async fn parse_ts_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/typescript/{name}"));
        let mut parser = Parser::new(CodemarkLang::TypeScript).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_ts_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if (kind == "function_declaration" || kind == "method_definition")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in TS fixture"))
    }

    #[tokio::test]
    async fn ts_top_level_function() {
        let (tree, source) = parse_ts_fixture("auth_service.ts").await;
        let range = find_ts_function_byte_range(&tree, &source, "validateAndCheck");
        let cm_lang = CodemarkLang::TypeScript;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateAndCheck"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn ts_class_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts").await;
        let range = find_ts_function_byte_range(&tree, &source, "validateToken");
        let cm_lang = CodemarkLang::TypeScript;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn ts_private_method() {
        let (tree, source) = parse_ts_fixture("auth_service.ts").await;
        let range = find_ts_function_byte_range(&tree, &source, "decode");
        let cm_lang = CodemarkLang::TypeScript;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Python tests ---

    async fn parse_py_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/python/{name}"));
        let mut parser = Parser::new(CodemarkLang::Python).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_py_function_byte_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if kind == "function_definition"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(range) = search(child, source, name) {
                    return Some(range);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Python fixture"))
    }

    #[tokio::test]
    async fn py_top_level_function() {
        let (tree, source) = parse_py_fixture("auth_service.py").await;
        let range = find_py_function_byte_range(&tree, &source, "create_default_auth_service");
        let cm_lang = CodemarkLang::Python;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("create_default_auth_service"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn py_class_method() {
        let (tree, source) = parse_py_fixture("auth_service.py").await;
        let range = find_py_function_byte_range(&tree, &source, "validate_token");
        let cm_lang = CodemarkLang::Python;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn py_private_method() {
        let (tree, source) = parse_py_fixture("auth_service.py").await;
        let range = find_py_function_byte_range(&tree, &source, "_decode");
        let cm_lang = CodemarkLang::Python;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("_decode"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn py_decorated_function() {
        let (tree, source) = parse_py_fixture("auth_service.py").await;
        let range = find_py_function_byte_range(&tree, &source, "require_auth");
        let cm_lang = CodemarkLang::Python;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("require_auth"));

        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Go tests ---

    async fn parse_go_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/go/{name}"));
        let mut parser = Parser::new(CodemarkLang::Go).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_go_function_range(tree: &Tree, source: &str, func_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            let kind = node.kind();
            if (kind == "function_declaration" || kind == "method_declaration")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, func_name)
            .unwrap_or_else(|| panic!("function '{func_name}' not found in Go fixture"))
    }

    #[tokio::test]
    async fn go_free_function() {
        let (tree, source) = parse_go_fixture("auth_service.go").await;
        let range = find_go_function_range(&tree, &source, "CreateDefaultAuthService");
        let cm_lang = CodemarkLang::Go;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("CreateDefaultAuthService"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn go_method() {
        let (tree, source) = parse_go_fixture("auth_service.go").await;
        let range = find_go_function_range(&tree, &source, "ValidateToken");
        let cm_lang = CodemarkLang::Go;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn go_private_method() {
        let (tree, source) = parse_go_fixture("auth_service.go").await;
        let range = find_go_function_range(&tree, &source, "decode");
        let cm_lang = CodemarkLang::Go;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Java tests ---

    async fn parse_java_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/java/{name}"));
        let mut parser = Parser::new(CodemarkLang::Java).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_java_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if (node.kind() == "method_declaration" || node.kind() == "constructor_declaration")
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, method_name)
            .unwrap_or_else(|| panic!("method '{method_name}' not found in Java fixture"))
    }

    #[tokio::test]
    async fn java_method() {
        let (tree, source) = parse_java_fixture("AuthService.java").await;
        let range = find_java_range(&tree, &source, "validateToken");
        let cm_lang = CodemarkLang::Java;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn java_private_method() {
        let (tree, source) = parse_java_fixture("AuthService.java").await;
        let range = find_java_range(&tree, &source, "decode");
        let cm_lang = CodemarkLang::Java;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn java_static_method() {
        let (tree, source) = parse_java_fixture("AuthService.java").await;
        let range = find_java_range(&tree, &source, "createDefault");
        let cm_lang = CodemarkLang::Java;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("createDefault"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- C# tests ---

    async fn parse_csharp_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/csharp/{name}"));
        let mut parser = Parser::new(CodemarkLang::CSharp).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    fn find_csharp_range(tree: &Tree, source: &str, method_name: &str) -> (usize, usize) {
        fn search(node: Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "method_declaration"
                && let Some(name_node) = node.child_by_field_name("name")
                && &source[name_node.byte_range()] == name
            {
                return Some((node.start_byte(), node.end_byte()));
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(r) = search(child, source, name) {
                    return Some(r);
                }
            }
            None
        }
        search(tree.root_node(), source, method_name)
            .unwrap_or_else(|| panic!("method '{method_name}' not found in C# fixture"))
    }

    #[tokio::test]
    async fn csharp_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs").await;
        let range = find_csharp_range(&tree, &source, "ValidateToken");
        let cm_lang = CodemarkLang::CSharp;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("ValidateToken"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn csharp_private_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs").await;
        let range = find_csharp_range(&tree, &source, "Decode");
        let cm_lang = CodemarkLang::CSharp;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("Decode"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn csharp_static_method() {
        let (tree, source) = parse_csharp_fixture("AuthService.cs").await;
        let range = find_csharp_range(&tree, &source, "CreateDefault");
        let cm_lang = CodemarkLang::CSharp;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("CreateDefault"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Dart tests ---

    async fn parse_dart_fixture(name: &str) -> (Tree, String) {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/dart/{name}"));
        let mut parser = Parser::new(CodemarkLang::Dart).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        parser.parse_file(&fixture, &provider).await.unwrap()
    }

    #[tokio::test]
    async fn dart_top_level_function() {
        let (tree, source) = parse_dart_fixture("auth_service.dart").await;
        // function_signature has the name
        let offset = source.find("createDefaultAuthService").unwrap();
        let range = (offset, offset + 10);
        let cm_lang = CodemarkLang::Dart;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("createDefaultAuthService"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn dart_class_method() {
        let (tree, source) = parse_dart_fixture("auth_service.dart").await;
        let offset = source.find("Claims _decode").unwrap();
        let range = (offset, offset + 10);
        let cm_lang = CodemarkLang::Dart;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert!(!matches.is_empty(), "query:\n{}", result.query);
    }

    #[tokio::test]
    async fn dart_enum() {
        let (tree, source) = parse_dart_fixture("auth_service.dart").await;
        let offset = source.find("enum AuthError").unwrap();
        let range = (offset, offset + 10);
        let cm_lang = CodemarkLang::Dart;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();
        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(result.target_name.as_deref(), Some("AuthError"));
        let matches = matcher::run_query(&result.query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1, "query:\n{}", result.query);
    }

    // --- Range precision tests: method range should target method, not class ---

    #[tokio::test]
    async fn swift_exact_range_targets_method_not_class() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        // Get the exact byte range of validateToken
        let range = find_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "function_declaration",
            "should target function_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[tokio::test]
    async fn rust_exact_range_targets_method_not_impl() {
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let range = find_rust_function_byte_range(&tree, &source, "decode");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "function_item",
            "should target function_item, not impl_item"
        );
        assert_eq!(result.target_name.as_deref(), Some("decode"));
    }

    #[tokio::test]
    async fn ts_exact_range_targets_method_not_class() {
        let (tree, source) = parse_ts_fixture("auth_service.ts").await;
        let cm_lang = CodemarkLang::TypeScript;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let range = find_ts_function_byte_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "method_definition",
            "should target method_definition, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[tokio::test]
    async fn py_exact_range_targets_method_not_class() {
        let (tree, source) = parse_py_fixture("auth_service.py").await;
        let cm_lang = CodemarkLang::Python;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let range = find_py_function_byte_range(&tree, &source, "validate_token");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "function_definition",
            "should target function_definition, not class_definition"
        );
        assert_eq!(result.target_name.as_deref(), Some("validate_token"));
    }

    #[tokio::test]
    async fn go_exact_range_targets_method() {
        let (tree, source) = parse_go_fixture("auth_service.go").await;
        let cm_lang = CodemarkLang::Go;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let range = find_go_function_range(&tree, &source, "ValidateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration"
        );
    }

    #[tokio::test]
    async fn java_exact_range_targets_method_not_class() {
        let (tree, source) = parse_java_fixture("AuthService.java").await;
        let cm_lang = CodemarkLang::Java;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let range = find_java_range(&tree, &source, "validateToken");

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        assert_eq!(
            result.target_node_type, "method_declaration",
            "should target method_declaration, not class_declaration"
        );
        assert_eq!(result.target_name.as_deref(), Some("validateToken"));
    }

    #[tokio::test]
    async fn single_line_inside_method_targets_anchored_declaration() {
        // A single line inside a method should target the enclosing method
        let (tree, source) = parse_rust_fixture("auth_service.rs").await;
        let cm_lang = CodemarkLang::Rust;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        // Line 50 is inside the decode function body
        let line_50_start = source.lines().take(49).map(|l: &str| l.len() + 1).sum::<usize>();
        let line_50_end = line_50_start + source.lines().nth(49).unwrap_or("").len();

        let result =
            generate_query(&tree, source.as_bytes(), (line_50_start, line_50_end), &lang, profile)
                .unwrap();
        assert_eq!(
            result.target_node_type, "block",
            "single line inside method should target the tightest node (block)"
        );
    }

    // --- Sticky capture tests ---

    #[tokio::test]
    async fn generated_query_includes_sticky_captures() {
        let (tree, source) = parse_fixture("auth_service.swift").await;
        let range = find_function_byte_range(&tree, &source, "validateToken");
        let cm_lang = CodemarkLang::Swift;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();

        // Query should contain @sticky.class and @sticky.function (Swift uses function_declaration for both)
        assert!(
            result.query.contains("@sticky.class"),
            "query should have @sticky.class:\n{}",
            result.query
        );
        assert!(
            result.query.contains("@sticky.function"),
            "query should have @sticky.function:\n{}",
            result.query
        );
    }

    #[tokio::test]
    async fn generated_query_for_nested_class_has_sticky_captures() {
        let (tree, source) = parse_ts_fixture("auth_service.ts").await;
        let range = find_ts_function_byte_range(&tree, &source, "validateToken");
        let cm_lang = CodemarkLang::TypeScript;
        let lang = cm_lang.tree_sitter_language();
        let profile = cm_lang.profile();

        let result = generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();

        // Query should contain @sticky.class and @sticky.method (TypeScript distinguishes these)
        assert!(
            result.query.contains("@sticky.class"),
            "query should have @sticky.class:\n{}",
            result.query
        );
        assert!(
            result.query.contains("@sticky.method"),
            "query should have @sticky.method:\n{}",
            result.query
        );
    }
}
