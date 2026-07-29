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

        // Recover any install whose swap was interrupted mid-replace by
        // `codemark languages add`, which leaves a `.bak-<name>` when the process
        // died after moving the old install aside but before the new one landed.
        recover_interrupted_installs(grammar_dir);

        let entries = std::fs::read_dir(grammar_dir).ok()?;

        // Sort entries by path so discovery is deterministic across processes:
        // filesystem iteration order is unspecified, so without this an
        // extension claimed by two grammars could resolve differently on
        // different runs. On conflict the first grammar (by sorted path) wins.
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            // Skip dot-prefixed entries (e.g. `.staging-*` written mid-install by
            // `codemark languages add`): they're transient and not grammars.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

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
                    // could select different grammars. Names are keyed
                    // case-insensitively (like extensions) so `Lua` and `lua`
                    // are treated as the same language rather than bypassing
                    // dedup or missing a `--lang lua` lookup.
                    let name_key = manifest.name.to_lowercase();
                    if let Some(existing) = registry.by_name.get(&name_key) {
                        eprintln!(
                            "codemark: language name '{}' already registered by grammar '{}' — ignoring duplicate",
                            manifest.name,
                            existing.name()
                        );
                        continue;
                    }
                    registry.by_name.insert(name_key, lang.clone());

                    for ext in &manifest.extensions {
                        let key = ext.to_lowercase();
                        // A built-in owns this extension, so `from_extension`
                        // would resolve to the built-in and never reach this
                        // grammar. Skip it rather than register a dead mapping.
                        if is_static_extension(&key) {
                            eprintln!(
                                "codemark: extension '.{key}' is owned by a built-in language — ignoring it for grammar '{}'",
                                manifest.name
                            );
                            continue;
                        }
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
        let Some(mut fresh) = Self::discover_in(&dir) else {
            eprintln!(
                "codemark: could not re-scan grammar cache {} — keeping current registry",
                dir.display()
            );
            return;
        };

        // A grammar can be dropped by an otherwise-successful scan if its manifest
        // is transiently absent, unreadable, malformed, or mid-rewrite. Don't let
        // such a transient state evict a working grammar: carry forward any
        // currently-registered grammar the fresh scan lost whose directory still
        // exists on disk. A grammar intentionally removed (its directory gone) is
        // not carried forward, so real removals still take effect.
        {
            let current = GLOBAL_REGISTRY.read().expect("grammar registry lock poisoned");
            fresh.carry_forward_transiently_missing(&current);
        }

        *GLOBAL_REGISTRY.write().expect("grammar registry lock poisoned") = fresh;
    }

    /// Re-add grammars from `previous` that this (freshly scanned) registry is
    /// missing but whose on-disk grammar directory still exists — i.e. the fresh
    /// scan lost them to a transient FS state rather than an intentional removal.
    fn carry_forward_transiently_missing(&mut self, previous: &Self) {
        for (name_key, lang) in &previous.by_name {
            if self.by_name.contains_key(name_key) {
                continue;
            }
            let Language::Dynamic(dl) = lang else { continue };
            // `wasm_path` is `<cache>/<grammar-dir>/grammar.wasm`; the grammar is
            // only "transiently" missing if that directory is still present.
            let dir_present = dl.wasm_path.parent().is_some_and(|d| d.exists());
            if !dir_present {
                continue;
            }
            eprintln!(
                "codemark: keeping grammar '{}' across refresh — its manifest was momentarily unreadable",
                dl.name
            );
            self.by_name.insert(name_key.clone(), lang.clone());
            for ext in &dl.extensions {
                let key = ext.to_lowercase();
                // Preserve resolution rules: don't shadow a built-in or a grammar
                // that already won this extension in the fresh scan.
                if is_static_extension(&key) || self.by_extension.contains_key(&key) {
                    continue;
                }
                self.by_extension.insert(key, lang.clone());
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

    /// Look up a dynamic language by name (case-insensitive).
    pub fn from_name(name: &str) -> Option<Language> {
        GLOBAL_REGISTRY
            .read()
            .expect("grammar registry lock poisoned")
            .by_name
            .get(&name.to_lowercase())
            .cloned()
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

/// Restore installs left half-swapped by an interrupted `codemark languages add`.
///
/// The installer replaces `<cache>/<name>` by renaming it to `<cache>/.bak-<name>`
/// and then renaming the new dir into place. If the process dies between those
/// two renames, `<name>` is momentarily absent while `.bak-<name>` holds the
/// previous (complete) install. Here we move the backup back so the grammar is
/// discoverable again; a `.bak-<name>` whose `<name>` already exists is a
/// leftover from a *completed* swap and is just removed.
///
/// A grammar with a live `.lock-<name>` (an install currently mid-swap) is left
/// untouched: that same `<name>` absent + `.bak-<name>` present state is the
/// *normal* transient of an active swap, not a crash, and recovering it would
/// fight the installer and make a valid `languages add` fail.
fn recover_interrupted_installs(grammar_dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(grammar_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_string_lossy().strip_prefix(".bak-").map(str::to_string)
        else {
            continue;
        };

        // An install is mid-swap for this name — its transient state isn't ours
        // to recover. (The installer reaps its own stale lock, so a dead process
        // won't wedge recovery forever.)
        if grammar_dir.join(format!(".lock-{name}")).exists() {
            continue;
        }

        let target = grammar_dir.join(&name);
        if target.exists() {
            // The swap completed; this backup is just an uncleaned leftover.
            let _ = std::fs::remove_dir_all(entry.path());
        } else {
            // Swap was interrupted — restore the previous install.
            if std::fs::rename(entry.path(), &target).is_ok() {
                eprintln!("codemark: recovered grammar '{name}' from an interrupted install");
            }
        }
    }
}

/// Whether `name` matches a built-in static language (including aliases).
fn is_static_language_name(name: &str) -> bool {
    // Delegate to the built-in resolver so this can't drift from the actual
    // set of names/aliases (e.g. `rs`, `ts`) the static languages claim.
    Language::static_from_name(name).is_some()
}

/// Whether `ext` (without the dot) is claimed by a built-in language, so a
/// dynamic grammar registering it would be shadowed by the built-in in
/// [`Language::from_extension`] and never resolve.
fn is_static_extension(ext: &str) -> bool {
    Language::static_from_extension(ext).is_some()
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
    fn discover_skips_dot_prefixed_staging_dirs() {
        // A `.staging-*` dir written mid-install must not be scanned as a grammar.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            ".staging-lua-123",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.is_empty());
        assert!(reg.by_extension.is_empty());
    }

    #[test]
    fn discover_recovers_backup_when_target_missing() {
        // Simulate an install interrupted mid-swap: `.bak-lua` holds the previous
        // install and `lua` is absent. Discovery must restore it and register it.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            ".bak-lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        assert!(!tmp.path().join("lua").exists());

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(tmp.path().join("lua").join("manifest.json").exists());
        assert!(!tmp.path().join(".bak-lua").exists());
        assert_eq!(reg.by_name.get("lua").map(|l| l.name()), Some("lua"));
    }

    #[test]
    fn discover_leaves_backup_untouched_while_install_lock_is_held() {
        // `<name>` absent + `.bak-<name>` present is the normal transient of an
        // active swap; a live `.lock-<name>` means recovery must not interfere.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            ".bak-lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        std::fs::write(tmp.path().join(".lock-lua"), b"").unwrap();

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        // Backup left in place (installer will finish the swap); not restored.
        assert!(tmp.path().join(".bak-lua").exists());
        assert!(!tmp.path().join("lua").exists());
        assert!(reg.by_name.is_empty());
    }

    #[test]
    fn discover_removes_stale_backup_when_target_present() {
        // A `.bak-lua` left over from a *completed* swap (its `lua` exists) is
        // just cleaned up, not restored over the current install.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        write_grammar(
            tmp.path(),
            ".bak-lua",
            r#"{ "name": "lua", "extensions": ["old"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(!tmp.path().join(".bak-lua").exists());
        assert!(reg.by_extension.contains_key("lua"));
        assert!(!reg.by_extension.contains_key("old"));
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
    fn duplicate_name_is_case_insensitive() {
        // `Lua` and `lua` name the same language. The first by sorted path
        // ("a_dir" → "Lua") wins and the case-variant duplicate is skipped, so
        // its extension never registers and name lookup stays consistent.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "a_dir",
            r#"{ "name": "Lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        write_grammar(
            tmp.path(),
            "b_dir",
            r#"{ "name": "lua", "extensions": ["luau"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        // Keyed case-insensitively, so both names collapse to one entry, keeping
        // the original-case display name "Lua". The duplicate's extension is
        // dropped along with the skipped entry.
        assert_eq!(reg.by_name.len(), 1);
        assert_eq!(reg.by_name.get("lua").map(|l| l.name()), Some("Lua"));
        assert!(reg.by_extension.contains_key("lua"));
        assert!(!reg.by_extension.contains_key("luau"));
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
    fn carry_forward_keeps_grammar_whose_manifest_went_transiently_bad() {
        // A grammar registered previously, then a refresh where its manifest is
        // momentarily unreadable/malformed but the directory (and wasm) remain.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        let previous = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(previous.by_name.contains_key("lua"));

        // Corrupt the manifest in place (dir + grammar.wasm still present).
        std::fs::write(tmp.path().join("lua").join("manifest.json"), "{ not json").unwrap();
        let mut fresh = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(fresh.by_name.is_empty(), "fresh scan drops the corrupt grammar");

        fresh.carry_forward_transiently_missing(&previous);
        assert_eq!(fresh.by_name.get("lua").map(|l| l.name()), Some("lua"));
        assert!(fresh.by_extension.contains_key("lua"));
    }

    #[test]
    fn carry_forward_drops_grammar_whose_directory_was_removed() {
        // An intentional removal (directory gone) must NOT be carried forward.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "lua",
            r#"{ "name": "lua", "extensions": ["lua"], "profile": {} }"#,
            true,
        );
        let previous = LanguageRegistry::discover_in(tmp.path()).unwrap();

        std::fs::remove_dir_all(tmp.path().join("lua")).unwrap();
        let mut fresh = LanguageRegistry::discover_in(tmp.path()).unwrap();
        fresh.carry_forward_transiently_missing(&previous);
        assert!(fresh.by_name.is_empty(), "removed grammar stays removed");
    }

    #[test]
    fn is_static_language_name_is_case_insensitive() {
        assert!(is_static_language_name("Rust"));
        assert!(is_static_language_name("PYTHON"));
        assert!(is_static_language_name("ts"));
        assert!(!is_static_language_name("lua"));
        assert!(!is_static_language_name("ruby"));
    }

    #[test]
    fn is_static_language_name_covers_builtin_aliases() {
        // Aliases resolve to a built-in, so a dynamic grammar can't claim them.
        // `rs` regressed previously because the check was a hand-maintained list.
        assert!(is_static_language_name("rs"));
        assert!(is_static_language_name("py"));
        assert!(is_static_language_name("cs"));
        assert!(is_static_language_name("tsx"));
    }

    #[test]
    fn discover_skips_name_colliding_with_builtin_alias() {
        // A manifest named `rs` would be shadowed by built-in Rust at resolution
        // time, so discovery must reject it rather than list a dead grammar.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "rs_grammar",
            r#"{ "name": "rs", "extensions": ["myrs"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.is_empty());
        assert!(reg.by_extension.is_empty());
    }

    #[test]
    fn discover_skips_extension_owned_by_builtin() {
        // A dynamic grammar claiming `.rs` would never resolve (built-in Rust
        // wins in `from_extension`), so that extension is dropped — but the
        // grammar's other, free extensions still register.
        let tmp = tempfile::tempdir().unwrap();
        write_grammar(
            tmp.path(),
            "rusty",
            r#"{ "name": "rusty", "extensions": ["rs", "rusty"], "profile": {} }"#,
            true,
        );

        let reg = LanguageRegistry::discover_in(tmp.path()).unwrap();
        assert!(reg.by_name.contains_key("rusty"));
        assert!(!reg.by_extension.contains_key("rs"));
        assert!(reg.by_extension.contains_key("rusty"));
    }
}
