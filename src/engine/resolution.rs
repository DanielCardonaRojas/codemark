use std::path::Path;

use tree_sitter::Language;

use crate::engine::bookmark::{Bookmark, ResolutionMethod};
use crate::engine::hash;
use crate::error::Result;
use crate::git::context as git_context;
use crate::parser::languages::ParseCache;
use crate::query::{generator, matcher, relaxer};

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
    /// If hash fallback was used, the regenerated query for the new location.
    pub new_query: Option<String>,
}

/// Resolve a single bookmark against the current state of its file.
#[allow(clippy::collapsible_if)]
pub fn resolve(
    bookmark: &Bookmark,
    cache: &mut ParseCache,
    language: &Language,
    db_path: &Path,
) -> Result<ResolutionResult> {
    // Resolve relative path to absolute for file reading
    let path = git_context::resolve_bookmark_file_path(&bookmark.file_path, db_path)?;
    let (tree, source) = cache.get_or_parse(&path)?;
    let source_bytes = source.as_bytes();

    // Tier 1: Exact query
    if let Ok(matches) = matcher::run_query(&bookmark.query, tree, source_bytes, language) {
        if matches.len() == 1 {
            let m = &matches[0];
            let ch = hash::content_hash(&m.node_text);
            let hash_matches = bookmark.content_hash.as_deref() == Some(ch.as_str());
            return Ok(ResolutionResult {
                method: ResolutionMethod::Exact,
                file_path: bookmark.file_path.clone(),
                byte_range: m.byte_range,
                start_line: m.start_point.0,
                start_col: m.start_point.1,
                end_line: m.end_point.0,
                matched_text: m.node_text.clone(),
                content_hash: ch,
                hash_matches,
                new_query: None,
            });
        }
    }

    // Tier 2: Relaxed query
    if let Ok(relaxed) = relaxer::relax_query(&bookmark.query) {
        if let Ok(matches) = matcher::run_query(&relaxed, tree, source_bytes, language) {
            if let Some(result) = pick_match(&matches, bookmark, ResolutionMethod::Relaxed) {
                return Ok(result);
            }
        }
    }

    // Tier 3: Minimal query
    if let Ok(minimal) = relaxer::minimize_query(&bookmark.query) {
        if let Ok(matches) = matcher::run_query(&minimal, tree, source_bytes, language) {
            if let Some(result) = pick_match(&matches, bookmark, ResolutionMethod::Relaxed) {
                return Ok(result);
            }
        }
    }

    // Tier 4: Hash fallback — walk all named nodes
    if let Some(ref stored_hash) = bookmark.content_hash {
        let root = tree.root_node();
        if let Some(result) =
            hash_fallback_walk(root, source_bytes, stored_hash, bookmark, language)
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
        new_query: None,
    })
}

/// Pick the best match from a list — single match or disambiguate by hash.
fn pick_match(
    matches: &[matcher::MatchResult],
    bookmark: &Bookmark,
    method: ResolutionMethod,
) -> Option<ResolutionResult> {
    if matches.len() == 1 {
        let m = &matches[0];
        let ch = hash::content_hash(&m.node_text);
        let hash_matches = bookmark.content_hash.as_deref() == Some(ch.as_str());
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
            new_query: None,
        });
    }

    // Multiple matches — disambiguate by content hash
    if let Some(ref stored_hash) = bookmark.content_hash {
        for m in matches {
            let ch = hash::content_hash(&m.node_text);
            if ch == *stored_hash {
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
                    new_query: None,
                });
            }
        }
    }

    None
}

/// Walk all named nodes looking for a hash match.
fn hash_fallback_walk(
    node: tree_sitter::Node,
    source: &[u8],
    stored_hash: &str,
    bookmark: &Bookmark,
    language: &Language,
) -> Option<ResolutionResult> {
    if node.is_named() {
        let text = std::str::from_utf8(&source[node.byte_range()]).unwrap_or("");
        let ch = hash::content_hash(text);
        if ch == stored_hash {
            // Regenerate query for this new location
            let new_query =
                generator::generate_query_for_node(node, source, language).ok().map(|gq| gq.query);

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
                new_query,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(result) = hash_fallback_walk(child, source, stored_hash, bookmark, language) {
            return Some(result);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, BookmarkStatus};
    use crate::parser::languages::Language as CodemarkLang;
    use crate::query::generator as qgen;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/swift/{name}"))
    }

    fn create_bookmark_for_function(file: &str, func_name: &str) -> (Bookmark, ParseCache) {
        let path = fixture_path(file);
        let mut cache = ParseCache::new(CodemarkLang::Swift).unwrap();
        let lang = CodemarkLang::Swift.tree_sitter_language();

        let (tree, source) = cache.get_or_parse(&path).unwrap();
        let range = find_function_range(tree, source, func_name);
        let generated = qgen::generate_query(tree, source.as_bytes(), range, &lang).unwrap();
        let ch = hash::content_hash(&source[range.0..range.1]);

        let bm = Bookmark {
            id: "test-bm".to_string(),
            query: generated.query,
            language: "swift".to_string(),
            file_path: path.to_string_lossy().to_string(),
            content_hash: Some(ch),
            commit_hash: None,
            status: BookmarkStatus::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            annotations: vec![],
        };

        (bm, cache)
    }

    fn find_function_range(tree: &tree_sitter::Tree, source: &str, name: &str) -> (usize, usize) {
        fn search(node: tree_sitter::Node, source: &str, name: &str) -> Option<(usize, usize)> {
            if node.kind() == "function_declaration" {
                if let Some(name_node) = node.child_by_field_name("name") {
                    if &source[name_node.byte_range()] == name {
                        return Some((node.start_byte(), node.end_byte()));
                    }
                }
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

    #[test]
    fn resolve_exact_match() {
        let (bm, mut cache) = create_bookmark_for_function("auth_service.swift", "validateToken");
        let lang = CodemarkLang::Swift.tree_sitter_language();
        // For tests, use a dummy db path - the bookmark stores absolute paths from fixture_path
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path()).unwrap();
        assert_eq!(result.method, ResolutionMethod::Exact);
        assert!(result.hash_matches);
        assert!(result.matched_text.contains("validateToken"));
    }

    #[test]
    fn resolve_exact_with_hash_mismatch() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken");
        // Corrupt the stored hash
        bm.content_hash = Some("sha256:0000000000000000".to_string());
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path()).unwrap();
        assert_eq!(result.method, ResolutionMethod::Exact);
        assert!(!result.hash_matches);
    }

    #[test]
    fn resolve_with_wrong_name_falls_through_tiers() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken");
        // Break the exact query name so Tier 1 fails, but relaxed (no name predicate)
        // can still match via hash disambiguation
        bm.query = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "nonexistentFunction")) @target"#
            .to_string();
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path()).unwrap();
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

    #[test]
    fn resolve_completely_missing_fails() {
        let (mut bm, mut cache) =
            create_bookmark_for_function("auth_service.swift", "validateToken");
        bm.query = r#"(function_declaration
  name: (simple_identifier) @fn_name
  (#eq? @fn_name "totallyGone")) @target"#
            .to_string();
        bm.content_hash = Some("sha256:0000000000000000".to_string());
        let lang = CodemarkLang::Swift.tree_sitter_language();
        let dummy_db =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".codemark/codemark.db");

        let result = resolve(&bm, &mut cache, &lang, dummy_db.as_path()).unwrap();
        assert_eq!(result.method, ResolutionMethod::Failed);
    }
}
