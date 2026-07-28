//! Dynamic language registry — discovers WASM grammars from the cache directory.
//!
//! Each `<lang>/` under [`global_grammars_dir`] holds a `grammar.wasm` and a
//! `manifest.json`. At startup, [`LanguageRegistry::discover`] scans the
//! directory, parses manifests, and builds the extension/name → `Language`
//! lookup tables used by [`Language::from_extension`] and
//! [`Language::from_str`].

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::RwLock;

use serde::Deserialize;

use crate::config::global_grammars_dir;
use crate::parser::languages::{DynamicLanguage, Language};
use crate::parser::profile::Profile;

/// The process-global registry, discovered on first access and re-buildable via
/// [`LanguageRegistry::refresh`]. Guarded by an `RwLock` so long-running
/// consumers (e.g. the TUI) can pick up grammars added/removed after startup
/// instead of retaining the original snapshot until the process restarts.
static GLOBAL_REGISTRY: LazyLock<RwLock<LanguageRegistry>> =
    LazyLock::new(|| RwLock::new(LanguageRegistry::discover()));

/// Registry of dynamically loaded languages, indexed by extension and name.
pub struct LanguageRegistry {
    by_extension: HashMap<String, Language>,
    by_name: HashMap<String, Language>,
}

impl LanguageRegistry {
    /// Scan the grammar cache directory and build the registry.
    ///
    /// Invalid manifests or missing `.wasm` files are skipped with a warning —
    /// they never prevent codemark from starting.
    fn discover() -> Self {
        let Some(grammar_dir) = global_grammars_dir() else {
            return Self { by_extension: HashMap::new(), by_name: HashMap::new() };
        };

        // Auto-create the directory so users have a discoverable place to drop WASM files
        if !grammar_dir.exists() {
            let _ = std::fs::create_dir_all(&grammar_dir);
        }

        // A read failure yields an empty registry at startup — there's nothing to
        // preserve yet. (`refresh` treats the same `None` differently: it keeps
        // the working registry rather than blanking it.)
        Self::discover_in(&grammar_dir)
            .unwrap_or_else(|| Self { by_extension: HashMap::new(), by_name: HashMap::new() })
    }

    /// Scan a specific grammar directory and build the registry.
    ///
    /// Returns `None` when the directory itself can't be read (so callers can
    /// distinguish "no grammars" from "couldn't scan"); an unreadable entry or
    /// invalid manifest inside a readable directory is skipped, not fatal.
    ///
    /// Split out from [`discover`](Self::discover) so the scan/skip logic can be
    /// tested against a fixture directory without touching the global cache.
    fn discover_in(grammar_dir: &std::path::Path) -> Option<Self> {
        let mut registry = Self { by_extension: HashMap::new(), by_name: HashMap::new() };

        let entries = std::fs::read_dir(grammar_dir).ok()?;

        // Sort entries by path so discovery is deterministic across processes:
        // filesystem iteration order is unspecified, so without this an
        // extension claimed by two grammars could resolve differently on
        // different runs. On conflict the first grammar (by sorted path) wins.
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let manifest_path = entry.path().join("manifest.json");
            let wasm_path = entry.path().join("grammar.wasm");

            let Ok(manifest_text) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };

            match serde_json::from_str::<Manifest>(&manifest_text) {
                Ok(manifest) => {
                    // Skip if name collides with a built-in static language.
                    if is_static_language_name(&manifest.name) {
                        eprintln!(
                            "codemark: skipping dynamic grammar '{}' — name conflicts with a built-in language",
                            manifest.name
                        );
                        continue;
                    }

                    if !wasm_path.exists() {
                        eprintln!(
                            "codemark: skipping dynamic grammar '{}' — grammar.wasm not found",
                            manifest.name
                        );
                        continue;
                    }

                    let dl = Arc::new(DynamicLanguage {
                        name: manifest.name.clone(),
                        extensions: manifest.extensions.clone(),
                        wasm_path: wasm_path.clone(),
                        profile: manifest.profile.unwrap_or_default(),
                    });

                    let lang = Language::Dynamic(dl);

                    // First grammar to claim a name wins, matching the extension
                    // rule below. Otherwise a duplicate name could win name-based
                    // lookup while the first still wins extension lookup, so
                    // creation (by extension) and resolution (by stored name)
                    // could select different grammars.
                    if let Some(existing) = registry.by_name.get(&manifest.name) {
                        eprintln!(
                            "codemark: language name '{}' already registered by grammar '{}' — ignoring duplicate",
                            manifest.name,
                            existing.name()
                        );
                        continue;
                    }
                    registry.by_name.insert(manifest.name.clone(), lang.clone());

                    for ext in &manifest.extensions {
                        let key = ext.to_lowercase();
                        // Keep the first grammar to claim an extension (entries
                        // are sorted, so this is deterministic) and warn rather
                        // than silently overwriting with a nondeterministic winner.
                        if let Some(existing) = registry.by_extension.get(&key) {
                            eprintln!(
                                "codemark: extension '.{key}' already claimed by grammar '{}' — ignoring '{}'",
                                existing.name(),
                                manifest.name
                            );
                            continue;
                        }
                        registry.by_extension.insert(key, lang.clone());
                    }

                    tracing::debug!(
                        target: "codemark::registry",
                        name = %manifest.name,
                        extensions = ?manifest.extensions,
                        "registered dynamic grammar"
                    );
                }
                Err(e) => {
                    eprintln!(
                        "codemark: skipping invalid manifest at {}: {e}",
                        manifest_path.display()
                    );
                }
            }
        }

        Some(registry)
    }

    /// Re-scan the grammar cache directory, replacing the process-global
    /// registry. Long-running consumers should call this when they want to pick
    /// up grammars added or removed since startup (or since the last refresh).
    ///
    /// If the grammar directory can't be read (e.g. a transient FS error), the
    /// existing registry is kept rather than blanked, so already-resolved
    /// dynamic-language bookmarks don't suddenly become unsupported.
    pub fn refresh() {
        let Some(dir) = global_grammars_dir() else {
            return;
        };
        match Self::discover_in(&dir) {
            Some(fresh) => {
                *GLOBAL_REGISTRY.write().expect("grammar registry lock poisoned") = fresh;
            }
            None => {
                eprintln!(
                    "codemark: could not re-scan grammar cache {} — keeping current registry",
                    dir.display()
                );
            }
        }
    }

    /// Look up a dynamic language by file extension (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Language> {
        GLOBAL_REGISTRY
            .read()
            .expect("grammar registry lock poisoned")
            .by_extension
            .get(&ext.to_lowercase())
            .cloned()
    }

    /// Look up a dynamic language by name.
    pub fn from_name(name: &str) -> Option<Language> {
        GLOBAL_REGISTRY.read().expect("grammar registry lock poisoned").by_name.get(name).cloned()
    }

    /// List all registered dynamic language names.
    pub fn dynamic_language_names() -> Vec<String> {
        GLOBAL_REGISTRY
            .read()
            .expect("grammar registry lock poisoned")
            .by_name
            .keys()
            .cloned()
            .collect()
    }

    /// List all registered dynamic languages.
    pub fn dynamic_languages() -> Vec<Language> {
        GLOBAL_REGISTRY
            .read()
            .expect("grammar registry lock poisoned")
            .by_name
            .values()
            .cloned()
            .collect()
    }
}

/// Whether `name` matches a built-in static language.
fn is_static_language_name(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "swift"
            | "rust"
            | "typescript"
            | "ts"
            | "tsx"
            | "python"
            | "py"
            | "go"
            | "java"
            | "csharp"
            | "c#"
            | "cs"
            | "dart"
    )
}

/// The on-disk manifest format for a WASM grammar.
#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    extensions: Vec<String>,
    profile: Option<Profile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a `<dir>/<name>/{manifest.json,grammar.wasm}` fixture. When
    /// `with_wasm` is false the `.wasm` is omitted so we can exercise the
    /// missing-grammar skip path.
    fn write_grammar(dir: &std::path::Path, name: &str, manifest: &str, with_wasm: bool) {
        let lang_dir = dir.join(name);
        std::fs::create_dir_all(&lang_dir).unwrap();
        std::fs::write(lang_dir.join("manifest.json"), manifest).unwrap();
        if with_wasm {
            std::fs::write(lang_dir.join("grammar.wasm"), b"\0asm").unwrap();
        }
    }

    #[test]
    fn discover_registers_valid_grammar_by_name_and_extension() {
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{ "name": "lua", "extensions": ["lua", "LUA"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();

        // Name lookup is exact; extension lookup is case-insensitive.
        assert_eq!(reg.by_name.get("lua").map(|l| l.name()), Some("lua"));
        assert!(reg.by_extension.contains_key("lua"));
        // Extensions are lowercased on insert, so "LUA" collapses onto "lua".
        assert_eq!(reg.by_extension.get("lua").map(|l| l.name()), Some("lua"));
        assert!(!reg.by_extension.contains_key("LUA"));
    }

    #[test]
    fn discover_skips_name_colliding_with_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        // "Rust" (any case) collides with a built-in and must be skipped.
        write_grammar(
            tmp.path(),
            "rust",
            r#"{ "name": "Rust", "extensions": ["rs"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.is_empty());
        assert!(reg.by_extension.is_empty());
    }

    #[test]
    fn discover_skips_grammar_missing_wasm() {
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            false, // no grammar.wasm
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.is_empty());
    }

    #[test]
    fn discover_skips_invalid_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(tmp.path(), "broken", "{ not valid json", true);

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.is_empty());
    }

    #[test]
    fn discover_parses_manifest_profile_landmarks() {
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{
                "name": "lua",
                "extensions": ["lua"],
                "profile": { "landmark_kinds": ["local_function_declaration"] }
            }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        let lang = reg.by_name.get("lua").expect("lua registered");
        assert!(lang.profile().landmark_kinds.iter().any(|k| k == "local_function_declaration"));
    }

    #[test]
    fn duplicate_extension_resolves_deterministically_to_first_by_sorted_path() {
        // Two grammars claim ".foo". Entries are scanned in sorted path order,
        // so "aaa" wins over "zzz" regardless of filesystem iteration order,
        // and the same extension always selects the same grammar.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "zzz",
            r#"{ "name": "zzz", "extensions": ["foo"], "profile": {} }"#,
            true,
        );
        write_grammar(
            tmp.path(),
            "aaa",
            r#"{ "name": "aaa", "extensions": ["foo"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        // Both names still register; only the extension conflict is resolved.
        assert!(reg.by_name.contains_key("aaa"));
        assert!(reg.by_name.contains_key("zzz"));
        assert_eq!(reg.by_extension.get("foo").map(|l| l.name()), Some("aaa"));
    }

    #[test]
    fn duplicate_name_keeps_first_by_sorted_path() {
        // Two grammars declare the same name under different directories. The
        // first by sorted path ("a_dir") wins name lookup, matching extension
        // resolution so creation and resolution can't diverge.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "b_dir",
            r#"{ "name": "dup", "extensions": ["bbb"], "profile": {} }"#,
            true,
        );
        write_grammar(
            tmp.path(),
            "a_dir",
            r#"{ "name": "dup", "extensions": ["aaa"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        // "dup" registers exactly once. The first grammar (a_dir, extension
        // "aaa") wins; the duplicate's "bbb" extension is never registered
        // because the whole duplicate entry is skipped.
        assert!(reg.by_name.contains_key("dup"));
        assert!(reg.by_extension.contains_key("aaa"));
        assert!(!reg.by_extension.contains_key("bbb"));
    }

    #[test]
    fn discover_in_returns_none_for_unreadable_directory() {
        // A path that isn't a readable directory (here, one that doesn't exist)
        // yields None so `refresh` can distinguish "couldn't scan" from "empty"
        // and avoid blanking a working registry.
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(LanguageRegistry::discover_in(&missing).is_none());
    }

    #[test]
    fn is_static_language_name_is_case_insensitive() {
        assert!(is_static_language_name("Rust"));
        assert!(is_static_language_name("PYTHON"));
        assert!(is_static_language_name("ts"));
        assert!(!is_static_language_name("lua"));
        assert!(!is_static_language_name("ruby"));
    }
}
