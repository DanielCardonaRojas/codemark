//! Programming language support for tree-sitter parsing.

#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::parser::profile::Profile;

/// A programming language codemark can parse.
///
/// Built-in variants are statically compiled. The [`Dynamic`] variant holds a
/// WASM grammar loaded at runtime from the cache directory. Two `Language`
/// values are equal if they have the same identity (same static variant, or
/// same name for dynamic). `Hash`/`Eq` are based on identity, not full struct
/// equality — this lets `Language` serve as a `HashMap` key.
#[derive(Debug, Clone)]
pub enum Language {
    Swift,
    Rust,
    TypeScript,
    Python,
    Go,
    Java,
    CSharp,
    Dart,
    /// A dynamically loaded WASM grammar.
    Dynamic(Arc<DynamicLanguage>),
}

/// Metadata for a WASM-loaded language, parsed from `manifest.json`.
#[derive(Debug, Clone)]
pub struct DynamicLanguage {
    /// Registry name, e.g. "lua", "ruby", "bash". Used in stored bookmarks.
    pub name: String,
    /// File extensions this grammar handles (without the dot).
    pub extensions: Vec<String>,
    /// Path to the cached `.wasm` file.
    pub wasm_path: PathBuf,
    /// Language-specific structural metadata.
    pub profile: Profile,
}
// Identity-based equality and hashing — two Languages are equal if they have
// the same name. This lets Language serve as a HashMap key even though
// DynamicLanguage carries heavy data (wasm_path, profile, etc.).
impl PartialEq for Language {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}
impl Eq for Language {}
impl Hash for Language {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name().hash(state);
    }
}

impl Language {
    /// The canonical name string for this language (e.g. "rust", "lua").
    pub fn name(&self) -> &str {
        match self {
            Language::Swift => "swift",
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
            Language::Java => "java",
            Language::CSharp => "csharp",
            Language::Dart => "dart",
            Language::Dynamic(dl) => &dl.name,
        }
    }

    /// Returns the static tree-sitter language for built-in variants.
    ///
    /// **Panics** for `Dynamic` — the WASM-loaded language is only available
    /// inside a `Parser` that holds the `WasmStore`. Callers that need a
    /// `tree_sitter::Language` for dynamic grammars must go through `Parser`.
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
            Language::Dynamic(_) => panic!(
                "tree_sitter_language() is not available for Dynamic languages; use Parser::new()"
            ),
        }
    }

    pub fn file_extensions(&self) -> Vec<String> {
        match self {
            Language::Swift => vec!["swift".into()],
            Language::Rust => vec!["rs".into()],
            Language::TypeScript => vec!["ts".into(), "tsx".into()],
            Language::Python => vec!["py".into()],
            Language::Go => vec!["go".into()],
            Language::Java => vec!["java".into()],
            Language::CSharp => vec!["cs".into()],
            Language::Dart => vec!["dart".into()],
            Language::Dynamic(dl) => dl.extensions.clone(),
        }
    }

    /// Infer language from a file extension. Checks static languages first,
    /// then the dynamic grammar registry.
    pub fn from_extension(ext: &str) -> Option<Self> {
        Self::static_from_extension(ext)
            .or_else(|| crate::parser::registry::LanguageRegistry::from_extension(ext))
    }

    /// Resolve a file extension (without the dot) to a *built-in* language,
    /// without consulting the dynamic registry. The registry uses this to reject
    /// dynamic grammars claiming an extension a built-in already owns (which
    /// would otherwise be shadowed and never resolve).
    pub fn static_from_extension(ext: &str) -> Option<Language> {
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

    /// List all built-in static languages.
    pub fn builtins() -> Vec<Self> {
        vec![
            Language::Rust,
            Language::Swift,
            Language::TypeScript,
            Language::Python,
            Language::Go,
            Language::Java,
            Language::CSharp,
            Language::Dart,
        ]
    }

    /// List all supported languages (static + any dynamic ones discovered).
    pub fn all_supported() -> Vec<Self> {
        let mut all = Self::builtins();
        all.extend(crate::parser::registry::LanguageRegistry::dynamic_languages());
        all
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for Language {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        if let Some(lang) = Language::static_from_name(s) {
            return Ok(lang);
        }
        crate::parser::registry::LanguageRegistry::from_name(s)
            .ok_or_else(|| Error::Input(format!("unsupported language: {s}")))
    }
}

impl Language {
    /// Resolve a name (including alias like `rs`/`ts`) to a *built-in* language,
    /// without consulting the dynamic registry.
    ///
    /// This is the authoritative list of names the static languages claim. The
    /// registry uses it to reject dynamic grammars whose name would be shadowed
    /// by a built-in, so the two can never drift (see [`from_str`](Self::from_str)).
    pub fn static_from_name(s: &str) -> Option<Language> {
        match s.to_lowercase().as_str() {
            "swift" => Some(Language::Swift),
            "rust" | "rs" => Some(Language::Rust),
            "typescript" | "ts" | "tsx" => Some(Language::TypeScript),
            "python" | "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "java" => Some(Language::Java),
            "csharp" | "c#" | "cs" => Some(Language::CSharp),
            "dart" => Some(Language::Dart),
            _ => None,
        }
    }
}

/// Tree-sitter parser wrapper for a specific language.
///
/// Owns the `tree_sitter::Language` handle so callers never need
/// [`Language::tree_sitter_language`] — which has no answer for a WASM-loaded
/// grammar, since that handle lives inside the parser's WASM store. Use
/// [`Parser::language`] instead.
pub struct Parser {
    inner: tree_sitter::Parser,
    language: Language,
    ts_language: tree_sitter::Language,
}

impl Parser {
    pub fn new(language: Language) -> Result<Self> {
        let mut inner = tree_sitter::Parser::new();
        let ts_language = match &language {
            Language::Swift
            | Language::Rust
            | Language::TypeScript
            | Language::Python
            | Language::Go
            | Language::Java
            | Language::CSharp
            | Language::Dart => {
                let ts = language.tree_sitter_language();
                inner
                    .set_language(&ts)
                    .map_err(|e| Error::TreeSitter(format!("failed to set language: {e}")))?;
                ts
            }
            Language::Dynamic(dl) => {
                #[cfg(feature = "wasm")]
                {
                    let engine = wasm_engine();
                    let mut store = tree_sitter::WasmStore::new(engine)
                        .map_err(|e| Error::TreeSitter(format!("wasm store: {e:?}")))?;
                    let wasm_bytes = std::fs::read(&dl.wasm_path)
                        .map_err(|e| Error::TreeSitter(format!("reading grammar.wasm: {e}")))?;
                    let lang = store.load_language(&dl.name, &wasm_bytes).map_err(|e| {
                        Error::TreeSitter(format!("loading grammar '{}': {e:?}", dl.name))
                    })?;
                    inner
                        .set_wasm_store(store)
                        .map_err(|e| Error::TreeSitter(format!("attaching wasm store: {e}")))?;
                    inner
                        .set_language(&lang)
                        .map_err(|e| Error::TreeSitter(format!("failed to set language: {e}")))?;
                    lang
                }
                #[cfg(not(feature = "wasm"))]
                {
                    return Err(Error::TreeSitter(format!(
                        "dynamic grammar '{}' requires the 'wasm' feature — rebuild codemark with --features wasm",
                        dl.name
                    )));
                }
            }
        };
        Ok(Parser { inner, language, ts_language })
    }

    /// The codemark [`Language`] this parser was built for.
    pub fn codemark_language(&self) -> &Language {
        &self.language
    }

    /// The underlying tree-sitter language handle, for compiling/running
    /// queries against trees this parser produces.
    ///
    /// For static grammars this is the compile-time language; for dynamic
    /// (WASM) grammars it is the handle loaded into the parser's WASM store,
    /// valid for the lifetime of this `Parser`.
    pub fn language(&self) -> &tree_sitter::Language {
        &self.ts_language
    }

    pub fn parse(&mut self, source: &[u8]) -> Result<tree_sitter::Tree> {
        self.inner
            .parse(source, None)
            .ok_or_else(|| Error::TreeSitter("failed to parse source".into()))
    }
    pub async fn parse_file(
        &mut self,
        path: &Path,
        provider: &dyn crate::vfs::FileProvider,
    ) -> Result<(tree_sitter::Tree, String)> {
        let source = provider.read_file(path, None).await?;
        let tree = self.parse(source.as_bytes())?;
        Ok((tree, source))
    }
}

/// Process-global wasmtime engine, shared by all WASM-loaded grammars.
#[cfg(feature = "wasm")]
fn wasm_engine() -> &'static tree_sitter::wasmtime::Engine {
    static ENGINE: std::sync::LazyLock<tree_sitter::wasmtime::Engine> =
        std::sync::LazyLock::new(tree_sitter::wasmtime::Engine::default);
    &ENGINE
}

/// A cached parse: the tree, its source, and the file mtime it was parsed at.
struct CachedTree {
    tree: tree_sitter::Tree,
    source: String,
    /// File modification time when this was parsed, or `None` for providers that
    /// report no mtime (treated as immutable, e.g. commit-pinned reads).
    mtime: Option<std::time::SystemTime>,
}

/// Cache of parsed trees, keyed by canonical path and invalidated by mtime.
///
/// Long-lived in the TUI (a session-level cache), so scrolling through bookmarks
/// in the same file reuses the parse tree instead of re-reading and re-parsing
/// on every selection. Entries are re-parsed only when the file's mtime changes,
/// so external edits are still picked up.
pub struct ParseCache {
    trees: HashMap<PathBuf, CachedTree>,
    parser: Parser,
}

impl ParseCache {
    pub fn new(language: Language) -> Result<Self> {
        Ok(ParseCache { trees: HashMap::new(), parser: Parser::new(language.clone())? })
    }

    /// Get the parsed tree and source for a file, parsing it if not already
    /// cached or if the file has changed on disk since it was last parsed.
    pub async fn get_or_parse(
        &mut self,
        path: &Path,
        provider: &dyn crate::vfs::FileProvider,
    ) -> Result<(&tree_sitter::Tree, &String)> {
        let canonical: std::path::PathBuf = provider.canonicalize(path).await;
        let current_mtime = provider.mtime(path).await;

        // Re-parse when there's no entry, or the file changed since we cached it.
        let needs_parse = match self.trees.get(&canonical) {
            Some(entry) => entry.mtime != current_mtime,
            None => true,
        };
        if needs_parse {
            let (tree, source) = self.parser.parse_file(path, provider).await?;
            self.trees.insert(canonical.clone(), CachedTree { tree, source, mtime: current_mtime });
        }

        let entry = self.trees.get(&canonical).unwrap();
        Ok((&entry.tree, &entry.source))
    }

    /// Clear all cached parse trees so the next `get_or_parse` call re-reads
    /// from disk. The parser (grammar) is retained.
    ///
    /// Rarely needed now that [`get_or_parse`](Self::get_or_parse) invalidates by
    /// mtime; kept for callers that want to force a cold re-read.
    pub fn clear_trees(&mut self) {
        self.trees.clear();
    }

    pub fn parser_mut(&mut self) -> &mut Parser {
        &mut self.parser
    }

    /// The codemark [`Language`] this cache parses for.
    pub fn codemark_language(&self) -> &Language {
        self.parser.codemark_language()
    }

    /// The underlying tree-sitter language handle, for compiling/running
    /// queries against trees this cache produces. Delegates to the parser —
    /// see [`Parser::language`].
    pub fn language(&self) -> &tree_sitter::Language {
        self.parser.language()
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
        let mut parser = Parser::new(Language::Swift.clone()).unwrap();
        let source = b"func hello() { print(\"hello\") }";
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[test]
    fn parse_rust_source() {
        let mut parser = Parser::new(Language::Rust.clone()).unwrap();
        let source = b"fn hello() { println!(\"hello\"); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parse_typescript_source() {
        let mut parser = Parser::new(Language::TypeScript.clone()).unwrap();
        let source = b"function hello(): void { console.log('hello'); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn parse_python_source() {
        let mut parser = Parser::new(Language::Python.clone()).unwrap();
        let source = b"def hello():\n    print('hello')\n";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "module");
        assert!(!tree.root_node().has_error());
    }

    #[tokio::test]
    async fn parse_swift_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(Language::Swift.clone()).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, _source) = parser.parse_file(&fixture, &provider).await.unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[tokio::test]
    async fn parse_cache_reuses_tree() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut cache = ParseCache::new(Language::Swift).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree1, _) = cache.get_or_parse(&fixture, &provider).await.unwrap();
        let root1_kind = tree1.root_node().kind().to_string();
        let (tree2, _) = cache.get_or_parse(&fixture, &provider).await.unwrap();
        let root2_kind = tree2.root_node().kind().to_string();
        assert_eq!(root1_kind, root2_kind);
    }

    /// Loads the committed `lua/grammar.wasm` fixture via the WASM store and
    /// parses Lua source. Validates the Phase 2 dynamic-loading path end to end.
    /// Skipped (not failed) when the fixture is absent so `--no-default-features`
    /// builds without the wasm grammars checked out still pass.
    #[cfg(feature = "wasm")]
    #[test]
    fn parse_dynamic_lua_grammar() {
        let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/grammars/lua/grammar.wasm");
        if !wasm_path.exists() {
            eprintln!("skipping: lua grammar fixture not found");
            return;
        }
        let dl = Arc::new(DynamicLanguage {
            name: "lua".into(),
            extensions: vec!["lua".into()],
            wasm_path,
            profile: Profile::default(),
        });
        let mut parser = Parser::new(Language::Dynamic(dl)).expect("wasm parser");
        let source = b"local function add(a, b)\n  return a + b\nend\n";
        let tree = parser.parse(source).expect("parse lua");
        assert!(!tree.root_node().has_error(), "lua parse produced ERROR nodes");
    }
}
