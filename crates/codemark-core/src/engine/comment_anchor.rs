//! Anchoring comments to a node inside their bookmark.
//!
//! A comment may carry a tree-sitter query that points at part of its bookmark
//! (a call, a branch, a parameter). The query is *relative to the bookmark's
//! deepest landmark*: the innermost landmark node (function, method, impl,
//! class...) that contains the bookmark's target, or the target itself when no
//! landmark encloses it. A comment can reach anything inside that node and
//! nothing outside it.
//!
//! Resolution is two steps: the bookmark resolves first (with its own
//! healing), then the comment's query runs rooted at the landmark found from
//! the bookmark's resolved range. A comment therefore never outlives its
//! bookmark, and needs no healing logic of its own.
//!
//! Comments without a query are about the whole bookmark and are untouched by
//! this module.

use tree_sitter::{Language as TsLanguage, Node, Tree};

use crate::engine::hash;
use crate::error::{Error, Result};
use crate::parser::languages::{Language, Parser};
use crate::parser::profile::Profile;
use crate::query::{generator, matcher};

/// Longest landmark or snippet text kept in messages and anchor hints.
const SNIPPET_CHARS: usize = 80;

/// The stored form of a comment's anchor, as produced at creation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentAnchor {
    /// Tree-sitter query relative to the bookmark's deepest landmark; holds a
    /// `@target` capture.
    pub query: String,
    /// Hash of the target node's text, used to break ties when the query
    /// matches more than once inside the landmark.
    pub content_hash: String,
    /// Where the anchor pointed when it was made (`L12-L14: <first line>`),
    /// shown when the anchor no longer resolves.
    pub anchor_hint: String,
}

/// Where a comment's anchor lands in the current code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorState {
    /// No query: the comment is about the bookmark as a whole.
    Bookmark,
    /// The query matched inside the landmark.
    Anchored(AnchoredRange),
    /// The bookmark resolved but the comment's query matched nothing inside
    /// its landmark.
    Orphaned,
    /// The bookmark itself did not resolve, so there is no landmark to run
    /// the query in. The comment re-resolves once the bookmark heals.
    Suspended,
}

impl AnchorState {
    /// Stable lowercase name, used in JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorState::Bookmark => "bookmark",
            AnchorState::Anchored(_) => "anchored",
            AnchorState::Orphaned => "orphaned",
            AnchorState::Suspended => "suspended",
        }
    }
}

/// A resolved anchor's position in the current file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchoredRange {
    pub byte_range: (usize, usize),
    /// 0-indexed.
    pub start_line: usize,
    /// 0-indexed.
    pub end_line: usize,
    /// Whether the matched text still hashes to the stored `content_hash`.
    pub hash_matches: bool,
}

/// The bookmark's deepest landmark: the innermost landmark node enclosing (or
/// equal to) the node at `target_range`, or that node itself when no landmark
/// encloses it.
///
/// When several nested nodes share `target_range` exactly, the outermost is
/// taken as the target so the scope is never narrower than the bookmark.
pub fn deepest_landmark<'t>(
    tree: &'t Tree,
    target_range: (usize, usize),
    profile: &Profile,
) -> Node<'t> {
    let root = tree.root_node();
    let mut target = root.descendant_for_byte_range(target_range.0, target_range.1).unwrap_or(root);
    while let Some(parent) = target.parent()
        && parent.start_byte() == target.start_byte()
        && parent.end_byte() == target.end_byte()
    {
        target = parent;
    }

    let mut current = Some(target);
    while let Some(node) = current {
        if generator::is_scope_landmark(node, profile) {
            return node;
        }
        current = node.parent();
    }
    target
}

/// Generate the anchor for a comment on `comment_range`, inside the bookmark
/// whose resolved target spans `bookmark_range`.
///
/// Errors with [`Error::CommentOutOfScope`] when `comment_range` is not
/// inside the bookmark's deepest landmark.
pub fn generate_comment_anchor(
    tree: &Tree,
    source: &str,
    language: &TsLanguage,
    profile: &Profile,
    bookmark_range: (usize, usize),
    comment_range: (usize, usize),
) -> Result<CommentAnchor> {
    let landmark = deepest_landmark(tree, bookmark_range, profile);
    // A line selection carries its indentation and newline; judge the code
    // itself, or the last statement of a function would fall "outside" it.
    let comment_range = trim_whitespace(source, comment_range);
    if comment_range.0 < landmark.start_byte() || comment_range.1 > landmark.end_byte() {
        tracing::debug!(
            target: "codemark::comments",
            landmark = landmark.kind(),
            ?comment_range,
            "comment range outside the bookmark's deepest landmark"
        );
        return Err(Error::CommentOutOfScope {
            landmark: first_line(&source[landmark.byte_range()]),
            start_line: landmark.start_position().row + 1,
            end_line: landmark.end_position().row + 1,
        });
    }

    let generated = generator::generate_scoped_query(
        tree,
        source.as_bytes(),
        comment_range,
        landmark,
        language,
        profile,
    )?;
    let (start, end) = generated.byte_range;
    let text = &source[start..end];
    let start_line = line_of(source, start);
    let end_line = line_of(source, end);
    tracing::debug!(
        target: "codemark::comments",
        landmark = landmark.kind(),
        target = %generated.target_node_type,
        query = %generated.query,
        "generated comment anchor"
    );
    Ok(CommentAnchor {
        query: generated.query,
        content_hash: hash::content_hash(text),
        anchor_hint: format!("L{}-L{}: {}", start_line + 1, end_line + 1, first_line(text)),
    })
}

/// Resolve a comment's anchor against the current code.
///
/// `bookmark_range` is the bookmark's resolved target, or `None` when the
/// bookmark failed to resolve. `query` and `content_hash` are the comment's
/// stored anchor fields.
pub fn resolve_comment_anchor(
    tree: &Tree,
    source: &str,
    language: &TsLanguage,
    profile: &Profile,
    bookmark_range: Option<(usize, usize)>,
    query: Option<&str>,
    content_hash: Option<&str>,
) -> AnchorState {
    let Some(query) = query else {
        return AnchorState::Bookmark;
    };
    let Some(bookmark_range) = bookmark_range else {
        return AnchorState::Suspended;
    };
    let landmark = deepest_landmark(tree, bookmark_range, profile);
    let matches = match matcher::run_query_in(query, landmark, source.as_bytes(), language) {
        Ok(matches) => matches,
        Err(e) => {
            tracing::debug!(target: "codemark::comments", error = %e, "comment query failed to run");
            return AnchorState::Orphaned;
        }
    };

    let hashed = |m: &matcher::MatchResult| hash::content_hash(&m.node_text);
    let chosen = match matches.len() {
        0 => None,
        1 => matches.first(),
        _ => content_hash
            .and_then(|stored| matches.iter().find(|m| hashed(m) == stored))
            .or_else(|| matches.first()),
    };
    tracing::debug!(
        target: "codemark::comments",
        landmark = landmark.kind(),
        matches = matches.len(),
        "resolved comment anchor"
    );
    match chosen {
        Some(m) => AnchorState::Anchored(AnchoredRange {
            byte_range: m.byte_range,
            start_line: m.start_point.0,
            end_line: m.end_point.0,
            hash_matches: content_hash.is_some_and(|stored| hashed(m) == stored),
        }),
        None => AnchorState::Orphaned,
    }
}

/// [`generate_comment_anchor`] for callers holding source text and a
/// codemark [`Language`] rather than a parsed tree.
pub fn generate_comment_anchor_from_source(
    source: &str,
    language: &Language,
    bookmark_range: (usize, usize),
    comment_range: (usize, usize),
) -> Result<CommentAnchor> {
    let mut parser = Parser::new(language.clone())?;
    let tree = parser.parse(source.as_bytes())?;
    let ts_language = parser.language().clone();
    generate_comment_anchor(
        &tree,
        source,
        &ts_language,
        language.profile(),
        bookmark_range,
        comment_range,
    )
}

/// Resolve many anchors in one file with a single parse. Each entry is a
/// comment's `(query, content_hash)`; the result is in the same order.
pub fn resolve_comment_anchors_from_source(
    source: &str,
    language: &Language,
    bookmark_range: Option<(usize, usize)>,
    anchors: &[(Option<&str>, Option<&str>)],
) -> Result<Vec<AnchorState>> {
    if anchors.iter().all(|(query, _)| query.is_none()) {
        return Ok(vec![AnchorState::Bookmark; anchors.len()]);
    }
    let mut parser = Parser::new(language.clone())?;
    let tree = parser.parse(source.as_bytes())?;
    let ts_language = parser.language().clone();
    Ok(anchors
        .iter()
        .map(|(query, hash)| {
            resolve_comment_anchor(
                &tree,
                source,
                &ts_language,
                language.profile(),
                bookmark_range,
                *query,
                *hash,
            )
        })
        .collect())
}

/// `range` without leading or trailing whitespace. A blank range collapses
/// to its start.
fn trim_whitespace(source: &str, (mut start, mut end): (usize, usize)) -> (usize, usize) {
    let bytes = source.as_bytes();
    end = end.min(bytes.len());
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (start, end)
}

/// 0-indexed line holding byte `offset`.
fn line_of(source: &str, offset: usize) -> usize {
    source.as_bytes()[..offset.min(source.len())].iter().filter(|b| **b == b'\n').count()
}

/// First non-blank line of `text`, trimmed and cut to [`SNIPPET_CHARS`].
fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > SNIPPET_CHARS {
        let cut: String = line.chars().take(SNIPPET_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::grammar;

    /// A bookmark on one `match` arm inside `parse`, with a sibling method
    /// and duplicated statements to force scoping and disambiguation.
    const RUST: &str = r#"struct Parser;

impl Parser {
    fn parse(&self, input: &str) -> usize {
        let tokens = lex(input);
        match tokens {
            0 => helper(1),
            _ => helper(2),
        }
    }

    fn other(&self) -> usize {
        let tokens = lex("x");
        helper(1)
    }
}

fn lex(s: &str) -> usize { s.len() }
fn helper(n: usize) -> usize { n }
"#;

    fn range_of(source: &str, needle: &str) -> (usize, usize) {
        let start = source.find(needle).unwrap_or_else(|| panic!("{needle:?} not in source"));
        (start, start + needle.len())
    }

    fn nth_range_of(source: &str, needle: &str, n: usize) -> (usize, usize) {
        let start = source
            .match_indices(needle)
            .nth(n)
            .unwrap_or_else(|| panic!("{needle:?} #{n} not in source"))
            .0;
        (start, start + needle.len())
    }

    fn generate(source: &str, bookmark: &str, comment: (usize, usize)) -> Result<CommentAnchor> {
        generate_comment_anchor_from_source(
            source,
            &grammar("rust"),
            range_of(source, bookmark),
            comment,
        )
    }

    fn resolve(
        source: &str,
        bookmark: Option<(usize, usize)>,
        anchor: &CommentAnchor,
    ) -> AnchorState {
        resolve_comment_anchors_from_source(
            source,
            &grammar("rust"),
            bookmark,
            &[(Some(anchor.query.as_str()), Some(anchor.content_hash.as_str()))],
        )
        .unwrap()
        .remove(0)
    }

    fn anchored_text<'s>(source: &'s str, state: &AnchorState) -> &'s str {
        match state {
            AnchorState::Anchored(r) => &source[r.byte_range.0..r.byte_range.1],
            other => panic!("expected an anchored comment, got {other:?}"),
        }
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn deepest_landmark_is_the_enclosing_function() {
        let source = RUST;
        let mut parser = Parser::new(grammar("rust")).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let arm = range_of(source, "0 => helper(1)");
        let landmark = deepest_landmark(&tree, arm, grammar("rust").profile());
        assert_eq!(landmark.kind(), "function_item");
        assert!(source[landmark.byte_range()].starts_with("fn parse"));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn landmark_bookmark_is_its_own_scope() {
        let source = RUST;
        let mut parser = Parser::new(grammar("rust")).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();
        let fn_start = source.find("fn other").unwrap();
        let fn_end = source[fn_start..].find("\n    }\n").unwrap() + fn_start + 6;
        let landmark = deepest_landmark(&tree, (fn_start, fn_end), grammar("rust").profile());
        assert!(source[landmark.byte_range()].starts_with("fn other"));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn anchors_a_node_outside_the_target_but_inside_the_landmark() {
        let source = RUST;
        // Bookmark the match arm; comment on the `let tokens` line above it.
        let comment = range_of(source, "let tokens = lex(input);");
        let anchor = generate(source, "0 => helper(1)", comment).unwrap();
        assert!(anchor.query.contains("@target"), "{}", anchor.query);
        assert!(anchor.anchor_hint.starts_with("L5-L5: let tokens"), "{}", anchor.anchor_hint);

        let state = resolve(source, Some(range_of(source, "0 => helper(1)")), &anchor);
        assert_eq!(anchored_text(source, &state), "let tokens = lex(input);");
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn rejects_a_comment_outside_the_landmark() {
        let source = RUST;
        // `helper(1)` inside the sibling method `other`.
        let sibling = nth_range_of(source, "helper(1)", 1);
        let err = generate(source, "0 => helper(1)", sibling).unwrap_err();
        match err {
            Error::CommentOutOfScope { landmark, start_line, end_line } => {
                assert!(landmark.starts_with("fn parse"), "{landmark}");
                assert_eq!((start_line, end_line), (4, 10));
            }
            other => panic!("expected CommentOutOfScope, got {other:?}"),
        }
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn query_is_unique_within_the_landmark_only() {
        let source = RUST;
        // `let tokens = lex(...)` appears in both methods; scoped to `parse`
        // the query must resolve to the one in `parse`.
        let comment = range_of(source, "let tokens = lex(input);");
        let anchor = generate(source, "fn parse", comment).unwrap();
        let state = resolve(source, Some(range_of(source, "fn parse")), &anchor);
        assert_eq!(anchored_text(source, &state), "let tokens = lex(input);");
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn follows_the_bookmark_when_code_moves() {
        let source = RUST;
        let anchor = generate(source, "0 => helper(1)", range_of(source, "helper(2)")).unwrap();

        // Insert a new method above `parse`: line numbers shift, structure holds.
        let moved = source.replacen(
            "impl Parser {\n",
            "impl Parser {\n    fn new() -> Self { Parser }\n\n",
            1,
        );
        let state = resolve(&moved, Some(range_of(&moved, "0 => helper(1)")), &anchor);
        assert_eq!(anchored_text(&moved, &state), "helper(2)");
        match state {
            AnchorState::Anchored(r) => {
                assert!(r.hash_matches);
                assert_eq!(r.start_line, 9);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn orphaned_when_the_target_is_gone() {
        let source = RUST;
        let anchor =
            generate(source, "0 => helper(1)", range_of(source, "let tokens = lex(input);"))
                .unwrap();
        let edited = source.replacen("        let tokens = lex(input);\n", "", 1).replacen(
            "match tokens",
            "match lex(input)",
            1,
        );
        let state = resolve(&edited, Some(range_of(&edited, "0 => helper(1)")), &anchor);
        assert_eq!(state, AnchorState::Orphaned);
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn suspended_when_the_bookmark_does_not_resolve() {
        let source = RUST;
        let anchor = generate(source, "0 => helper(1)", range_of(source, "helper(2)")).unwrap();
        assert_eq!(resolve(source, None, &anchor), AnchorState::Suspended);
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn no_query_is_a_whole_bookmark_comment() {
        let states = resolve_comment_anchors_from_source(
            RUST,
            &grammar("rust"),
            None,
            &[(None, None), (None, None)],
        )
        .unwrap();
        assert_eq!(states, vec![AnchorState::Bookmark, AnchorState::Bookmark]);
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn typescript_method_scope() {
        let source = "class Cart {\n  total(items: number[]): number {\n    const sum = items.reduce((a, b) => a + b, 0);\n    return sum;\n  }\n\n  count(items: number[]): number {\n    const sum = items.length;\n    return sum;\n  }\n}\n";
        let lang = grammar("typescript");
        let bookmark = range_of(source, "return sum;");
        let comment = range_of(source, "const sum = items.reduce((a, b) => a + b, 0);");
        let anchor = generate_comment_anchor_from_source(source, &lang, bookmark, comment).unwrap();
        let state = resolve_comment_anchors_from_source(
            source,
            &lang,
            Some(bookmark),
            &[(Some(anchor.query.as_str()), Some(anchor.content_hash.as_str()))],
        )
        .unwrap()
        .remove(0);
        assert!(anchored_text(source, &state).starts_with("const sum = items.reduce"));

        // `count`'s body is outside `total`.
        let outside = nth_range_of(source, "return sum;", 1);
        assert!(matches!(
            generate_comment_anchor_from_source(source, &lang, bookmark, outside),
            Err(Error::CommentOutOfScope { .. })
        ));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn python_function_scope() {
        let source = "def load(path):\n    data = read(path)\n    if not data:\n        return None\n    return parse(data)\n\n\ndef save(path, data):\n    write(path, data)\n";
        let lang = grammar("python");
        let bookmark = range_of(source, "return parse(data)");
        let comment = range_of(source, "if not data:\n        return None");
        let anchor = generate_comment_anchor_from_source(source, &lang, bookmark, comment).unwrap();
        let state = resolve_comment_anchors_from_source(
            source,
            &lang,
            Some(bookmark),
            &[(Some(anchor.query.as_str()), Some(anchor.content_hash.as_str()))],
        )
        .unwrap()
        .remove(0);
        assert!(anchored_text(source, &state).starts_with("if not data"));
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn whole_line_selections_of_a_last_statement_stay_in_scope() {
        // A line range includes indentation and the newline, which run past
        // the end of the function node for its last statement.
        let source = "def load(path):\n    data = read(path)\n    return parse(data)\n\ndef other():\n    pass\n";
        let lang = grammar("python");
        let bookmark = range_of(source, "data = read(path)");
        let last_line_start = source.find("    return parse").unwrap();
        let last_line_end = source.find("\n\ndef other").unwrap() + 1;
        let anchor = generate_comment_anchor_from_source(
            source,
            &lang,
            bookmark,
            (last_line_start, last_line_end),
        )
        .unwrap();
        assert!(anchor.anchor_hint.contains("return parse(data)"), "{}", anchor.anchor_hint);
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn look_alike_calls_keep_the_selected_one() {
        // `helper(1)` and `helper(2)` share a callee, so no query for the
        // call is unique; the anchor must still be the selected call.
        let source = "fn f() {\n    helper(1);\n    helper(2);\n}\nfn helper(n: u8) {}\n";
        let comment = range_of(source, "helper(2)");
        let anchor = generate(source, "helper(1)", comment).unwrap();
        assert!(anchor.anchor_hint.ends_with("helper(2)"), "{}", anchor.anchor_hint);
        assert_eq!(anchor.content_hash, hash::content_hash("helper(2)"));

        let state = resolve(source, Some(range_of(source, "helper(1)")), &anchor);
        assert_eq!(anchored_text(source, &state), "helper(2)");
    }
}
