use codemark::parser::languages::{Language as CodemarkLang, Parser};
use codemark::query::generator;
use std::fs;
use std::path::Path;

fn get_fixture_content(name: &str) -> (String, tree_sitter::Tree) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/swift").join(name);
    let source = fs::read_to_string(&path).unwrap();
    let mut parser = Parser::new(CodemarkLang::Swift).unwrap();
    let (tree, _) = parser.parse_file(&path).unwrap();
    (source, tree)
}

use serde::Serialize;

#[derive(Serialize)]
struct QuerySnapshot {
    name: String,
    strategy: String,
    range: String,
    node_type: String,
    query: String,
}

fn parse_test_range(source: &str, range_str: &str) -> (usize, usize) {
    if let Some((start_str, end_str)) = range_str.split_once('-') {
        let start = parse_test_point(source, start_str);
        let end = parse_test_point(source, end_str);
        (start, end)
    } else if let Some((line_str, col_str)) = range_str.split_once(':') {
        let line: usize = line_str.parse().unwrap();
        let col: usize = col_str.parse().unwrap();
        let offset = line_col_to_byte(source, line, col);
        (offset, offset)
    } else {
        let line: usize = range_str.parse().unwrap();
        let offset = line_col_to_byte(source, line, 1);
        // Find end of line
        let mut end = offset;
        let bytes = source.as_bytes();
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        (offset, end)
    }
}

fn parse_test_point(source: &str, s: &str) -> usize {
    if let Some((line_str, col_str)) = s.split_once(':') {
        let line: usize = line_str.parse().unwrap();
        let col: usize = col_str.parse().unwrap();
        line_col_to_byte(source, line, col)
    } else {
        let line: usize = s.parse().unwrap();
        line_col_to_byte(source, line, 1)
    }
}

fn line_col_to_byte(source: &str, line: usize, col: usize) -> usize {
    let mut byte_offset = 0;
    for (i, line_text) in source.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            return byte_offset + col - 1;
        }
        byte_offset += line_text.len();
    }
    panic!("Line {} out of bounds", line);
}

#[test]
fn test_swift_complex_query_snapshots() {
    let (source, tree) = get_fixture_content("complex_scenarios.swift");
    let language = CodemarkLang::Swift.tree_sitter_language();

    let scenarios = [
        ("declaration_method", "27"),
        ("fine_grained_point_call", "28:10"),
        ("fine_grained_point_self", "29:23"),
        ("fine_grained_range_closure", "28-37"),
        ("fine_grained_point_identifier", "33:14"),
        ("declaration_from_inner_point", "33:14"),
        ("declaration_extension_method", "46"),
        ("declaration_enum_property", "56"),
    ];

    for (name, range_str) in scenarios {
        let range = parse_test_range(&source, range_str);
        let result = generator::generate_query(&tree, source.as_bytes(), range, &language)
            .unwrap_or_else(|e| {
                panic!("Failed to generate query for {} ({}): {}", name, range_str, e)
            });

        let snapshot = QuerySnapshot {
            name: name.to_string(),
            strategy: "Unified".to_string(),
            range: range_str.to_string(),
            node_type: result.target_node_type,
            query: "[query stored in .scm file]".to_string(),
        };

        // Snapshot metadata as YAML
        insta::assert_yaml_snapshot!(format!("{}_meta", name), snapshot);
        // Snapshot query as plain text (SCM-like)
        insta::assert_snapshot!(format!("{}_query", name), result.query);
    }
}
