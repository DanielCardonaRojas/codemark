//! Every generated query must compile against the grammar it was generated from.
//!
//! A query is "impossible" when it constrains a node kind in a way the grammar
//! can never satisfy — naming a field the kind does not define, or demanding two
//! of a child it has only one of. Tree-sitter rejects the *whole* query at
//! compile time, so no bookmark is created at all.
//!
//! This is the failure mode that is easiest to ship by accident, because it
//! depends on a grammar's hidden-node layout rather than on anything visible in
//! a profile. Three causes have been found so far:
//!
//! | Cause | Issue | State |
//! |---|---|---|
//! | A field named on a kind that does not declare it | [#239] | fixed |
//! | Sibling patterns duplicated, or out of grammar order | [#263] | fixed |
//! | `init` emitted as a Swift node kind | [#265] | open |
//!
//! The sweeps at the bottom are the part that generalizes: they bookmark
//! *every* named node of a source file and collect the node kinds whose query
//! fails to compile. Adding a language means adding one `sweep(...)` line, and
//! it inherits the whole check.
//!
//! Each sweep carries the set of kinds still expected to fail, compared
//! exactly, so a newly broken kind cannot hide behind a newly fixed one and
//! fixing a bug forces the list down rather than letting it rot upward.
//!
//! [#239]: https://github.com/DanielCardonaRojas/codemark/issues/239
//! [#263]: https://github.com/DanielCardonaRojas/codemark/issues/263
//! [#265]: https://github.com/DanielCardonaRojas/codemark/issues/265

#![cfg(feature = "wasm")]

use codemark_core::error::Error;
use codemark_core::parser::languages::{DynamicLanguage, Language, Parser};
use codemark_core::parser::profile::Profile;
use codemark_core::query::generator::generate_query;
use std::path::PathBuf;
use std::sync::Arc;

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // root/
    p
}

/// Lua, loaded dynamically with an **empty** profile.
///
/// The empty profile is deliberate: #239 reproduces with `"profile": {}`, which
/// proves the bug is in the generator rather than in curated profile data. No
/// registry manifest could mask or fix it.
fn lua_dynamic() -> Option<Language> {
    let wasm = repo_root().join("tests/fixtures/grammars/lua/grammar.wasm");
    wasm.exists().then(|| {
        Language::Dynamic(Arc::new(DynamicLanguage {
            name: "lua".into(),
            extensions: vec!["lua".into()],
            wasm_path: wasm,
            profile: Profile::default(),
        }))
    })
}

/// Byte range of the first occurrence of `needle`.
fn range_of(source: &str, needle: &str) -> (usize, usize) {
    let start = source.find(needle).expect("needle not in source");
    (start, start + needle.len())
}

/// Generate a query and compile it, returning the query text on success.
///
/// `generate_query` compiles internally while disambiguating, so an impossible
/// pattern surfaces as `Error::TreeSitter` rather than as a bad return value.
/// Compiling again here guards the final text too, in case a later tier emits
/// something the disambiguation loop never ran.
fn generate_and_compile(
    lang: &Language,
    source: &str,
    range: (usize, usize),
) -> Result<String, String> {
    let mut parser = Parser::new(lang.clone()).map_err(|e| e.to_string())?;
    let tree = parser.parse(source.as_bytes()).map_err(|e| e.to_string())?;
    let ts_lang = parser.language().clone();
    compile_for(&tree, source, range, &ts_lang)
}

/// The body of [`generate_and_compile`], against an already-parsed tree.
///
/// The sweep calls this thousands of times; re-parsing per target made it take
/// minutes, which is the difference between a test people run and one they skip.
fn compile_for(
    tree: &tree_sitter::Tree,
    source: &str,
    range: (usize, usize),
    ts_lang: &tree_sitter::Language,
) -> Result<String, String> {
    let generated = generate_query(tree, source.as_bytes(), range, ts_lang, &Profile::default())
        .map_err(|e| match e {
            Error::TreeSitter(msg) => format!("generator emitted an uncompilable query: {msg}"),
            // Ambiguity and "no unique node" are legitimate outcomes for an
            // arbitrary target; only query *compilation* is under test here.
            other => format!("@skip {other}"),
        })?;

    tree_sitter::Query::new(ts_lang, &generated.query)
        .map_err(|e| format!("final query failed to compile: {e}\n{}", generated.query))?;

    Ok(generated.query)
}

// --- #239: the reported case, and the shape behind it -----------------------

/// The exact reproduction from the issue.
#[test]
fn assignment_inside_a_for_loop_is_bookmarkable() {
    let Some(lang) = lua_dynamic() else {
        eprintln!("skipping: lua grammar fixture not found");
        return;
    };
    let source = "for i, item in ipairs(t) do\n  total = total + item\nend\n";

    let query = generate_and_compile(&lang, source, range_of(source, "total = total + item"))
        .expect("bookmarking inside a Lua for loop must succeed");

    assert!(
        !query.contains("name:"),
        "`assignment_statement` has no `name` field; emitting one is the #239 bug:\n{query}"
    );
}

/// The `for` loop in #239 was incidental. Lua's `assignment_statement` keeps its
/// name under a fieldless `variable_list`, so the bug fires at file scope too —
/// worth pinning separately so a fix aimed only at loops cannot look complete.
#[test]
fn a_bare_assignment_is_bookmarkable() {
    let Some(lang) = lua_dynamic() else { return };
    let source = "total = 1\n";

    generate_and_compile(&lang, source, range_of(source, "total = 1"))
        .expect("bookmarking a top-level Lua assignment must succeed");
}

/// The name is still queried, just positionally.
///
/// Dropping the name would also compile, so a fix that merely deletes the field
/// constraint passes the tests above while quietly weakening every such
/// bookmark to a positional match that breaks when a sibling is inserted.
#[test]
fn a_nested_name_is_recovered_rather_than_dropped() {
    let Some(lang) = lua_dynamic() else { return };
    let source = "total = 1\n";

    let query = generate_and_compile(&lang, source, range_of(source, "total = 1")).expect("query");

    assert!(
        query.contains("(variable_list (identifier)"),
        "the name should be recovered through the fieldless wrapper:\n{query}"
    );
    assert!(
        query.contains("\"total\""),
        "the recovered name should still be asserted by predicate:\n{query}"
    );
}

/// A field valid *somewhere* in the grammar is not valid *everywhere*.
///
/// Lua declares `name` — on `variable_list`, not on `assignment_statement`.
/// Checking the grammar's global field table would pass this and still emit an
/// impossible pattern, so the generator has to ask per node kind. This test is
/// what distinguishes the two implementations.
#[test]
fn a_field_declared_elsewhere_in_the_grammar_is_not_assumed_valid_here() {
    let Some(lang) = lua_dynamic() else { return };
    let mut parser = Parser::new(lang.clone()).expect("wasm parser");
    let ts_lang = parser.language().clone();

    let declares_name_globally = (0..=u16::try_from(ts_lang.field_count()).unwrap_or(u16::MAX))
        .any(|id| ts_lang.field_name_for_id(id) == Some("name"));
    assert!(declares_name_globally, "precondition: Lua declares a `name` field");

    assert!(
        tree_sitter::Query::new(&ts_lang, "(assignment_statement name: (identifier) @x)").is_err(),
        "precondition: `name` on assignment_statement is impossible despite being declared"
    );

    let source = "total = 1\n";
    let tree = parser.parse(source.as_bytes()).expect("parse");
    let asg = tree.root_node().named_child(0).expect("assignment");
    assert_eq!(asg.kind(), "assignment_statement");
    assert!(
        asg.child_by_field_name("name").is_some(),
        "precondition: child_by_field_name sees through the hidden node, which is the trap"
    );

    // The preconditions above describe the trap; this is the generator refusing
    // to fall into it. Without this the test would pass against an
    // implementation that checks only the grammar's global field table.
    let query = generate_and_compile(&lang, source, range_of(source, "total = 1"))
        .expect("generator must not constrain a field the kind does not declare");
    assert!(
        !query.contains("name:"),
        "generator wrote a `name:` field on a kind that has none:\n{query}"
    );
}

/// Recovering the name must not duplicate structure the path already carries.
///
/// Dart nests `method_signature > function_signature [name=identifier]`. Naming
/// the intermediate there emits two `function_signature` children for a node
/// that has one — impossible for a different reason than the Lua case, and a
/// regression introduced by an over-eager first fix.
#[test]
fn dart_method_name_is_not_recovered_twice() {
    let fixture = repo_root().join("tests/fixtures/dart/auth_service.dart");
    if !fixture.exists() {
        eprintln!("skipping: dart fixture not found");
        return;
    }
    let source = std::fs::read_to_string(&fixture).expect("read dart fixture");

    let query = generate_and_compile(&Language::Dart, &source, range_of(&source, "Claims _decode"))
        .expect("bookmarking a Dart method must succeed");

    assert_eq!(
        query.matches("(function_signature").count(),
        1,
        "two sibling function_signature patterns cannot both match:\n{query}"
    );
}

// --- #263: ordered siblings and duplicated children ------------------------

/// Sibling patterns must follow the grammar's child order.
///
/// Dart puts the return type *before* the method name:
///
/// ```text
/// function_signature [ return_type=type_identifier  name=identifier  parameters=... ]
/// ```
///
/// so bookmarking the return type has to emit the target first. Emitting
/// `name:` ahead of it is rejected outright, which made every return type in a
/// Dart method unbookmarkable.
#[test]
fn a_name_after_the_target_is_emitted_after_it() {
    let source = "class C {\n  bool check(String role) {\n    return true;\n  }\n}\n";
    let query = generate_and_compile(&Language::Dart, source, range_of(source, "bool"))
        .expect("bookmarking a Dart return type must succeed");

    let target = query.find("@target").expect("target capture");
    let name = query.find("name:").expect("name pattern");
    assert!(
        target < name,
        "the grammar orders return_type before name, so the query must too:\n{query}"
    );
}

/// An ancestor must not name the very child the path descends into.
///
/// Rust's `impl_item` names itself through its `type` field, so bookmarking the
/// `Claims` in `impl Claims` asked for `type: (type_identifier)` *and* a
/// positional `(type_identifier) @target` — the same single child twice. The
/// path starts at the parent instead, which also yields the better anchor.
#[test]
fn an_impl_type_is_named_once() {
    let source = "impl Claims {\n    pub fn new() -> Self {\n        Self\n    }\n}\n";
    let query = generate_and_compile(&Language::Rust, source, range_of(source, "Claims"))
        .expect("bookmarking an impl's type must succeed");

    assert_eq!(
        query.matches("(type_identifier)").count(),
        1,
        "impl_item has one type child, so it may be named once:\n{query}"
    );
    assert!(query.contains("type:"), "the name should stay a field match:\n{query}");
}

/// The deeper form of the same duplication, where the named child is an
/// intermediate the path walks *through* rather than the target itself.
///
/// Lua's `function_declaration` names a `dot_index_expression`, and bookmarking
/// `compute` in `function M.compute(...)` descends through that same node.
#[test]
fn a_named_intermediate_on_the_path_is_not_also_named() {
    let Some(lang) = lua_dynamic() else {
        eprintln!("skipping: lua grammar fixture not found");
        return;
    };
    let source = "function M.compute(items)\n  return 1\nend\n";
    let query = generate_and_compile(&lang, source, range_of(source, "compute"))
        .expect("bookmarking a dotted Lua function name must succeed");

    assert_eq!(
        query.matches("(dot_index_expression").count(),
        1,
        "function_declaration has one dot_index_expression:\n{query}"
    );
}

/// A wrapper kind may not borrow its inner declaration's field name.
///
/// `decorated_definition [ -=decorator  definition=function_definition ]` has no
/// `name` field, but the name was taken from the inner definition *with its
/// field intact*, producing `(decorated_definition name: (identifier))`. Same
/// failure class as #239, reached only when the wrapper is itself the target.
#[test]
fn a_decorated_definition_does_not_claim_the_inner_name_field() {
    let source = "@app.route('/x')\ndef handler(req):\n    return 1\n";
    let mut parser = Parser::new(Language::Python).expect("parser");
    let tree = parser.parse(source.as_bytes()).expect("parse");
    let ts_lang = parser.language().clone();

    let decorated = tree.root_node().named_child(0).expect("first child");
    assert_eq!(decorated.kind(), "decorated_definition");
    assert!(
        child_by_field_is_absent(decorated, "name"),
        "precondition: decorated_definition declares no `name` field of its own"
    );

    let range = decorated.byte_range();
    let query = compile_for(&tree, source, (range.start, range.end), &ts_lang)
        .expect("bookmarking a decorated definition must succeed");
    assert!(
        !query.contains("decorated_definition\n  name:"),
        "the inner name field cannot be claimed by the wrapper:\n{query}"
    );
}

/// A wrapper's nested name must reach the identifier, not stop at a declarator.
///
/// `export const Foo = 1` names itself through `lexical_declaration >
/// variable_declarator > identifier`. Emitting only one level of nesting
/// captured the declarator and compared `Foo = 1` against `Foo`, so the query
/// **compiled and matched nothing** — invisible to every sweep here, which only
/// checks compilation. Hence the match-count assertion.
#[test]
fn an_exported_const_query_matches_its_declaration() {
    let source = "export const Foo = 1;\n";
    let mut parser = Parser::new(Language::TypeScript).expect("parser");
    let tree = parser.parse(source.as_bytes()).expect("parse");
    let ts_lang = parser.language().clone();

    let exported = tree.root_node().named_child(0).expect("first child");
    assert_eq!(exported.kind(), "export_statement");

    let range = exported.byte_range();
    let query = compile_for(&tree, source, (range.start, range.end), &ts_lang)
        .expect("bookmarking an exported const must succeed");

    assert_eq!(
        match_count(&query, &tree, source, &ts_lang),
        1,
        "the name capture must land on the identifier, not the declarator:\n{query}"
    );
}

/// How many nodes `query` matches in `tree` — the check compilation cannot make.
fn match_count(
    query: &str,
    tree: &tree_sitter::Tree,
    source: &str,
    ts_lang: &tree_sitter::Language,
) -> usize {
    use tree_sitter::StreamingIterator;

    let compiled = tree_sitter::Query::new(ts_lang, query).expect("query must compile");
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&compiled, tree.root_node(), source.as_bytes());
    let mut count = 0;
    while matches.next().is_some() {
        count += 1;
    }
    count
}

/// Whether no *direct* child of `node` sits under `field`.
fn child_by_field_is_absent(node: tree_sitter::Node, field: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .enumerate()
        .all(|(i, _)| u32::try_from(i).is_ok_and(|i| node.field_name_for_child(i) != Some(field)))
}

// --- The generalizing sweep ------------------------------------------------

/// Bookmark every named node in a source file and count uncompilable queries.
///
/// Targeted tests only cover shapes someone already thought of, and this class
/// of bug lives in grammar layouts nobody inspected. Sweeping every node is what
/// turns "we fixed Lua assignments" into "no node in this file is
/// unbookmarkable".
///
/// `expected` is a ratchet, not a blessing: the node kinds still failing for a
/// cause not yet fixed — today only Swift's `init` ([#265]) and a Rust
/// "Invalid capture name" case. Carrying them keeps the sweep useful now
/// instead of being deleted for being red.
///
/// [#265]: https://github.com/DanielCardonaRojas/codemark/issues/265
///
/// Compared as a **set of kinds**, not a count. A count lets a newly broken
/// target hide behind a newly fixed one; the set makes any change in *which*
/// kinds fail a test failure in its own right. Kinds rather than byte offsets so
/// that editing a snippet does not invalidate the baseline.
fn sweep(lang: &Language, source: &str, label: &str, expected: &[&str]) {
    let mut parser = Parser::new(lang.clone()).expect("parser");
    let tree = parser.parse(source.as_bytes()).expect("parse");

    let mut cursor = tree.walk();
    let mut targets = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() && node.byte_range().len() > 1 {
            targets.push((node.kind().to_string(), node.byte_range()));
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    assert!(!targets.is_empty(), "{label}: nothing to sweep");

    let ts_lang = parser.language().clone();
    let mut failed_kinds = std::collections::BTreeSet::new();
    let mut detail = Vec::new();
    for (kind, range) in &targets {
        if let Err(msg) = compile_for(&tree, source, (range.start, range.end), &ts_lang) {
            // Ambiguous / non-unique targets are expected for arbitrary ranges;
            // only compilation failures are defects.
            if !msg.starts_with("@skip") {
                failed_kinds.insert(kind.clone());
                detail.push(format!("  {kind} at {}..{}: {msg}", range.start, range.end));
            }
        }
    }

    let expected: std::collections::BTreeSet<String> =
        expected.iter().map(|s| (*s).to_string()).collect();
    if failed_kinds != expected {
        let new: Vec<_> = failed_kinds.difference(&expected).collect();
        let fixed: Vec<_> = expected.difference(&failed_kinds).collect();
        panic!(
            "{label}: the set of kinds producing uncompilable queries changed.\n\
             newly broken (regression): {new:?}\n\
             newly passing (fixed — remove from the expected list): {fixed:?}\n\
             all {} failures across {} targets:\n{}",
            detail.len(),
            targets.len(),
            detail.join("\n")
        );
    }
}

/// Lua exercises the dynamic-grammar path, where #239 was found.
#[test]
fn sweep_lua() {
    let Some(lang) = lua_dynamic() else {
        eprintln!("skipping: lua grammar fixture not found");
        return;
    };
    let source = "\
local M = {}
total = 0

local function add(a, b)
  return a + b
end

function M.compute(items)
  for i, item in ipairs(items) do
    total = total + item
    if item > 10 then
      print(item)
    end
  end
  while total > 0 do
    total = total - 1
  end
  return total
end

return M
";
    sweep(&lang, source, "lua", &[]);
}

/// The built-in languages, swept over compact snippets.
///
/// Short inline sources rather than the committed `auth_service.*` fixtures:
/// disambiguation runs a query per candidate tier, so sweeping ~500 targets per
/// language took minutes. These cover the same *shapes* — nested declarations,
/// visibility modifiers, fields, methods — in a few seconds.
///
/// Python having an empty list is the load-bearing part: the sweep is not
/// uniformly red, so the other entries are real defects rather than an artifact
/// of sweeping every node.
#[test]
fn sweep_builtin_snippets() {
    // Measured against the #239 fix; every entry is attributable to #263.
    let cases: &[(Language, &str, &str, &[&str])] = &[
        (
            Language::Rust,
            "rust",
            "\
pub struct Claims {
    pub subject: String,
}

impl Claims {
    pub fn new(subject: String) -> Self {
        Self { subject }
    }
}
",
            &[],
        ),
        (
            Language::Python,
            "python",
            "\
class Claims:
    def __init__(self, subject):
        self.subject = subject

    def check(self, role):
        if role in self.roles:
            return True
        return False
",
            &[],
        ),
        (
            Language::Dart,
            "dart",
            "\
class Claims {
  final String subject;
  Claims(this.subject);

  bool check(String role) {
    return roles.contains(role);
  }
}
",
            &[],
        ),
    ];

    for (lang, label, source, expected) in cases {
        sweep(lang, source, label, expected);
    }
}

/// The same sweep over the committed `auth_service.*` fixtures.
///
/// Slower than the snippets (~500 targets each, and disambiguation runs a query
/// per candidate tier) but it is the one that finds things: the snippets missed
/// the Python decorator bug entirely because no snippet had a decorator. Real
/// fixtures contain constructs nobody thinks to write by hand.
///
/// `#[ignore]` keeps `cargo test` quick; CI and anyone touching the generator
/// should run `cargo test -- --ignored`.
#[test]
#[ignore = "slow (~4 min): run with --ignored when changing the query generator"]
fn sweep_full_fixtures() {
    let cases: &[(Language, &str, &[&str])] = &[
        // Both remaining Rust failures are `use std::collections::...` paths,
        // where a predicate references a capture the query never emits
        // ("Invalid capture name name0") — a different bug from #263.
        (Language::Rust, "rust/auth_service.rs", &["identifier"]),
        // Swift emits `init` as a node kind, which its grammar does not define
        // ("Invalid node type init"), plus a handful of genuine impossible
        // patterns. Also distinct from #263; see #265.
        (
            Language::Swift,
            "swift/auth_service.swift",
            &[
                "array_type",
                "assignment",
                "dictionary_type",
                "directly_assignable_expression",
                "function_body",
                "init_declaration",
                "line_str_text",
                "line_string_literal",
                "navigation_expression",
                "navigation_suffix",
                "parameter",
                "self_expression",
                "simple_identifier",
                "statements",
                "type_annotation",
                "type_identifier",
                "user_type",
            ],
        ),
        (Language::Python, "python/auth_service.py", &[]),
        (Language::Dart, "dart/auth_service.dart", &[]),
    ];

    let mut swept = 0;
    for (lang, rel, expected) in cases {
        let fixture = repo_root().join("tests/fixtures").join(rel);
        if !fixture.exists() {
            eprintln!("skipping {rel}: fixture not found");
            continue;
        }
        let source = std::fs::read_to_string(&fixture).expect("read fixture");
        sweep(lang, &source, rel, expected);
        swept += 1;
    }
    assert!(swept > 0, "no fixtures were swept");
}
