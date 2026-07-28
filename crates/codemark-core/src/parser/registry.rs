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

use serde::Deserialize;

use crate::config::global_grammars_dir;
use crate::parser::languages::{DynamicLanguage, Language};
use crate::parser::profile::Profile;

/// The process-global registry, discovered once at first access.
static GLOBAL_REGISTRY: LazyLock<LanguageRegistry> = LazyLock::new(LanguageRegistry::discover);

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

        Self::discover_in(&grammar_dir)
    }

    /// Scan a specific grammar directory and build the registry.
    ///
    /// Split out from [`discover`](Self::discover) so the scan/skip logic can be
    /// tested against a fixture directory without touching the global cache.
    fn discover_in(grammar_dir: &std::path::Path) -> Self {
        let mut registry = Self { by_extension: HashMap::new(), by_name: HashMap::new() };

        let Ok(entries) = std::fs::read_dir(grammar_dir) else {
            return registry;
        };

        for entry in entries.flatten() {
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
                    registry.by_name.insert(manifest.name.clone(), lang.clone());

                    for ext in &manifest.extensions {
                        registry.by_extension.insert(ext.to_lowercase(), lang.clone());
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

        registry
    }

    /// Look up a dynamic language by file extension (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Language> {
        GLOBAL_REGISTRY.by_extension.get(&ext.to_lowercase()).cloned()
    }

    /// Look up a dynamic language by name.
    pub fn from_name(name: &str) -> Option<Language> {
        GLOBAL_REGISTRY.by_name.get(name).cloned()
    }

    /// List all registered dynamic language names.
    pub fn dynamic_language_names() -> Vec<&'static str> {
        GLOBAL_REGISTRY.by_name.keys().map(|s| s.as_str()).collect()
    }

    /// List all registered dynamic languages.
    pub fn dynamic_languages() -> Vec<Language> {
        GLOBAL_REGISTRY.by_name.values().cloned().collect()
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

        let reg = LanguageRegistry::discover_in(tmp.path());

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

        let reg = LanguageRegistry::discover_in(tmp.path());
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

        let reg = LanguageRegistry::discover_in(tmp.path());
        assert!(reg.by_name.is_empty());
    }

    #[test]
    fn discover_skips_invalid_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(tmp.path(), "broken", "{ not valid json", true);

        let reg = LanguageRegistry::discover_in(tmp.path());
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

        let reg = LanguageRegistry::discover_in(tmp.path());
        let lang = reg.by_name.get("lua").expect("lua registered");
        assert!(lang.profile().landmark_kinds.iter().any(|k| k == "local_function_declaration"));
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
