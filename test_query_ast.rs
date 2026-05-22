use tree_sitter::Parser;

fn main() {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_query::LANGUAGE.into()).unwrap();
    
    let queries = [
        "(function_item) @target",
        "(function_item name: (identifier) @name (#eq? @name \"foo\")) @target",
        "(class_declaration (function_item) @target)"
    ];
    
    for query in queries {
        println!("Query: {}", query);
        let tree = parser.parse(query, None).unwrap();
        print_node(tree.root_node(), query, 0);
        println!("---");
    }
}

fn print_node(node: tree_sitter::Node, source: &str, depth: usize) {
    let text = node.utf8_text(source.as_bytes()).unwrap();
    println!("{}{:?} - \"{}\"", "  ".repeat(depth), node.kind(), text);
    for i in 0..node.child_count() {
        print_node(node.child(i).unwrap(), source, depth + 1);
    }
}
