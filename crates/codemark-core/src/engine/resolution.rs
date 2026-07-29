use std::path::Path;

use tree_sitter::{Language, Tree};

use crate::engine::bookmark::{Bookmark, ResolutionMethod};
use crate::engine::hash;
use crate::error::Result;
use crate::git::context as git_context;
use crate::parser::languages::ParseCache;
use crate::query::{matcher, relaxer};

/// Lightweight result from on-the-fly resolution, used for live previews
/// without persisting to the database.
#[derive(Debug, Clone)]
pub struct TransientResolution {
    pub method: ResolutionMethod,
    pub file_path: String,
    /// 0-indexed start line (from tree-sitter Point.row)
    pub start_line: usize,
    /// 0-indexed end line (from tree-sitter Point.row)
    pub end_line: usize,
    pub matched_text: String,
    pub content_hash: String,
    pub hash_matches: bool,
    pub breadcrumbs: Vec<crate::engine::breadcrumbs::Breadcrumb>,
}

/// Simplified health status derived from live resolution, for UI display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveUIStatus {
    /// Exact match with hash match — code is unchanged.
    Healthy,
    /// Relaxed/HashFallback match, or hash mismatch — code has moved or changed.
    Drifted,
    /// Resolution failed — code not found.
    Broken,
}

impl TransientResolution {
    /// Derive a [`LiveUIStatus`] from the resolution method and hash match.
    pub fn live_status(&self) -> LiveUIStatus {
        match self.method {
            ResolutionMethod::Exact if self.hash_matches => LiveUIStatus::Healthy,
            ResolutionMethod::Failed => LiveUIStatus::Broken,
            _ => LiveUIStatus::Drifted,
        }
    }
}

/// Resolve a bookmark on-the-fly for live preview, without persisting anything.
///
/// Accepts the codemark `Language` for logging; the tree-sitter handle comes
/// from the parse cache (works for both static and WASM-loaded grammars).
/// On file-not-found the error is propagated so the caller can fall back.
pub async fn resolve_transient(
    bookmark: &Bookmark,
    cache: &mut ParseCache,
    language: crate::parser::languages::Language,
    db_path: &Path,
    provider: &dyn crate::vfs::FileProvider,
) -> Result<TransientResolution> {
    tracing::debug!(
        target: "codemark::resolution",
        bookmark_id = %bookmark.id,
        file_path = %bookmark.file_path,
        language = %language,
        "starting transient resolution"
    );
    let ts_lang = cache.language().clone();
    let result = resolve(bookmark, cache, &ts_lang, db_path, provider).await?;
    tracing::debug!(
        target: "codemark::resolution",
        bookmark_id = %bookmark.id,
        method = ?result.method,
        start_line = result.start_line,
        end_line = result.end_line,
        hash_matches = result.hash_matches,
        "transient resolution complete"
    );
    Ok(TransientResolution {
        method: result.method,
        file_path: result.file_path,
        start_line: result.start_line,
        end_line: result.end_line,
        matched_text: result.matched_text,
        content_hash: result.content_hash,
        hash_matches: result.hash_matches,
        breadcrumbs: result.breadcrumbs,
    })
}

/// The result of resolving a single bookmark.
#[derive(Debug)]
pub struct ResolutionResult {
    pub method: ResolutionMethod,
    pub file_path: String,
    pub byte_range: (usize, usize),
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub matched_text: String,
    pub content_hash: String,
    pub hash_matches: bool,
    /// Structural ancestors for sticky headers.
    pub breadcrumbs: Vec<crate::engine::breadcrumbs::Breadcrumb>,
}

impl ResolutionResult {
    /// Capture exact snapshot lines from the resolved range.
    /// New: Captures ONLY the lines within target_range (no padding).
    /// The sticky headers (breadcrumbs) provide structural context instead.
    pub fn capture_preview(&self, _source: &str, _padding: usize) -> String {
        // Return the exact matched text - no padding
        // The padding parameter is kept for API compatibility but ignored
        self.matched_text.clone()
    }
}

/// Resolve a single bookmark against the current state of its file.
#[allow(clippy::collapsible_if)]
pub async fn resolve(
    bookmark: &Bookmark,
    cache: &mut ParseCache,
    language: &Language,
    db_path: &Path,
    provider: &dyn crate::vfs::FileProvider,
) -> Result<ResolutionResult> {
    // Resolve relative path to absolute for file reading
    let path = git_context::resolve_bookmark_file_path(&bookmark.file_path, db_path)?;
    let (tree, source) = cache.get_or_parse(&path, provider).await?;
    let source_bytes = source.as_bytes();

    // Tier 1: Exact query
    if let Ok(matches) = matcher::run_query(&bookmark.query, tree, source_bytes, language) {
        if std::env::var("CODEMARK_DEBUG_QUERY").is_ok() {
            eprintln!(
                "DEBUG: resolve: Tier 1 matches={} query=\n{}",
                matches.len(),
                bookmark.query
            );
        }
        if let Some(result) = pick_match(&matches, bookmark, tree, source, ResolutionMethod::Exact)
        {
            return Ok(result);
        }
    }

    // Tier 2: Relaxed query
    if let Ok(relaxed) = relaxer::relax_query(&bookmark.query) {
        if let Ok(matches) = matcher::run_query(&relaxed, tree, source_bytes, language) {
            if let Some(result) =
                pick_match(&matches, bookmark, tree, source, ResolutionMethod::Relaxed)
            {
                return Ok(result);
            }
        }
    }

    // Tier 3: Minimal query
    if let Ok(minimal) = relaxer::minimize_query(&bookmark.query) {
        if let Ok(matches) = matcher::run_query(&minimal, tree, source_bytes, language) {
            if let Some(result) =
                pick_match(&matches, bookmark, tree, source, ResolutionMethod::Relaxed)
            {
                return Ok(result);
            }
        }
    }

    // Tier 4: Hash fallback — walk all named nodes
    if let Some(ref stored_hash) = bookmark.content_hash {
        let root = tree.root_node();
        if let Some(result) =
            hash_fallback_walk(tree, root, source_bytes, stored_hash, bookmark, language)
        {
            return Ok(result);
        }
    }

    // Failed
    Ok(ResolutionResult {
        method: ResolutionMethod::Failed,
        file_path: bookmark.file_path.clone(),
        byte_range: (0, 0),
        start_line: 0,
        start_col: 0,
        end_line: 0,
        matched_text: String::new(),
        content_hash: String::new(),
        hash_matches: false,
        breadcrumbs: Vec::new(),
    })
}

/// Pick the best match from a list — single match or disambiguate by hash.
fn pick_match(
    matches: &[matcher::MatchResult],
    bookmark: &Bookmark,
    tree: &tree_sitter::Tree,
    source: &str,
    method: ResolutionMethod,
) -> Option<ResolutionResult> {
    if matches.len() == 1 {
        let m = &matches[0];
        let ch = hash::content_hash(&m.node_text);
        let hash_matches = bookmark.content_hash.as_deref() == Some(ch.as_str());
        let breadcrumbs = extract_breadcrumbs_from_match(m, tree, source, bookmark);

        return Some(ResolutionResult {
            method,
            file_path: bookmark.file_path.clone(),
            byte_range: m.byte_range,
            start_line: m.start_point.0,
            start_col: m.start_point.1,
            end_line: m.end_point.0,
            matched_text: m.node_text.clone(),
            content_hash: ch,
            hash_matches,
            breadcrumbs,
        });
    }

    // Multiple matches — disambiguate by content hash
    if let Some(ref stored_hash) = bookmark.content_hash {
        for m in matches {
            let ch = hash::content_hash(&m.node_text);
            if ch == *stored_hash {
                let breadcrumbs = extract_breadcrumbs_from_match(m, tree, source, bookmark);
                return Some(ResolutionResult {
                    method,
                    file_path: bookmark.file_path.clone(),
                    byte_range: m.byte_range,
                    start_line: m.start_point.0,
                    start_col: m.start_point.1,
                    end_line: m.end_point.0,
                    matched_text: m.node_text.clone(),
                    content_hash: ch,
                    hash_matches: true,
                    breadcrumbs,
                });
            }
        }
    }

    None
}

fn extract_breadcrumbs_from_match(
    m: &matcher::MatchResult,
    tree: &tree_sitter::Tree,
    source: &str,
    bookmark: &Bookmark,
) -> Vec<crate::engine::breadcrumbs::Breadcrumb> {
    // Try to extract from sticky captures first
    if !m.captures.is_empty() {
        let sticky_breadcrumbs = extract_breadcrumbs_from_captures(&m.captures, source);
        if !sticky_breadcrumbs.is_empty() {
            return sticky_breadcrumbs;
        }
    }

    // Fallback to AST walking for legacy queries
    use std::str::FromStr;
    let lang = crate::parser::languages::Language::from_str(&bookmark.language)
        .unwrap_or(crate::parser::languages::Language::Rust);

    if let Some(target_node) =
        tree.root_node().descendant_for_byte_range(m.byte_range.0, m.byte_range.1)
    {
        crate::engine::breadcrumbs::extract_breadcrumbs(target_node, source, lang, 3)
    } else {
        Vec::new()
    }
}

/// Extract breadcrumbs from sticky captures.
fn extract_breadcrumbs_from_captures(
    captures: &[(String, (usize, usize), usize)],
    source: &str,
) -> Vec<crate::engine::breadcrumbs::Breadcrumb> {
    let mut breadcrumbs = Vec::new();

    for (capture_name, _byte_range, line) in captures {
        if capture_name.starts_with("sticky.") {
            let line_text = source.lines().nth(*line).unwrap_or("").trim_end();
            breadcrumbs.push(crate::engine::breadcrumbs::Breadcrumb {
                line: line + 1,
                text: line_text.to_string(),
            });
        }
    }

    breadcrumbs
}

/// Walk all named nodes looking for a hash match.
fn hash_fallback_walk(
    _tree: &Tree,
    node: tree_sitter::Node,
    source_bytes: &[u8],
    stored_hash: &str,
    bookmark: &Bookmark,
    _language: &Language,
) -> Option<ResolutionResult> {
    if node.is_named() {
        let text = std::str::from_utf8(&source_bytes[node.byte_range()]).unwrap_or("");
        let ch = hash::content_hash(text);
        if ch == stored_hash {
            let source_str = std::str::from_utf8(source_bytes).unwrap_or("");
            use std::str::FromStr;
            let lang_enum = crate::parser::languages::Language::from_str(&bookmark.language)
                .unwrap_or(crate::parser::languages::Language::Rust);
            let breadcrumbs =
                crate::engine::breadcrumbs::extract_breadcrumbs(node, source_str, lang_enum, 3);

            return Some(ResolutionResult {
                method: ResolutionMethod::HashFallback,
                file_path: bookmark.file_path.clone(),
                byte_range: (node.start_byte(), node.end_byte()),
                start_line: node.start_position().row,
                start_col: node.start_position().column,
                end_line: node.end_position().row,
                matched_text: text.to_string(),
                content_hash: ch,
                hash_matches: true,
                breadcrumbs,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(result) =
            hash_fallback_walk(_tree, child, source_bytes, stored_hash, bookmark, _language)
        {
            return Some(result);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, BookmarkHealth};
    use crate::parser::languages::Language as CodemarkLang;
    use crate::query::generator as qgen;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/fixtures/swift/{name}"))
    }

    async fn create_bookmark_for_function(file: &str, func_name: &str) -> (Bookmark, ParseCache) {
        let path = fixture_path(file);
        let mut cache = ParseCache::new(CodemarkLang::Swift).unwrap();
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let profile = CodemarkLang::Swift.profile();

        let provider = crate::vfs::LocalFileProvider;
        let (tree, source) = cache.get_or_parse(&path, &provider).await.unwrap();
        let range = find_function_range(tree, source, func_name);
        let generated =
            qgen::generate_query(tree, source.as_bytes(), range, &lang, profile).unwrap();
        let ch = hash::content_hash(&source[range.0..range.1]);

        let bm = Bookmark {
            id: "test-bm".to_string(),
            query: generated.query,
            language: "swift".to_string(),
            file_path: path.to_string_lossy().to_string(),
            content_hash: Some(ch),
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            current_resolution_id: None,
            repo_id: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        (bm, cache)
    }

    fn find_function_range(tree: &tree_sitter::Tree, source: &str, name: &str) -> (usize, usize) {
        fn search(node: tree_sitter::Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_declaration"
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
        search(tree.root_node(), source, name).unwrap()
    }

    #[tokio::test]
    async fn resolve_exact_match() {
        let (bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        let lang = CodemarkLang::Swift.tree_sitter_language();
        // For tests, use a dummy db path - the bookmark stores absolute paths from fixture_path
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        assert_eq!(result.method, ResolutionMethod::Exact);
        assert!(result.hash_matches);
        assert!(result.matched_text.contains("validateToken"));
    }

    #[tokio::test]
    async fn resolve_exact_with_hash_mismatch() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        // Corrupt the stored hash
        bm.content_hash = Some("sha256:0000000000000000".to_string());
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        assert_eq!(result.method, ResolutionMethod::Exact);
        assert!(!result.hash_matches);
    }

    #[tokio::test]
    async fn resolve_with_wrong_name_falls_through_tiers() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        // Break the exact query name so Tier 1 fails, but relaxed (no name predicate)
        // can still match via hash disambiguation
        bm.query = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "nonexistentFunction")) @target"#
            .to_string();
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        // Relaxed strips the predicate, finds multiple functions, disambiguates by hash
        assert!(
            result.method == ResolutionMethod::Relaxed
                || result.method == ResolutionMethod::HashFallback,
            "expected relaxed or hash_fallback, got {:?}",
            result.method
        );
        // Should still find the right content
        assert!(result.hash_matches);
    }

    #[tokio::test]
    async fn resolve_completely_missing_fails() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        bm.query = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "totallyGone")) @target"#
            .to_string();
        bm.content_hash = Some("sha256:0000000000000000".to_string());
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        assert_eq!(result.method, ResolutionMethod::Failed);
    }

    #[tokio::test]
    async fn breadcrumbs_extracted_from_sticky_captures() {
        // Test that breadcrumbs come from captures when present
        let (bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        // The generated query should have sticky captures
        assert!(bm.query.contains("@sticky.class"));

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();

        // Should have breadcrumbs from sticky captures
        assert!(!result.breadcrumbs.is_empty(), "expected breadcrumbs from sticky captures");
        // First breadcrumb should be the class declaration line
        assert!(result.breadcrumbs[0].text.contains("class"));
    }

    #[tokio::test]
    async fn fallback_to_ast_walking_without_sticky_captures() {
        // Test that legacy queries without @sticky captures still work
        let source = r#"
class LegacyService {
    func legacyFunc() {}
}
"#;

        let path = fixture_path("legacy_test.swift");
        // Create a temporary fixture file
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, source).ok();

        let _cache = ParseCache::new(CodemarkLang::Swift).unwrap();
        let lang = CodemarkLang::Swift.tree_sitter_language();

        // Create a legacy query without sticky captures
        let query = r#"
(class_declaration
  name: (type_identifier) @name0
  (class_body
    (function_declaration
      name: (simple_identifier) @fn_name) @target))
"#;

        let mut parser = crate::parser::languages::Parser::new(CodemarkLang::Swift).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();

        let matches =
            crate::query::matcher::run_query(query, &tree, source.as_bytes(), &lang).unwrap();
        assert_eq!(matches.len(), 1);

        // Breadcrumb extraction should fall back to AST walking
        let target_node = tree
            .root_node()
            .descendant_for_byte_range(matches[0].byte_range.0, matches[0].byte_range.1)
            .unwrap();
        let breadcrumbs = crate::engine::breadcrumbs::extract_breadcrumbs(
            target_node,
            source,
            crate::parser::languages::Language::Swift,
            3,
        );

        // Should have breadcrumbs from AST walking (the class)
        assert!(!breadcrumbs.is_empty());

        // Clean up
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn extract_breadcrumbs_from_captures_direct() {
        // Direct unit test for extract_breadcrumbs_from_captures
        let source = r#"
class TestClass {
    func testMethod() {}
}
"#;

        let mut parser = crate::parser::languages::Parser::new(CodemarkLang::Swift).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();

        // Manually create captures to test the extraction function
        let root = tree.root_node();
        let mut class_line = None;
        let mut method_line = None;

        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "class_declaration" {
                class_line = Some(child.start_position().row);
                // Class body contains the function
                let mut cursor2 = child.walk();
                if cursor2.goto_first_child() {
                    loop {
                        let child2 = cursor2.node();
                        // Look through the class body to find the function
                        if child2.is_named() {
                            let mut cursor3 = child2.walk();
                            for child3 in child2.named_children(&mut cursor3) {
                                if child3.kind() == "function_declaration" {
                                    method_line = Some(child3.start_position().row);
                                    break;
                                }
                            }
                        }
                        if !cursor2.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
        }

        let class_l = class_line.expect("class node not found");
        let method_l = method_line.expect("method node not found");

        let captures = vec![
            ("sticky.class".to_string(), (0, 0), class_l),
            ("sticky.function".to_string(), (0, 0), method_l),
        ];

        let breadcrumbs = extract_breadcrumbs_from_captures(&captures, source);

        assert_eq!(breadcrumbs.len(), 2);
        assert!(breadcrumbs[0].text.contains("class TestClass"));
        assert!(breadcrumbs[1].text.contains("func testMethod"));
    }

    #[tokio::test]
    async fn test_exact_over_relaxed() {
        // @lat: [[tests#System Invariants Tests#Health & Resolution Rules#Hierarchical Preference]]
        // Verifies that the resolution engine always prefers higher-tier matches.
        // It must never return a `relaxed` match if an `exact` match is possible
        // for the same query.
        let (bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken").await;
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let provider = crate::vfs::LocalFileProvider;
        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();

        // The original query should match exactly
        assert_eq!(
            result.method,
            ResolutionMethod::Exact,
            "Expected Exact match when the query uniquely matches the target"
        );

        // Test the hierarchy: when exact fails but relaxed would succeed,
        // verify we fall through to a lower tier (not Exact)
        let mut bm_relaxed = bm.clone();
        // Break the exact query with a wrong name, but relaxed will strip the predicate
        bm_relaxed.query = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "thisDoesNotExist")) @target"#
            .to_string();

        let result3 =
            resolve(&bm_relaxed, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();

        // Should fall through to Relaxed tier (or HashFallback depending on implementation)
        // The key is that it should NOT be Exact
        assert_ne!(
            result3.method,
            ResolutionMethod::Exact,
            "Should not return Exact when the exact query predicate fails"
        );

        // And it should be Relaxed or HashFallback (both are valid fall-throughs)
        assert!(
            result3.method == ResolutionMethod::Relaxed
                || result3.method == ResolutionMethod::HashFallback,
            "Expected Relaxed or HashFallback when exact predicate fails, got {:?}",
            result3.method
        );
    }

    #[tokio::test]
    async fn test_resolution_tier_hierarchy() {
        // @lat: [[tests#System Invariants Tests#Health & Resolution Rules#Hierarchical Preference]]
        // Explicitly test that tiers are tried in order: Exact → Relaxed → Minimal → HashFallback
        let source = r#"
class TierTest {
    func targetFunction() -> String {
        return "test"
    }

    func otherFunction() -> String {
        return "other"
    }
}
"#;

        let path = fixture_path("tier_test.swift");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, source).ok();

        let mut cache = ParseCache::new(CodemarkLang::Swift).unwrap();
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let profile = CodemarkLang::Swift.profile();

        // Generate a query for targetFunction with its hash
        let mut parser = crate::parser::languages::Parser::new(CodemarkLang::Swift).unwrap();
        let tree = parser.parse(source.as_bytes()).unwrap();

        // Find targetFunction node
        let root = tree.root_node();
        let mut target_byte_range = None;
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "class_declaration" {
                let mut cursor2 = child.walk();
                for child2 in child.named_children(&mut cursor2) {
                    if child2.is_named() && source[child2.byte_range()].contains("targetFunction") {
                        target_byte_range = Some(child2.byte_range());
                        break;
                    }
                }
            }
        }
        let byte_range = target_byte_range.unwrap();
        let range = (byte_range.start, byte_range.end);

        let generated =
            qgen::generate_query(&tree, source.as_bytes(), range, &lang, profile).unwrap();
        let ch = hash::content_hash(&source[byte_range]);

        // Test 1: Exact match - query as generated
        let bm_exact = Bookmark {
            id: "tier-test-exact".to_string(),
            query: generated.query.clone(),
            language: "swift".to_string(),
            file_path: path.to_string_lossy().to_string(),
            content_hash: Some(ch.clone()),
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            current_resolution_id: None,
            repo_id: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");
        let provider = crate::vfs::LocalFileProvider;
        let result =
            resolve(&bm_exact, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        assert_eq!(result.method, ResolutionMethod::Exact, "Tier 1: Exact should match");

        // Test 2: Relaxed - wrong name predicate but right hash
        let bm_relaxed = Bookmark {
            id: "tier-test-relaxed".to_string(),
            query: r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "wrongName")) @target"#
                .to_string(),
            language: "swift".to_string(),
            file_path: path.to_string_lossy().to_string(),
            content_hash: Some(ch.clone()),
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            current_resolution_id: None,
            repo_id: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let result =
            resolve(&bm_relaxed, &mut cache, &lang, dummy_db.as_path(), &provider).await.unwrap();
        assert_ne!(
            result.method,
            ResolutionMethod::Exact,
            "Exact should not match with wrong name"
        );
        // The relaxed tier may produce multiple matches, causing fall-through to hash_fallback
        // Both Relaxed and HashFallback are valid outcomes when exact fails
        assert!(
            result.method == ResolutionMethod::Relaxed
                || result.method == ResolutionMethod::HashFallback,
            "Expected Relaxed or HashFallback when exact predicate fails, got {:?}",
            result.method
        );

        // Clean up
        std::fs::remove_file(&path).ok();
    }
}
