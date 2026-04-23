use codemark::parser::languages::{Language as CodemarkLang, Parser};
use codemark::query::generator::{self, StrategyType};
use std::fs;
use std::path::Path;

fn get_fixture_content(name: &str) -> (String, tree_sitter::Tree) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/swift")
        .join(name);
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

#[test]
fn test_swift_complex_query_snapshots() {
    let (source, tree) = get_fixture_content("complex_scenarios.swift");
    let language = CodemarkLang::Swift.tree_sitter_language();

    let scenarios = [
        ("declaration_method", "27", StrategyType::Declaration),
        ("fine_grained_point_call", "28:10", StrategyType::FineGrained),
        ("fine_grained_point_self", "29:23", StrategyType::FineGrained),
        ("fine_grained_range_closure", "28-37", StrategyType::FineGrained),
        ("fine_grained_point_identifier", "33:14", StrategyType::FineGrained),
        ("declaration_from_inner_point", "33:14", StrategyType::Declaration),
        ("declaration_extension_method", "46", StrategyType::Declaration),
        ("declaration_enum_property", "56", StrategyType::Declaration),
    ];

    for (name, range_str, strategy) in scenarios {
        let range = codemark::cli::handlers::parse_range(&range_str, &source).unwrap();
        let result = generator::generate_query(
            &tree,
            source.as_bytes(),
            range,
            &language,
            strategy,
        ).unwrap_or_else(|e| panic!("Failed to generate query for {} ({}): {}", name, range_str, e));

        let snapshot = QuerySnapshot {
            name: name.to_string(),
            strategy: format!("{:?}", strategy),
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
