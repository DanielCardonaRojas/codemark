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
/// Only Dart is still statically compiled — it has no prebuilt WASM in the
/// registry yet. Every other language is a [`Dynamic`] WASM grammar loaded at
/// runtime from the cache directory and installed from the grammar registry
/// (`codemark languages install <lang>`). Two `Language`
/// values are equal if they are the same kind of grammar
/// ([`identity`](Self::identity)) with the same [`name`](Self::name) —
/// deliberately *not* full struct equality, so `Language` can serve as a
/// `HashMap` key despite `DynamicLanguage` carrying heavy data.
#[derive(Debug, Clone)]
pub enum Language {
    Dart,
    /// A dynamically loaded WASM grammar.
    Dynamic(Arc<DynamicLanguage>),
}

/// Which *kind* of grammar a [`Language`] is — the half of a grammar's identity
/// that its name does not carry.
///
/// A bookmark records a language name, and a dynamic grammar may share that name
/// with the built-in it overrides. So when an override claims only some of the
/// built-in's extensions, creation (by extension) and resolution (by name) can
/// select different grammars while both report the same name. Storing this
/// alongside the name is what lets resolution ask for the grammar that actually
/// generated the query. See <https://github.com/DanielCardonaRojas/codemark/issues/244>.
///
/// Serialized as `"builtin"` / `"dynamic"`; that string is a stored value, so
/// treat the spellings as a wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrammarIdentity {
    /// A grammar compiled into the binary.
    Builtin,
    /// A WASM grammar loaded at runtime from the cache directory.
    Dynamic,
}

impl GrammarIdentity {
    /// The stored spelling. Kept as one function so the DB value, the pack wire
    /// format, and [`FromStr`] can't drift apart.
    pub fn as_str(&self) -> &'static str {
        match self {
            GrammarIdentity::Builtin => "builtin",
            GrammarIdentity::Dynamic => "dynamic",
        }
    }
}

impl fmt::Display for GrammarIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for GrammarIdentity {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "builtin" => Ok(GrammarIdentity::Builtin),
            "dynamic" => Ok(GrammarIdentity::Dynamic),
            other => Err(Error::Input(format!(
                "unknown grammar identity '{other}' (expected 'builtin' or 'dynamic')"
            ))),
        }
    }
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
// Identity-based equality and hashing — two Languages are equal if they are the
// same *kind* of grammar with the same name. This lets Language serve as a
// HashMap key even though DynamicLanguage carries heavy data (wasm_path,
// profile, etc.).
//
// The kind is part of the key, not just the name. A dynamic grammar overriding a
// built-in shares its name, so comparing names alone makes the two
// indistinguishable — and `HashMap<Language, ParseCache>` (the TUI's per-language
// parser cache) would then hand a bookmark the *other* grammar's parser,
// defeating the `GrammarIdentity` a bookmark records for exactly this reason.
// Queries are grammar-specific, so a built-in bookmark parsed by an override's
// grammar can silently fail to match.
impl PartialEq for Language {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity() && self.name() == other.name()
    }
}
impl Eq for Language {}
impl Hash for Language {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity().hash(state);
        self.name().hash(state);
    }
}

impl Language {
    /// The canonical name string for this language (e.g. "rust", "lua").
    pub fn name(&self) -> &str {
        match self {
            Language::Dart => "dart",
            Language::Dynamic(dl) => &dl.name,
        }
    }

    /// Whether this is a compiled-in or a runtime-loaded grammar.
    ///
    /// Together with [`name`](Self::name) this is the grammar's identity, which
    /// is what a bookmark needs to record in order to be resolved against the
    /// same grammar that generated its query.
    pub fn identity(&self) -> GrammarIdentity {
        match self {
            Language::Dynamic(_) => GrammarIdentity::Dynamic,
            _ => GrammarIdentity::Builtin,
        }
    }

    /// Resolve a stored grammar identity — a name plus the kind of grammar it
    /// named when the query was generated.
    ///
    /// This is the lookup resolution should use, and the fix for the divergence
    /// documented on [`from_str`](Self::from_str): `from_str` has only a name to
    /// go on, so it must guess, and it guesses dynamic-first. Given the recorded
    /// kind there is nothing to guess.
    ///
    /// `identity` is a *preference*, not a requirement. When the recorded
    /// grammar is gone — a dynamic override since uninstalled, or a built-in
    /// since unbundled — this falls back to whatever now answers to the name.
    /// That may re-introduce the mismatch the identity exists to prevent, but
    /// the alternative is reporting a resolvable bookmark as having no grammar
    /// at all. A grammar swapped underneath a bookmark is a drift to detect, not
    /// a reason to refuse to look.
    ///
    /// **Without the `wasm` feature a recorded `Dynamic` is ignored**, for the
    /// same reason [`from_extension`](Self::from_extension) prefers the built-in
    /// in that build: `Parser::new` cannot load a WASM grammar there, so
    /// honouring the record would hand back a grammar that fails to parse and
    /// break a bookmark the built-in could still resolve. A grammar that can't
    /// be loaded is not a usable answer, however faithfully it was recorded.
    pub fn from_identity(name: &str, identity: GrammarIdentity) -> Result<Self> {
        let exact = match identity {
            GrammarIdentity::Builtin => Self::static_from_name(name),
            GrammarIdentity::Dynamic if cfg!(feature = "wasm") => {
                crate::parser::registry::LanguageRegistry::from_name(name)
            }
            GrammarIdentity::Dynamic => None,
        };
        if let Some(lang) = exact {
            return Ok(lang);
        }
        tracing::debug!(
            target: "codemark::languages",
            name = %name,
            wanted = %identity,
            "recorded grammar is unavailable; falling back to name-only resolution"
        );
        Self::from_str(name)
    }

    /// The tree-sitter language handle, for compiling and running queries.
    ///
    /// **Panics** if a dynamic grammar fails to load; use
    /// [`load_tree_sitter_language`](Self::load_tree_sitter_language) where that
    /// is a possibility to handle rather than a bug.
    pub fn tree_sitter_language(&self) -> tree_sitter::Language {
        self.load_tree_sitter_language()
            .unwrap_or_else(|e| panic!("loading the {} grammar: {e}", self.name()))
    }

    /// The tree-sitter language handle, loading a dynamic grammar's WASM if
    /// this process has not already.
    ///
    /// A handle loaded from WASM is self-contained for queries, and parsing
    /// with it only needs a parser that has a `WasmStore` attached —
    /// [`Parser::new`] attaches one. It is cached per grammar file, so the
    /// module is compiled once per process rather than once per parser.
    pub fn load_tree_sitter_language(&self) -> Result<tree_sitter::Language> {
        match self {
            Language::Dart => Ok(tree_sitter_dart::LANGUAGE.into()),
            Language::Dynamic(dl) => {
                #[cfg(feature = "wasm")]
                {
                    load_wasm_language(dl)
                }
                #[cfg(not(feature = "wasm"))]
                {
                    Err(Error::TreeSitter(format!(
                        "dynamic grammar '{}' requires the 'wasm' feature — rebuild codemark with --features wasm",
                        dl.name
                    )))
                }
            }
        }
    }

    pub fn file_extensions(&self) -> Vec<String> {
        match self {
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
    /// how seven of the eight bundled grammars were unbundled in 0.8.0.
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
            "dart" => Some(Language::Dart),
            _ => None,
        }
    }

    /// List all built-in static languages.
    pub fn builtins() -> Vec<Self> {
        vec![Language::Dart]
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
    /// # A name is not an identity
    ///
    /// A name alone cannot say *which* grammar is meant when an override shares
    /// it with the built-in, so this has to guess, and it guesses dynamic-first
    /// to match [`from_extension`](Self::from_extension). The guess is wrong for
    /// an extension the override doesn't claim: a dynamic `typescript` claiming
    /// only `.ts` leaves a `.tsx` file resolving to the built-in — which stores
    /// `"typescript"`, and that name resolves back to the dynamic grammar.
    ///
    /// Prefer [`from_identity`](Self::from_identity) wherever a recorded
    /// [`GrammarIdentity`] is available, which is the case for any bookmark
    /// created since schema v25. Use this only for a bare name from a user
    /// (`--lang`) or a bookmark predating that column.
    fn from_str(s: &str) -> Result<Self> {
        let s = canonical_name(s);
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

/// Spellings that name an unbundled language, mapped to its registry name.
///
/// Before 0.8.0 these languages were compiled in and answered to aliases
/// (`rs`, `ts`, `py`, `cs`) and, for C#, a name the registry spells
/// differently (`csharp` vs `c_sharp`). Bookmarks and shared packs created then
/// store those spellings, and users still type them for `--lang`, so a name is
/// canonicalized before the registry lookup. Spellings are matched
/// case-insensitively; anything not listed passes through unchanged.
const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("ts", "typescript"),
    ("py", "python"),
    ("csharp", "c_sharp"),
    ("c#", "c_sharp"),
    ("cs", "c_sharp"),
];

/// Extensions of the languages unbundled in 0.8.0, mapped to the registry
/// grammar that handles them.
///
/// These were parseable out of the box before 0.8.0, so meeting one with no
/// grammar installed is the common upgrade case, not an unknown file type. This
/// table lets that be reported as a missing grammar *with its name*, which
/// tells the user (or the agent skill) exactly what to install.
const UNBUNDLED_EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("swift", "swift"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("py", "python"),
    ("go", "go"),
    ("java", "java"),
    ("cs", "c_sharp"),
];

impl Language {
    /// The registry grammar to install for a file extension that was built in
    /// before 0.8.0, if `ext` is one. See [`UNBUNDLED_EXTENSIONS`].
    pub fn unbundled_grammar_for_extension(ext: &str) -> Option<&'static str> {
        UNBUNDLED_EXTENSIONS
            .iter()
            .find(|(e, _)| e.eq_ignore_ascii_case(ext))
            .map(|(_, name)| *name)
    }
}

/// The registry name for `name`, resolving the pre-0.8.0 aliases in
/// [`LANGUAGE_ALIASES`].
pub fn canonical_name(name: &str) -> &str {
    LANGUAGE_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, canonical)| *canonical)
        .unwrap_or(name)
}

/// Every stored spelling that refers to the same language as `name`: the
/// canonical name plus each alias of it. A bookmark stores the name its grammar
/// reported when it was created, so filtering C# bookmarks must match both the
/// pre-0.8.0 `csharp` and the registry's `c_sharp`.
pub fn stored_spellings(name: &str) -> Vec<String> {
    let canonical = canonical_name(name);
    let mut out = vec![canonical.to_string()];
    out.extend(
        LANGUAGE_ALIASES
            .iter()
            .filter(|(_, c)| c.eq_ignore_ascii_case(canonical))
            .map(|(alias, _)| alias.to_string()),
    );
    out
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
        let ts_language = language.load_tree_sitter_language()?;
        // A WASM language is compiled once and cached (see
        // `load_wasm_language`), but each parser still needs a store of its own
        // to instantiate it into: stores are not shareable across parsers.
        #[cfg(feature = "wasm")]
        if ts_language.is_wasm() {
            let store = tree_sitter::WasmStore::new(wasm_engine())
                .map_err(|e| Error::TreeSitter(format!("wasm store: {e:?}")))?;
            inner
                .set_wasm_store(store)
                .map_err(|e| Error::TreeSitter(format!("attaching wasm store: {e}")))?;
        }
        inner
            .set_language(&ts_language)
            .map_err(|e| Error::TreeSitter(format!("failed to set language: {e}")))?;
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

/// Load (or fetch from the process cache) the tree-sitter language for a
/// dynamic grammar.
///
/// Compiling a grammar's WASM is the expensive step — hundreds of milliseconds
/// for a large grammar like Swift — and it used to happen on every
/// `Parser::new`. The handle is now kept per grammar file, keyed by path, size
/// and mtime so a reinstalled grammar is picked up rather than served stale.
#[cfg(feature = "wasm")]
fn load_wasm_language(dl: &DynamicLanguage) -> Result<tree_sitter::Language> {
    use std::sync::{LazyLock, Mutex};
    use std::time::SystemTime;

    type Key = (PathBuf, u64, Option<SystemTime>);
    static CACHE: LazyLock<Mutex<HashMap<Key, tree_sitter::Language>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let meta = std::fs::metadata(&dl.wasm_path)
        .map_err(|e| Error::TreeSitter(format!("reading grammar.wasm: {e}")))?;
    let key: Key = (dl.wasm_path.clone(), meta.len(), meta.modified().ok());

    // Held across the compile so two threads asking for the same grammar don't
    // both pay for it. A compile is a one-off per grammar per process.
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(lang) = cache.get(&key) {
        return Ok(lang.clone());
    }

    let started = std::time::Instant::now();
    let wasm_bytes = std::fs::read(&dl.wasm_path)
        .map_err(|e| Error::TreeSitter(format!("reading grammar.wasm: {e}")))?;
    let mut store = tree_sitter::WasmStore::new(wasm_engine())
        .map_err(|e| Error::TreeSitter(format!("wasm store: {e:?}")))?;
    let lang = store
        .load_language(&dl.name, &wasm_bytes)
        .map_err(|e| Error::TreeSitter(format!("loading grammar '{}': {e:?}", dl.name)))?;
    tracing::debug!(
        target: "codemark::languages",
        name = %dl.name,
        path = %dl.wasm_path.display(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "compiled wasm grammar"
    );

    // Drop any handle for an older build of this same file.
    cache.retain(|(path, _, _), _| path != &dl.wasm_path);
    cache.insert(key, lang.clone());
    Ok(lang)
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

    /// The pre-0.8.0 names and aliases of the unbundled languages still
    /// resolve — now to the registry grammar. Bookmarks and shared packs
    /// created before 0.8.0 store these spellings.
    #[test]
    fn unbundled_languages_resolve_by_their_old_names() {
        use super::Language;
        use std::str::FromStr;

        for (name, canonical) in [
            ("rust", "rust"),
            ("rs", "rust"),
            ("swift", "swift"),
            ("typescript", "typescript"),
            ("ts", "typescript"),
            // A grammar of its own since the registry gained one — no longer
            // an alias of `typescript`, which cannot parse JSX.
            ("tsx", "tsx"),
            ("python", "python"),
            ("py", "python"),
            ("go", "go"),
            ("java", "java"),
            ("csharp", "c_sharp"),
            ("C#", "c_sharp"),
            ("cs", "c_sharp"),
        ] {
            let lang =
                Language::from_str(name).unwrap_or_else(|e| panic!("{name} should resolve: {e}"));
            assert_eq!(lang.name(), canonical, "{name}");
            assert_eq!(lang.identity(), GrammarIdentity::Dynamic, "{name}");
        }
    }

    /// A bookmark recorded against a built-in that has since been unbundled
    /// still resolves: the recorded identity is a preference, and the name
    /// falls through to the registry grammar that replaced it.
    #[test]
    fn a_builtin_identity_for_an_unbundled_language_falls_back_to_the_registry() {
        let lang = Language::from_identity("csharp", GrammarIdentity::Builtin).unwrap();
        assert_eq!(lang.name(), "c_sharp");
        assert_eq!(lang.identity(), GrammarIdentity::Dynamic);
    }

    #[test]
    fn an_unbundled_extension_names_the_grammar_to_install() {
        assert_eq!(Language::unbundled_grammar_for_extension("rs"), Some("rust"));
        assert_eq!(Language::unbundled_grammar_for_extension("CS"), Some("c_sharp"));
        assert_eq!(Language::unbundled_grammar_for_extension("lua"), None);
        assert_eq!(Language::unbundled_grammar_for_extension("dart"), None);
        // Every name it suggests must be one install will accept.
        for (_, name) in UNBUNDLED_EXTENSIONS {
            assert_eq!(canonical_name(name), *name, "{name} must be canonical");
        }
    }

    #[test]
    fn stored_spellings_cover_the_canonical_name_and_its_aliases() {
        let mut csharp = stored_spellings("csharp");
        csharp.sort();
        assert_eq!(csharp, ["c#", "c_sharp", "cs", "csharp"]);
        assert_eq!(stored_spellings("lua"), ["lua"]);

        // A canonical name in another case still pulls in its aliases.
        let mut upper = stored_spellings("C_SHARP");
        upper.sort();
        assert_eq!(upper, ["C_SHARP", "c#", "cs", "csharp"]);
    }

    /// Precedence must follow what the build can actually load.
    ///
    /// With `wasm`, a dynamic grammar wins — that is what lets a bundled
    /// language be replaced by a registry one. Without it, `Parser::new`
    /// rejects every dynamic grammar, so preferring one would route `.dart` to
    /// a grammar that cannot parse and break a language that worked before the
    /// override was installed.
    #[test]
    fn builtin_resolution_survives_without_the_wasm_feature() {
        use super::Language;
        use std::str::FromStr;

        assert!(matches!(Language::static_from_extension("dart"), Some(Language::Dart)));
        assert!(matches!(Language::static_from_name("dart"), Some(Language::Dart)));

        // And the built-in remains reachable through the public lookups when
        // nothing overrides it — true in CI, and the no-default-features build
        // covers the other ordering.
        if crate::parser::registry::LanguageRegistry::from_extension("dart").is_none() {
            assert!(matches!(Language::from_extension("dart"), Some(Language::Dart)));
            assert!(matches!(Language::from_str("dart"), Ok(Language::Dart)));
        }
    }

    use super::*;

    /// A built-in and a dynamic grammar sharing a name are distinct map keys.
    ///
    /// The TUI caches parsers in a `HashMap<Language, ParseCache>`. If the key
    /// were the name alone, these two would collide and a bookmark would be
    /// handed the *other* grammar's parser — which would defeat the recorded
    /// `GrammarIdentity` at the last step, since queries are grammar-specific.
    #[test]
    fn an_override_and_its_builtin_are_different_cache_keys() {
        let dynamic = Language::Dynamic(Arc::new(DynamicLanguage {
            name: "dart".into(),
            extensions: vec!["dart".into()],
            wasm_path: PathBuf::from("/nonexistent"),
            profile: Profile::default(),
        }));
        let builtin = Language::Dart;

        assert_eq!(dynamic.name(), builtin.name(), "the premise: names collide");
        assert_ne!(dynamic, builtin, "but the languages must not compare equal");

        let mut caches: HashMap<Language, &str> = HashMap::new();
        caches.insert(builtin.clone(), "builtin parser");
        caches.insert(dynamic.clone(), "dynamic parser");
        assert_eq!(caches.len(), 2, "each grammar needs its own cache entry");
        assert_eq!(caches.get(&builtin), Some(&"builtin parser"));
        assert_eq!(caches.get(&dynamic), Some(&"dynamic parser"));
    }

    /// A recorded identity selects the grammar that generated the query, where a
    /// bare name cannot — the fix for #244.
    #[test]
    fn a_recorded_identity_pins_the_builtin_against_a_same_named_override() {
        // Only meaningful when nothing overrides `dart` in the grammar cache.
        if crate::parser::registry::LanguageRegistry::from_name("dart").is_some() {
            return;
        }

        // Recording `Builtin` asks for the compiled-in grammar by name...
        let pinned = Language::from_identity("dart", GrammarIdentity::Builtin).unwrap();
        assert!(matches!(pinned, Language::Dart));
        assert_eq!(pinned.identity(), GrammarIdentity::Builtin);

        // ...and an identity for a grammar that isn't installed falls back to
        // name-only resolution rather than failing: a bookmark whose override
        // was uninstalled is still worth trying to resolve.
        let fallback = Language::from_identity("dart", GrammarIdentity::Dynamic).unwrap();
        assert!(matches!(fallback, Language::Dart));
    }

    /// After a fallback, the parser reports the grammar it actually loaded.
    ///
    /// `Parser::new` degrades to the built-in when an override fails to load,
    /// so the requested language is no longer the one that will parse. Bookmark
    /// creation records `codemark_language()` for exactly this reason: filing a
    /// query generated against the built-in under the override's identity would
    /// send it back to the override once that grammar is repaired, and queries
    /// are grammar-specific.
    #[test]
    fn a_fallback_parser_reports_the_builtin_it_loaded() {
        let broken = || {
            // An override that cannot possibly load: the wasm file does not exist.
            Language::Dynamic(Arc::new(DynamicLanguage {
                name: "dart".into(),
                extensions: vec!["dart".into()],
                wasm_path: PathBuf::from("/nonexistent/grammar.wasm"),
                profile: Profile::default(),
            }))
        };
        assert_eq!(broken().identity(), GrammarIdentity::Dynamic, "the premise");

        let parser = Parser::new(broken()).expect("must fall back rather than fail");

        // What creation now saves — the built-in, not the override we asked for.
        assert_eq!(parser.codemark_language().identity(), GrammarIdentity::Builtin);
        assert!(matches!(parser.codemark_language(), Language::Dart));

        // `new_exact` is the contrast: install and `languages validate` must see
        // the failure instead of a grammar that silently became another one.
        assert!(Parser::new_exact(broken()).is_err());
    }

    /// An unbundled language with no grammar installed has no built-in floor:
    /// a broken grammar is an error, not a silent fallback.
    #[test]
    fn a_broken_unbundled_grammar_has_no_fallback() {
        let broken = Language::Dynamic(Arc::new(DynamicLanguage {
            name: "rust".into(),
            extensions: vec!["rs".into()],
            wasm_path: PathBuf::from("/nonexistent/grammar.wasm"),
            profile: Profile::default(),
        }));
        assert!(Parser::new(broken).is_err());
    }

    /// Two parsers for one WASM grammar share a single compiled language and
    /// still parse independently — each gets its own store.
    #[cfg(feature = "wasm")]
    #[test]
    fn parsers_share_a_compiled_wasm_grammar() {
        let swift = crate::testing::grammar("swift");
        let a = swift.tree_sitter_language();
        let b = swift.tree_sitter_language();
        assert!(a.is_wasm());
        assert_eq!(a.node_kind_count(), b.node_kind_count());

        let mut p1 = Parser::new(swift.clone()).unwrap();
        let mut p2 = Parser::new(swift).unwrap();
        let t1 = p1.parse(b"func a() {}").unwrap();
        let t2 = p2.parse(b"struct B {}").unwrap();
        assert!(!t1.root_node().has_error());
        assert!(!t2.root_node().has_error());
    }

    /// The stored spellings are a wire format: a value written by this version
    /// must still read back, since it lands in a DB column and in shared packs.
    #[test]
    fn grammar_identity_round_trips_through_its_stored_spelling() {
        for identity in [GrammarIdentity::Builtin, GrammarIdentity::Dynamic] {
            assert_eq!(identity.as_str().parse::<GrammarIdentity>().unwrap(), identity);
            // Serde uses the same spellings, so a pack and the DB agree.
            let json = serde_json::to_string(&identity).unwrap();
            assert_eq!(json, format!("\"{}\"", identity.as_str()));
        }
        assert_eq!("BUILTIN".parse::<GrammarIdentity>().unwrap(), GrammarIdentity::Builtin);
        assert!("wasm".parse::<GrammarIdentity>().is_err());
    }

    #[test]
    fn language_round_trip() {
        for (input, display) in [
            ("dart", "dart"),
            ("swift", "swift"),
            ("rust", "rust"),
            ("rs", "rust"),
            ("typescript", "typescript"),
            ("ts", "typescript"),
            ("python", "python"),
            ("py", "python"),
        ] {
            let lang: Language = input.parse().unwrap();
            assert_eq!(lang.to_string(), display, "parsing '{input}'");
            let again: Language = lang.to_string().parse().unwrap();
            assert_eq!(again, lang);
        }
    }

    #[test]
    fn unsupported_language_errors() {
        assert!("fortran".parse::<Language>().is_err());
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn parse_swift_source() {
        let mut parser = Parser::new(crate::testing::grammar("swift").clone()).unwrap();
        let source = b"func hello() { print(\"hello\") }";
        let tree = parser.parse(source).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn parse_rust_source() {
        let mut parser = Parser::new(crate::testing::grammar("rust").clone()).unwrap();
        let source = b"fn hello() { println!(\"hello\"); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "source_file");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn parse_typescript_source() {
        let mut parser = Parser::new(crate::testing::grammar("typescript").clone()).unwrap();
        let source = b"function hello(): void { console.log('hello'); }";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "program");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    fn parse_python_source() {
        let mut parser = Parser::new(crate::testing::grammar("python").clone()).unwrap();
        let source = b"def hello():\n    print('hello')\n";
        let tree = parser.parse(source).unwrap();
        assert_eq!(tree.root_node().kind(), "module");
        assert!(!tree.root_node().has_error());
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn parse_swift_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut parser = Parser::new(crate::testing::grammar("swift").clone()).unwrap();
        let provider = crate::vfs::LocalFileProvider;
        let (tree, _source) = parser.parse_file(&fixture, &provider).await.unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(!root.has_error());
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "wasm"), ignore = "needs the wasm feature to load registry grammars")]
    async fn parse_cache_reuses_tree() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/swift/auth_service.swift");
        if !fixture.exists() {
            return;
        }
        let mut cache = ParseCache::new(crate::testing::grammar("swift")).unwrap();
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
