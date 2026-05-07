use syntect_assets::assets::HighlightingAssets;

fn main() {
    let assets = HighlightingAssets::from_binary();
    let ss = assets.get_syntax_set().unwrap();
    println!("Total syntaxes: {}", ss.syntaxes().len());

    // Check for Swift
    let swift_syntax = ss.find_syntax_by_name("Swift");
    match swift_syntax {
        Some(s) => println!("Found Swift: {} - extensions: {:?}", s.name, s.file_extensions),
        None => println!("Swift NOT found in syntax set"),
    }

    // Check if we can find it by extension
    let by_ext = ss.find_syntax_by_extension("swift");
    match by_ext {
        Some(s) => println!("Found by extension 'swift': {}", s.name),
        None => println!("NOT found by extension 'swift'"),
    }
}
