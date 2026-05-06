use codemark_core::engine::breadcrumbs::extract_breadcrumbs;
use codemark_core::parser::languages::{Language, ParseCache};
use std::path::PathBuf;

fn get_fixture_path(lang: &str, file: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // root/
    path.push("tests");
    path.push("fixtures");
    path.push(lang);
    path.push(file);
    path
}

#[tokio::test]
async fn test_breadcrumb_extraction_rust() {
    let path = get_fixture_path("rust", "auth_service.rs");
    let source = std::fs::read_to_string(&path).expect("failed to read rust fixture");
    let lang = Language::Rust;
    let mut cache = ParseCache::new(lang).expect("failed to create cache");
    let ts_lang = lang.tree_sitter_language();

    let tree = cache
        .parser_mut()
        .parse(source.as_bytes())
        .expect("failed to parse");
    let root = tree.root_node();

    // Target: decode method on line 49 (0-indexed line 48)
    // Looking for: fn decode(&self, token: &str)
    let target_node = root
        .descendant_for_point_range(
            tree_sitter::Point::new(48, 4),
            tree_sitter::Point::new(48, 10),
        )
        .expect("node not found");

    let breadcrumbs = extract_breadcrumbs(target_node, &source, lang, 3);
    
    // Also capture a preview around the target (padding 2)
    let range = (target_node.start_byte(), target_node.end_byte());
    let start_line = target_node.start_position().row;
    let end_line = target_node.end_position().row;
    
    let res = codemark_core::engine::resolution::ResolutionResult {
        method: codemark_core::engine::bookmark::ResolutionMethod::Exact,
        file_path: "test.file".into(),
        byte_range: range,
        start_line,
        start_col: 0,
        end_line,
        matched_text: source[range.0..range.1].to_string(),
        content_hash: "".into(),
        hash_matches: true,
        breadcrumbs: breadcrumbs.clone(),
        new_query: None,
    };
    
    let preview = res.capture_preview(&source, 2);

    insta::assert_yaml_snapshot!(serde_json::json!({
        "breadcrumbs": breadcrumbs,
        "preview_lines": preview
    }));
}

#[tokio::test]
async fn test_breadcrumb_extraction_swift() {
    let path = get_fixture_path("swift", "auth_service.swift");
    let source = std::fs::read_to_string(&path).expect("failed to read swift fixture");
    let lang = Language::Swift;
    let mut cache = ParseCache::new(lang).expect("failed to create cache");
    let ts_lang = lang.tree_sitter_language();

    let tree = cache
        .parser_mut()
        .parse(source.as_bytes())
        .expect("failed to parse");
    let root = tree.root_node();

    // Target: decode method on line 73 (0-indexed line 72)
    let target_node = root
        .descendant_for_point_range(
            tree_sitter::Point::new(72, 10),
            tree_sitter::Point::new(72, 20),
        )
        .expect("node not found");

    let breadcrumbs = extract_breadcrumbs(target_node, &source, lang, 3);
    
    // Also capture a preview around the target (padding 2)
    let range = (target_node.start_byte(), target_node.end_byte());
    let start_line = target_node.start_position().row;
    let end_line = target_node.end_position().row;
    
    let res = codemark_core::engine::resolution::ResolutionResult {
        method: codemark_core::engine::bookmark::ResolutionMethod::Exact,
        file_path: "test.file".into(),
        byte_range: range,
        start_line,
        start_col: 0,
        end_line,
        matched_text: source[range.0..range.1].to_string(),
        content_hash: "".into(),
        hash_matches: true,
        breadcrumbs: breadcrumbs.clone(),
        new_query: None,
    };
    
    let preview = res.capture_preview(&source, 2);

    insta::assert_yaml_snapshot!(serde_json::json!({
        "breadcrumbs": breadcrumbs,
        "preview_lines": preview
    }));
}

#[tokio::test]
async fn test_breadcrumb_extraction_typescript() {
    let path = get_fixture_path("typescript", "auth_service.ts");
    let source = std::fs::read_to_string(&path).expect("failed to read ts fixture");
    let lang = Language::TypeScript;
    let mut cache = ParseCache::new(lang).expect("failed to create cache");
    let ts_lang = lang.tree_sitter_language();

    let tree = cache
        .parser_mut()
        .parse(source.as_bytes())
        .expect("failed to parse");
    let root = tree.root_node();

    // Find a class method
    let target_node = root
        .descendant_for_point_range(
            tree_sitter::Point::new(30, 10),
            tree_sitter::Point::new(30, 20),
        )
        .expect("node not found");

    let breadcrumbs = extract_breadcrumbs(target_node, &source, lang, 3);
    
    // Also capture a preview around the target (padding 2)
    let range = (target_node.start_byte(), target_node.end_byte());
    let start_line = target_node.start_position().row;
    let end_line = target_node.end_position().row;
    
    let res = codemark_core::engine::resolution::ResolutionResult {
        method: codemark_core::engine::bookmark::ResolutionMethod::Exact,
        file_path: "test.file".into(),
        byte_range: range,
        start_line,
        start_col: 0,
        end_line,
        matched_text: source[range.0..range.1].to_string(),
        content_hash: "".into(),
        hash_matches: true,
        breadcrumbs: breadcrumbs.clone(),
        new_query: None,
    };
    
    let preview = res.capture_preview(&source, 2);

    insta::assert_yaml_snapshot!(serde_json::json!({
        "breadcrumbs": breadcrumbs,
        "preview_lines": preview
    }));
}
