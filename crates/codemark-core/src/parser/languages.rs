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

    /// Resolve a file extension to a language, **preferring a dynamic grammar**
    /// over the built-in that would otherwise claim it.
    ///
    /// An installed grammar is something the user went out of their way to add,
    /// so it reads as the more specific intent. It also makes a built-in
    /// *replaceable*: a language can be dropped from the binary and picked up
    /// from the registry without the extension becoming unresolvable, which is
    /// how the eight bundled grammars can be unbundled one at a time.
    ///
    /// Nothing is shadowed by accident — installing over a built-in takes an
    /// explicit `--override-builtin` (see
    /// [`validate_name_and_extensions`](crate::grammar::validate_name_and_extensions)).
    ///
    /// **Without the `wasm` feature the built-in wins instead.** `Parser::new`
    /// cannot load a dynamic grammar in that build, so preferring one would
    /// route `.rs` to a grammar that then fails to parse — breaking a language
    /// that worked before the override was installed. A grammar that can't be
    /// loaded is not a usable override.
    pub fn from_extension(ext: &str) -> Option<Self> {
        if cfg!(feature = "wasm") {
            crate::parser::registry::LanguageRegistry::from_extension(ext)
                .or_else(|| Self::static_from_extension(ext))
        } else {
            Self::static_from_extension(ext)
                .or_else(|| crate::parser::registry::LanguageRegistry::from_extension(ext))
        }
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

    /// Resolve a language name, preferring a dynamic grammar over a built-in of
    /// the same name — matching [`Language::from_extension`], so a stored
    /// bookmark and a fresh one agree on which grammar a name refers to.
    ///
    /// Falls back to built-in-first without the `wasm` feature, for the same
    /// reason: an unloadable grammar is not a usable override.
    ///
    /// # Known gap
    ///
    /// A bookmark records only the language *name*, so the two lookups can
    /// disagree for an extension the override doesn't claim. A dynamic
    /// `typescript` claiming only `.ts` leaves a `.tsx` file resolving to the
    /// built-in — which stores `"typescript"`, and that name later resolves to
    /// the dynamic grammar.
    ///
    /// Closing it properly means recording grammar identity on the bookmark,
    /// which is a schema change. Until then the narrower mitigation is to claim
    /// every extension of the built-in you override, which
    /// `--override-builtin` makes visible at install time.
    fn from_str(s: &str) -> Result<Self> {
        let resolved = if cfg!(feature = "wasm") {
            crate::parser::registry::LanguageRegistry::from_name(s)
                .or_else(|| Language::static_from_name(s))
        } else {
            Language::static_from_name(s)
                .or_else(|| crate::parser::registry::LanguageRegistry::from_name(s))
        };
        resolved.ok_or_else(|| Error::GrammarNotInstalled { language: s.to_string() })
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
    /// Build a parser, falling back to the built-in when a dynamic grammar
    /// that *overrides* one fails to load.
    ///
    /// Discovery registers a grammar on the strength of a readable manifest and
    /// an existing `grammar.wasm`; it does not prove the module loads. Since a
    /// dynamic grammar now wins resolution, a corrupt or ABI-incompatible
    /// override would otherwise make its extension unparseable — worse than the
    /// state before the override was installed. Falling back keeps the built-in
    /// as a floor.
    pub fn new(language: Language) -> Result<Self> {
        let fallback = match &language {
            Language::Dynamic(dl) => Language::static_from_name(&dl.name)
                .or_else(|| dl.extensions.iter().find_map(|e| Language::static_from_extension(e))),
            _ => None,
        };
        match Self::new_exact(language) {
            Ok(p) => Ok(p),
            Err(e) => match fallback {
                Some(builtin) => {
                    eprintln!(
                        "codemark: dynamic grammar failed to load ({e}); falling back to the \
                         built-in {} parser",
                        builtin.name()
                    );
                    Self::new_exact(builtin)
                }
                None => Err(e),
            },
        }
    }

    /// [`Parser::new`] without the built-in fallback.
    ///
    /// Installation and `languages validate` must use this: the fallback exists
    /// so a *running* session degrades gracefully, but it would make a broken
    /// grammar look installable and report it as valid.
    pub fn new_exact(language: Language) -> Result<Self> {
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
    /// Build a parser, falling back to the built-in when a dynamic grammar
    /// that *overrides* one fails to load.
    ///
    /// Discovery registers a grammar on the strength of a readable manifest and
    /// an existing `grammar.wasm`; it does not prove the module loads. Since a
    /// dynamic grammar now wins resolution, a corrupt or ABI-incompatible
    /// override would otherwise make its extension unparseable — worse than the
    /// state before the override was installed. Falling back keeps the built-in
    /// as a floor.
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
    /// A language with no registered grammar is reported as something the user
    /// can fix, not as bad input.
    ///
    /// The distinction matters because a bookmark whose grammar is missing is
    /// not broken — it is unreadable until the language is installed. Callers
    /// use the variant to say "No grammar" rather than "Unmatched", so
    /// flattening it into `Error::Input` anywhere makes a healthy collection
    /// look rotten. See #257.
    #[test]
    fn a_missing_grammar_is_its_own_error() {
        use super::Language;
        use crate::error::Error;
        use std::str::FromStr;

        // A name no built-in claims and no grammar could plausibly be
        // installed under, so the result does not depend on what the
        // developer running the tests happens to have in their cache.
        let err = Language::from_str("not-a-real-language-xyz").unwrap_err();

        assert!(
            matches!(err, Error::GrammarNotInstalled { ref language }
                     if language == "not-a-real-language-xyz"),
            "expected GrammarNotInstalled, got {err:?}"
        );

        // The message has to name the fix: this is the most common first
        // encounter with a shared tour for a language you lack.
        let shown = err.to_string();
        assert!(shown.contains("not-a-real-language-xyz"), "{shown}");

        // `install` is offered before `add`: it downloads the grammar, whereas
        // `add` requires a .wasm the user must find or build first. Leading
        // with `add` sends people off to do work the CLI can do for them.
        assert!(shown.contains("languages install"), "{shown}");
        assert!(shown.contains("languages add"), "{shown}");
        let install_at = shown.find("languages install").expect("install mentioned");
        let add_at = shown.find("languages add").expect("add mentioned");
        assert!(install_at < add_at, "install should be recommended before add: {shown}");
    }

    /// Built-in languages keep resolving by every alias they advertise.
    #[test]
    fn builtin_languages_still_resolve_by_name() {
        use super::Language;
        use std::str::FromStr;

        for name in ["rust", "rs", "swift", "typescript", "ts", "python", "py", "go", "cs"] {
            assert!(Language::from_str(name).is_ok(), "{name} should resolve");
        }
    }

    /// Precedence must follow what the build can actually load.
    ///
    /// With `wasm`, a dynamic grammar wins — that is what lets a bundled
    /// language be replaced by a registry one. Without it, `Parser::new`
    /// rejects every dynamic grammar, so preferring one would route `.rs` to a
    /// grammar that cannot parse and break a language that worked before the
    /// override was installed.
    #[test]
    fn builtin_resolution_survives_without_the_wasm_feature() {
        use super::Language;
        use std::str::FromStr;

        // Deliberately the *static* helpers: `from_extension` / `from_str`
        // consult the user's real grammar cache, so a `rust` override on the
        // machine running the tests would fail these. What matters here is
        // that the built-in arm still maps correctly.
        assert!(matches!(Language::static_from_extension("rs"), Some(Language::Rust)));
        assert!(matches!(Language::static_from_name("rust"), Some(Language::Rust)));
        assert!(matches!(Language::static_from_name("rs"), Some(Language::Rust)));

        // And the built-in remains reachable through the public lookups when
        // nothing overrides it — true in CI, and the no-default-features build
        // covers the other ordering.
        if crate::parser::registry::LanguageRegistry::from_extension("rs").is_none() {
            assert!(matches!(Language::from_extension("rs"), Some(Language::Rust)));
            assert!(matches!(Language::from_str("rust"), Ok(Language::Rust)));
        }
    }

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
