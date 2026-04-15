//! Programming language support for tree-sitter parsing.

#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Swift,
    Rust,
    TypeScript,
    Python,
    Go,
    Java,
    CSharp,
    Dart,
}

impl Language {
    pub fn tree_sitter_language(&self) -> tree_sitter::Language {
        match self {
            Language::Swift => tree_sitter_swift::LANGUAGE.into(),
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::Java => tree_sitter_java::LANGUAGE.into(),
            Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Language::Dart => tree_sitter_dart::LANGUAGE.into(),
        }
    }

    pub fn file_extensions(&self) -> &[&str] {
        match self {
            Language::Swift => &["swift"],
            Language::Rust => &["rs"],
            Language::TypeScript => &["ts", "tsx"],
            Language::Python => &["py"],
            Language::Go => &["go"],
            Language::Java => &["java"],
            Language::CSharp => &["cs"],
            Language::Dart => &["dart"],
        }
    }

    /// Infer language from a file extension. Returns None if unrecognized.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "swift" => Some(Language::Swift),
            "rs" => Some(Language::Rust),
            "ts" | "tsx" => Some(Language::TypeScript),
            "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "java" => Some(Language::Java),
            "cs" => Some(Language::CSharp),
            "dart" => Some(Language::Dart),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Language::Swift => write!(f, "swift"),
            Language::Rust => write!(f, "rust"),
            Language::TypeScript => write!(f, "typescript"),
            Language::Python => write!(f, "python"),
            Language::Go => write!(f, "go"),
            Language::Java => write!(f, "java"),
            Language::CSharp => write!(f, "csharp"),
            Language::Dart => write!(f, "dart"),
        }
    }
}

impl FromStr for Language {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "swift" => Ok(Language::Swift),
            "rust" | "rs" => Ok(Language::Rust),
            "typescript" | "ts" | "tsx" => Ok(Language::TypeScript),
            "python" | "py" => Ok(Language::Python),
            "go" => Ok(Language::Go),
            "java" => Ok(Language::Java),
            "csharp" | "c#" | "cs" => Ok(Language::CSharp),
            "dart" => Ok(Language::Dart),
            _ => Err(Error::Input(format!("unsupported language: {s}"))),
        }
    }
}

/// Tree-sitter parser wrapper for a specific language.
pub struct Parser {
    inner: tree_sitter::Parser,
    language: Language,
}

impl Parser {
    pub fn new(language: Language) -> Result<Self> {
        let mut inner = tree_sitter::Parser::new();
        inner
            .set_language(&language.tree_sitter_language())
            .map_err(|e| Error::TreeSitter(format!("failed to set language: {e}")))?;
        Ok(Parser { inner, language })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn parse(&mut self, source: &[u8]) -> Result<tree_sitter::Tree> {
        self.inner
            .parse(source, None)
            .ok_or_else(|| Error::TreeSitter("failed to parse source".into()))
    }

    pub fn parse_file(&mut self, path: &Path) -> Result<(tree_sitter::Tree, String)> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Input(format!("cannot read {}: {e}", path.display())))?;
        let tree = self.parse(source.as_bytes())?;
        Ok((tree, source))
    }
}

/// Cache of parsed trees within a single CLI invocation.
pub struct ParseCache {
    trees: HashMap<PathBuf, (tree_sitter::Tree, String)>,
    parser: Parser,
}

impl ParseCache {
    pub fn new(language: Language) -> Result<Self> {
        Ok(ParseCache { trees: HashMap::new(), parser: Parser::new(language)? })
    }

    /// Get the parsed tree and source for a file, parsing it if not already cached.
    pub fn get_or_parse(&mut self, path: &Path) -> Result<&(tree_sitter::Tree, String)> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        if !self.trees.contains_key(&canonical) {
            let (tree, source) = self.parser.parse_file(path)?;
            self.trees.insert(canonical.clone(), (tree, source));
        }

        Ok(self.trees.get(&canonical).unwrap())
    }

    pub fn parser_mut(&mut self) -> &mut Parser {
        &mut self.parser
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_round_trip() {
        for (input, expected, display) in [
            ("swift", Language::Swift, "swift"),
            ("rust", Language::Rust, "rust"),
            ("rs", Language::Rust, "rust"),
            ("typescript", Language::TypeScript, "typescript"),
            ("ts", Language::TypeScript, "typescript"),
            ("tsx", Language::TypeScript, "typescript"),
            ("python", Language::Python, "python"),
            ("py", Language::Python, "python"),
        ] {
            let lang: Language = input.parse().unwrap();
            assert_eq!(lang, expected, "parsing '{input}'");
            assert_eq!(lang.to_string(), display);
        }
    }

    #[test]
    fn unsupported_language_errors() {
        assert!("fortran".parse::<Language>().is_err());
    }

    #[test]
    fn parse_swift_source() {
        let mut parser = Parser::new(Language::Swift).unwrap();
        let source = b"func hello() { print(\"hello\") }";
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[test]
    fn parse_rust_source() {
        let mut parser = Parser::new(Language::Rust).unwrap();
        let source = b"fn hello() { println!(\"hello\"); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parse_typescript_source() {
        let mut parser = Parser::new(Language::TypeScript).unwrap();
        let source = b"function hello(): void { console.log('hello'); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parse_python_source() {
        let mut parser = Parser::new(Language::Python).unwrap();
        let source = b"def hello():\n    print('hello')\n";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "module");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parse_swift_fixture() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(Language::Swift).unwrap();
        let (tree, _source) = parser.parse_file(&fixture).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[test]
    fn parse_cache_reuses_tree() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut cache = ParseCache::new(Language::Swift).unwrap();
        let (tree1, _) = cache.get_or_parse(&fixture).unwrap();
        let root1_kind = tree1.root_node().kind().to_string();
        let (tree2, _) = cache.get_or_parse(&fixture).unwrap();
        let root2_kind = tree2.root_node().kind().to_string();
        assert_eq!(root1_kind, root2_kind);
    }
}
