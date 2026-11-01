//! Every generated query must compile against the grammar it was generated from.
//!
//! A query is "impossible" when it constrains a node kind in a way the grammar
//! can never satisfy — naming a field the kind does not define, or demanding two
//! of a child it has only one of. Tree-sitter rejects the *whole* query at
//! compile time, so no bookmark is created at all.
//!
//! This is the failure mode that is easiest to ship by accident, because it
//! depends on a grammar's hidden-node layout rather than on anything visible in
//! a profile. See https://github.com/DanielCardonaRojas/codemark/issues/239
//!
//! The sweeps at the bottom are the part that generalizes: they bookmark
//! *every* named node of a source file and count the queries that fail to
//! compile. Adding a language means adding one `sweep(...)` line, and it
//! inherits the whole check.
//!
//! Each sweep carries a `budget` of known failures attributable to
//! <https://github.com/DanielCardonaRojas/codemark/issues/263>, a distinct
//! pre-existing bug. The budget is asserted *exactly*, so fixing #263 forces
//! the number down rather than letting it rot upward.

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

// --- The generalizing sweep ------------------------------------------------

/// Bookmark every named node in a source file and count uncompilable queries.
///
/// Targeted tests only cover shapes someone already thought of, and this class
/// of bug lives in grammar layouts nobody inspected. Sweeping every node is what
/// turns "we fixed Lua assignments" into "no node in this file is
/// unbookmarkable".
///
/// `budget` is a ratchet, not a blessing. It is the number of targets still
/// failing for a **separate, pre-existing** reason: the generator can name an
/// ancestor *and* emit a positional pattern for the same single child, or emit
/// fields in an order the grammar forbids —
/// <https://github.com/DanielCardonaRojas/codemark/issues/263>. Those reproduce
/// with the #239 fix reverted. A budget keeps the sweep useful now (it fails on
/// any *new* uncompilable query) instead of being deleted for being red; lower
/// it as #263 is fixed, and the assertion below refuses a stale budget so it
/// cannot silently over-permit.
fn sweep(lang: &Language, source: &str, label: &str, budget: usize) {
    let mut parser = Parser::new(lang.clone()).expect("parser");
    let tree = parser.parse(source.as_bytes()).expect("parse");

    let mut cursor = tree.walk();
    let mut targets = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_named() && node.byte_range().len() > 1 {
            targets.push(node.byte_range());
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    assert!(!targets.is_empty(), "{label}: nothing to sweep");

    let ts_lang = parser.language().clone();
    let mut failures = Vec::new();
    for range in &targets {
        if let Err(msg) = compile_for(&tree, source, (range.start, range.end), &ts_lang) {
            // Ambiguous / non-unique targets are expected for arbitrary ranges;
            // only compilation failures are defects.
            if !msg.starts_with("@skip") {
                failures.push(format!("  bytes {}..{}: {msg}", range.start, range.end));
            }
        }
    }

    assert!(
        failures.len() <= budget,
        "{label}: {} of {} targets produced an uncompilable query, budget is {budget}.\n\
         A new one means a regression — see the failures below:\n{}",
        failures.len(),
        targets.len(),
        failures.join("\n")
    );
    assert_eq!(
        failures.len(),
        budget,
        "{label}: budget is {budget} but only {} targets fail. \
         Progress on #263 — lower the budget to {} to lock it in.",
        failures.len(),
        failures.len()
    );
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
    // 1 remaining: `function M.compute` names the ancestor
    // `name: (dot_index_expression)` and also emits it positionally (#263).
    sweep(&lang, source, "lua", 1);
}

/// The built-in languages, swept over compact snippets.
///
/// Short inline sources rather than the committed `auth_service.*` fixtures:
/// disambiguation runs a query per candidate tier, so sweeping ~500 targets per
/// language took minutes. These cover the same *shapes* — nested declarations,
/// visibility modifiers, fields, methods — in a few seconds.
///
/// Python reaching zero is the load-bearing part: the sweep is not uniformly
/// red, so the other counts are real defects rather than an artifact of
/// sweeping every node.
#[test]
fn sweep_builtin_snippets() {
    // Budgets measured against the #239 fix; every one is attributable to #263.
    let cases: &[(Language, &str, &str, usize)] = &[
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
            3,
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
            0,
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
            1,
        ),
    ];

    for (lang, label, source, budget) in cases {
        sweep(lang, source, label, *budget);
    }
}
