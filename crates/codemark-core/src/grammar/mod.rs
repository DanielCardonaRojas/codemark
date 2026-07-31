//! Installing and validating dynamic WASM grammars.
//!
//! This is the *writer* half of the grammar lifecycle: it takes in-memory
//! `grammar.wasm` bytes and commits them into the grammar cache through a
//! hardened stage-then-swap so an install is all-or-nothing and can't corrupt a
//! previously-working grammar. The *reader* half — discovery and recovery of an
//! interrupted swap — lives in [`crate::parser::registry`]; both agree on the
//! on-disk layout via [`protocol`].
//!
//! [`install_grammar`] is the source-agnostic seam: it takes a name, normalized
//! extensions, an optional profile, and the wasm bytes. Today those always come
//! from a local file (`codemark languages add`); the seam stays source-agnostic
//! so a future registry source could feed it too. Presentation (output
//! formatting) stays in the caller — this module returns a value, it does not
//! print.

pub mod protocol;
pub mod source;

use crate::error::{Error, Result};

/// Caller-supplied overrides that win over whatever a [`source::GrammarSource`]
/// resolves. Both optional — a local file requires them (it carries no metadata).
#[derive(Debug, Default)]
pub struct InstallOverrides {
    pub name: Option<String>,
    pub extensions: Option<String>,
}

/// Install a grammar from a local `.wasm` file on disk (`codemark languages
/// add`). Takes a `&Path` directly so a non-UTF-8 path survives intact.
///
/// This is the single install entry point the CLI drives. It resolves the file
/// through [`source::LocalFileSource`], applies `overrides`, and hands off to the
/// hardened [`install_grammar`]. (When a grammar *registry* source is added it
/// will grow a sibling entry point; the [`source::GrammarSource`] trait keeps the
/// door open.)
pub async fn install_from_path(
    path: &std::path::Path,
    overrides: InstallOverrides,
) -> Result<InstallOutcome> {
    let resolved = source::LocalFileSource.resolve_path(path)?;

    // Overrides win over the source's reported metadata.
    let name = overrides.name.or(resolved.name).ok_or_else(|| {
        Error::Input("could not determine the language name; pass --name explicitly".to_string())
    })?;
    let raw_extensions = overrides.extensions.or(resolved.raw_extensions).ok_or_else(|| {
        Error::Input(
            "could not determine file extensions; pass --extensions (they are required for a \
             local file)"
                .to_string(),
        )
    })?;

    // Reject built-in collisions before any file write.
    let extensions = validate_name_and_extensions(&name, &raw_extensions)?;

    let wasm_bytes = resolved.wasm.into_bytes().await?;
    install_grammar(&name, &extensions, resolved.profile, wasm_bytes)
}

/// The outcome of a successful [`install_grammar`], returned so the caller can
/// format it (human or JSON) without this module owning presentation.
#[derive(Debug)]
pub struct InstallOutcome {
    /// The grammar name it was installed under.
    pub name: String,
    /// The directory the grammar now lives in.
    pub directory: std::path::PathBuf,
    /// The normalized extensions it claims.
    pub extensions: Vec<String>,
    /// Whether this build can actually load the grammar at runtime (the `wasm`
    /// feature). When false the files are installed but inert.
    pub runtime_enabled: bool,
}

/// Install a grammar from in-memory `wasm_bytes` under `name`, writing a manifest
/// with the given normalized `extensions` and `profile`. Used by `add` (local
/// file) — and kept source-agnostic so a future registry source could reuse the
/// same validation and hardened stage-then-swap install.
///
/// `extensions` must already be normalized (trimmed, dot-stripped, lowercased,
/// non-empty). Use [`validate_name_and_extensions`] to produce them; it also
/// rejects names/extensions that collide with a built-in language, an invariant
/// that must run before any file is written.
///
/// `profile` is written into the manifest verbatim. `None` writes an empty
/// profile (`{}`) — today's behavior for local files and GitHub releases; a
/// future registry source can supply a curated profile here so breadcrumbs and
/// query summaries work without hand-authoring.
pub fn install_grammar(
    name: &str,
    extensions: &[String],
    profile: Option<serde_json::Value>,
    wasm_bytes: Vec<u8>,
) -> Result<InstallOutcome> {
    let Some(grammar_dir) = crate::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()));
    };

    // Constrain the language name to a single safe path component so an absolute
    // or `..`-laden name can't escape the grammar cache.
    let mut components = std::path::Path::new(name).components();
    let safe_name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(c)), None) => c,
        _ => {
            tracing::debug!(
                target: "codemark::languages",
                %name,
                "rejected grammar name — not a single normal path component"
            );
            return Err(Error::Input(format!(
                "Invalid grammar name '{name}': must be a single path component with no separators or '..'"
            )));
        }
    };
    tracing::debug!(target: "codemark::languages", %name, "accepted grammar name");

    let lang_dir = grammar_dir.join(safe_name);
    reject_symlink(&lang_dir, name)?;

    let manifest_str = build_manifest(name, extensions, profile)?;

    install_staged(&grammar_dir, safe_name, &lang_dir, &wasm_bytes, manifest_str.as_bytes(), name)?;

    Ok(InstallOutcome {
        name: name.to_string(),
        directory: lang_dir,
        extensions: extensions.to_vec(),
        runtime_enabled: cfg!(feature = "wasm"),
    })
}

/// Build the `manifest.json` text for an install. A `None` profile writes an
/// empty object (`{}`); `Some(..)` is written verbatim so a registry can ship a
/// curated profile. Extracted so the profile-threading can be tested without
/// touching the filesystem or the global grammar cache.
fn build_manifest(
    name: &str,
    extensions: &[String],
    profile: Option<serde_json::Value>,
) -> Result<String> {
    // Validate a supplied profile against the `Profile` schema *before* it's
    // written, so a source (e.g. a future registry) can't commit a manifest whose
    // profile the registry reader would reject — which would report a successful
    // install of an unusable grammar. `None` writes an empty object, which is a
    // valid (all-default) Profile.
    let profile = match profile {
        Some(value) => {
            serde_json::from_value::<crate::parser::profile::Profile>(value.clone()).map_err(
                |e| {
                    Error::Input(format!("grammar profile does not match the expected schema: {e}"))
                },
            )?;
            value
        }
        None => serde_json::json!({}),
    };
    let manifest_json = serde_json::json!({
        "name": name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": profile,
    });
    Ok(serde_json::to_string_pretty(&manifest_json).unwrap())
}

/// Validate a grammar `name` and raw comma-separated `raw_extensions`, returning
/// the normalized (trimmed, dot-stripped, lowercased, non-empty) extensions.
///
/// Runs before any file is written so a request that could never resolve — a
/// name shadowed by a built-in (incl. aliases like `rs`/`ts`), or extensions
/// that are all empty/dotted or owned by a built-in — is rejected instead of
/// truncating an existing same-named install.
pub fn validate_name_and_extensions(name: &str, raw_extensions: &str) -> Result<Vec<String>> {
    use crate::parser::languages::Language;

    if Language::static_from_name(name).is_some() {
        return Err(Error::Input(format!(
            "Invalid grammar name '{name}': it is (or aliases) a built-in language and would never resolve"
        )));
    }

    // Runtime lookup keys on non-empty, dotless, lowercase extensions, so a
    // `.lua` or empty token stored verbatim would never match (or would wrongly
    // claim extensionless files).
    let extensions: Vec<String> = raw_extensions
        .split(',')
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if extensions.is_empty() {
        return Err(Error::Input(
            "No valid extensions given: provide a comma-separated list like `lua,luau`".to_string(),
        ));
    }
    if let Some(builtin) = extensions.iter().find(|e| Language::static_from_extension(e).is_some())
    {
        return Err(Error::Input(format!(
            "Extension '.{builtin}' is owned by a built-in language and can't be claimed by a dynamic grammar"
        )));
    }

    Ok(extensions)
}

/// Per-grammar-name exclusive lock, held across the install swap so two
/// concurrent `codemark languages add` for the same name can't interleave and
/// clobber the shared deterministic backup. Released (lock file removed) on drop,
/// including on error or panic.
struct InstallLock(std::path::PathBuf);

impl InstallLock {
    fn acquire(
        grammar_dir: &std::path::Path,
        safe_name: &std::ffi::OsStr,
        name: &str,
    ) -> Result<Self> {
        let lock_path = protocol::lock_path(grammar_dir, &safe_name.to_string_lossy());
        // `create_new` is an atomic "create only if absent", so exactly one
        // process wins. Retry briefly for a concurrent installer to finish;
        // reap an orphaned lock older than the timeout so a killed process can't
        // wedge future installs.
        let start = std::time::Instant::now();
        let timeout = protocol::INSTALL_LOCK_TIMEOUT;
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(_) => return Ok(InstallLock(lock_path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Reap an orphaned lock, then retry immediately on success.
                    // If the reap *fails* (EPERM/EACCES, or a directory squatting
                    // at the lock path), fall through to the timeout+sleep path
                    // below rather than `continue`-ing — otherwise a lock that
                    // reads stale but can't be removed would spin the loop with no
                    // sleep and no timeout check (100% CPU hang).
                    if protocol::lock_is_stale(&lock_path)
                        && std::fs::remove_file(&lock_path).is_ok()
                    {
                        continue;
                    }
                    if start.elapsed() > timeout {
                        return Err(Error::Operation(format!(
                            "Another install of grammar '{name}' is in progress (lock {} held)",
                            lock_path.display()
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(Error::Operation(format!(
                        "Failed to acquire install lock {}: {e}",
                        lock_path.display()
                    )));
                }
            }
        }
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stage `grammar.wasm` + `manifest.json` in a temp dir under the cache, then
/// swap them into `lang_dir` so the install is all-or-nothing.
///
/// Writing the two files straight into `lang_dir` would leave a truncated wasm
/// with a stale/missing manifest if the second write fails or the process dies,
/// corrupting a previously-working grammar. Instead we write both into a staging
/// directory and only replace the target once both are on disk. On any failure
/// before the swap, the existing install is left untouched.
fn install_staged(
    grammar_dir: &std::path::Path,
    safe_name: &std::ffi::OsStr,
    lang_dir: &std::path::Path,
    wasm_bytes: &[u8],
    manifest_bytes: &[u8],
    name: &str,
) -> Result<()> {
    // Unique staging dir under the cache (hidden, pid-suffixed) so it's removed
    // on failure and can never collide with a real grammar name.
    let staging =
        protocol::staging_path(grammar_dir, &safe_name.to_string_lossy(), std::process::id());
    let _ = std::fs::remove_dir_all(&staging); // clear any stale leftover
    std::fs::create_dir_all(&staging).map_err(|e| {
        Error::Operation(format!("Failed to create staging dir {}: {}", staging.display(), e))
    })?;

    // Everything below must clean up `staging` on error, so run it in a closure.
    let result = (|| -> Result<()> {
        // Confirm the staging dir resolves inside the cache before writing.
        let canonical_stage = std::fs::canonicalize(&staging).map_err(|e| {
            Error::Operation(format!("Failed to resolve {}: {}", staging.display(), e))
        })?;
        let canonical_cache = std::fs::canonicalize(grammar_dir).map_err(|e| {
            Error::Operation(format!("Failed to resolve {}: {}", grammar_dir.display(), e))
        })?;
        if !canonical_stage.starts_with(&canonical_cache) {
            return Err(Error::Input(format!(
                "Refusing to install grammar '{name}': staging dir escapes the grammar cache"
            )));
        }

        // Fresh staging dir, so these targets can't be pre-existing symlinks;
        // write_no_follow still guards against a concurrent swap-in.
        let staged_wasm = staging.join("grammar.wasm");
        write_no_follow(&staged_wasm, wasm_bytes, name)?;
        write_no_follow(&staging.join("manifest.json"), manifest_bytes, name)?;

        // Validate the *staged* grammar — the exact bytes we're about to commit,
        // not the (mutable) source path. On wasm builds an unloadable grammar
        // fails here, before any swap, so it can never replace a working install.
        #[cfg(feature = "wasm")]
        {
            let manifest: serde_json::Value = serde_json::from_slice(manifest_bytes)
                .map_err(|e| Error::Operation(format!("staged manifest not JSON: {e}")))?;
            load_dynamic_grammar(name, &staged_wasm, &manifest).map_err(|e| {
                Error::Input(format!("'{}' is not a loadable WASM grammar: {e}", name))
            })?;
        }

        // Serialize the swap against another `languages add` replacing the same
        // grammar. The backup path is deterministic (so discovery can recover a
        // crash-interrupted swap), which means two concurrent same-name swaps
        // would otherwise share and clobber it. A per-name lock makes the
        // move-aside / rename-in / drop-backup sequence mutually exclusive.
        let _lock = InstallLock::acquire(grammar_dir, safe_name, name)?;

        // No existing install: a single rename is fully atomic — the target
        // either doesn't exist or exists complete, never partial.
        if !lang_dir.exists() {
            return std::fs::rename(&staging, lang_dir)
                .map_err(|e| Error::Operation(format!("Failed to install grammar '{name}': {e}")));
        }

        // Replacing an existing install needs two dir renames (POSIX can't
        // atomically replace a non-empty dir). Use a *deterministic* backup name
        // so that if the process dies between the renames — leaving `lang_dir`
        // momentarily absent — discovery can restore it (see `recover_backup`).
        let backup = protocol::backup_path(grammar_dir, &safe_name.to_string_lossy());
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(lang_dir, &backup)
            .map_err(|e| Error::Operation(format!("Failed to move existing install aside: {e}")))?;
        match std::fs::rename(&staging, lang_dir) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&backup);
                Ok(())
            }
            Err(e) => {
                // Restore the previous install so a failed swap doesn't lose it.
                let _ = std::fs::rename(&backup, lang_dir);
                Err(Error::Operation(format!("Failed to install grammar '{name}': {e}")))
            }
        }
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// Reject `path` if it already exists as a symlink, so a subsequent
/// symlink-following operation (`create_dir_all`) can't be redirected to a
/// target outside the grammar cache.
fn reject_symlink(path: &std::path::Path, name: &str) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::Input(format!(
            "Refusing to install grammar '{}': {} is a symlink",
            name,
            path.display()
        )));
    }
    Ok(())
}

/// Write `contents` to `path`, refusing to follow a symlink at the final path
/// component. Using `O_NOFOLLOW` at open time makes the symlink check and the
/// write a single atomic operation, so a concurrent process can't swap the
/// target for a symlink between a pre-check and the write (TOCTOU) to redirect
/// it outside the grammar cache.
#[cfg(unix)]
fn write_no_follow(path: &std::path::Path, contents: &[u8], name: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| {
            Error::Operation(format!(
                "Refusing to install grammar '{}': cannot open {} for writing: {}",
                name,
                path.display(),
                e
            ))
        })?;
    file.write_all(contents)
        .map_err(|e| Error::Operation(format!("Failed to write {}: {}", path.display(), e)))?;
    Ok(())
}

/// Windows equivalent of the Unix `O_NOFOLLOW` path. Opening with
/// `FILE_FLAG_OPEN_REPARSE_POINT` returns a handle to the reparse point itself
/// (symlink/junction) rather than following it, so checking the *opened
/// handle's* metadata — not the path — makes the symlink check and the write a
/// single atomic operation, closing the check-then-write (TOCTOU) window that a
/// plain `symlink_metadata` + `write` would leave open.
#[cfg(windows)]
fn write_no_follow(path: &std::path::Path, contents: &[u8], name: &str) -> Result<()> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;

    // FILE_FLAG_OPEN_REPARSE_POINT — open the link itself instead of its target.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| {
            Error::Operation(format!(
                "Refusing to install grammar '{}': cannot open {} for writing: {}",
                name,
                path.display(),
                e
            ))
        })?;

    // The handle came from the path above with no re-resolution, so this
    // metadata reflects exactly what we opened. Reject a reparse point rather
    // than writing through it to an external target.
    let is_reparse = file
        .metadata()
        .map(|m| m.file_type().is_symlink())
        .map_err(|e| Error::Operation(format!("Failed to stat {}: {}", path.display(), e)))?;
    if is_reparse {
        return Err(Error::Input(format!(
            "Refusing to install grammar '{}': {} is a symlink",
            name,
            path.display()
        )));
    }

    file.write_all(contents)
        .map_err(|e| Error::Operation(format!("Failed to write {}: {}", path.display(), e)))?;
    Ok(())
}

/// Fallback for any remaining non-Unix, non-Windows target: `O_NOFOLLOW` has no
/// portable equivalent, so use a symlink pre-check plus a plain write. Codemark's
/// grammar cache is a single-user local directory, so the residual window is
/// negligible on platforms without Unix symlinks or Windows reparse points.
#[cfg(not(any(unix, windows)))]
fn write_no_follow(path: &std::path::Path, contents: &[u8], name: &str) -> Result<()> {
    reject_symlink(path, name)?;
    std::fs::write(path, contents)
        .map_err(|e| Error::Operation(format!("Failed to write {}: {}", path.display(), e)))
}

/// Load a grammar through the runtime parser path so an empty or non-WASM
/// `grammar.wasm` that mere existence checks would accept is detected. The
/// `profile` is not needed to prove loadability, so it's left at default.
///
/// Used both to validate a *staged* grammar before committing it (so an
/// unloadable download can't replace a working install) and by
/// `codemark languages validate` to check already-installed grammars.
#[cfg(feature = "wasm")]
pub fn load_dynamic_grammar(
    name: &str,
    wasm_path: &std::path::Path,
    manifest: &serde_json::Value,
) -> Result<()> {
    use crate::parser::languages::{DynamicLanguage, Language, Parser};

    let extensions = manifest
        .get("extensions")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let dl = std::sync::Arc::new(DynamicLanguage {
        name: name.to_string(),
        extensions,
        wasm_path: wasm_path.to_path_buf(),
        profile: Default::default(),
    });

    // Constructing the parser reads the .wasm and calls `set_language`, which is
    // exactly what fails for an empty or malformed grammar.
    Parser::new(Language::Dynamic(dl)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_extensions_stripping_dots_empties_and_case() {
        let exts = validate_name_and_extensions("lua", " .Lua, luau ,, ").unwrap();
        assert_eq!(exts, vec!["lua".to_string(), "luau".to_string()]);
    }

    #[test]
    fn rejects_name_that_aliases_a_builtin() {
        // `rs` aliases built-in Rust — the grammar would never resolve, and
        // installing it could clobber an unrelated same-named install.
        assert!(validate_name_and_extensions("rs", "myrs").is_err());
        assert!(validate_name_and_extensions("typescript", "myts").is_err());
    }

    #[test]
    fn rejects_extensions_owned_by_a_builtin() {
        // `.rs` is owned by built-in Rust; a dynamic grammar can't claim it.
        assert!(validate_name_and_extensions("lua", "rs").is_err());
    }

    #[test]
    fn rejects_when_no_valid_extensions_remain() {
        // All tokens empty/dot-only → nothing usable.
        assert!(validate_name_and_extensions("lua", " , . , ").is_err());
        assert!(validate_name_and_extensions("lua", "").is_err());
    }

    #[test]
    fn accepts_a_normal_dynamic_grammar() {
        let exts = validate_name_and_extensions("lua", "lua").unwrap();
        assert_eq!(exts, vec!["lua".to_string()]);
    }

    #[test]
    fn manifest_writes_empty_profile_when_none() {
        let m: serde_json::Value =
            serde_json::from_str(&build_manifest("lua", &["lua".to_string()], None).unwrap())
                .unwrap();
        assert_eq!(m["profile"], serde_json::json!({}));
        assert_eq!(m["name"], "lua");
        assert_eq!(m["extensions"], serde_json::json!(["lua"]));
    }

    #[test]
    fn manifest_threads_a_supplied_profile_verbatim() {
        // Proves the registry path: a source's curated profile survives into the
        // manifest rather than being flattened to `{}`.
        let profile = serde_json::json!({ "landmark_kinds": ["function_item"] });
        let m: serde_json::Value = serde_json::from_str(
            &build_manifest("lua", &["lua".to_string()], Some(profile.clone())).unwrap(),
        )
        .unwrap();
        assert_eq!(m["profile"], profile);
    }

    #[test]
    fn manifest_rejects_a_profile_that_isnt_a_valid_profile() {
        // A non-object (or otherwise schema-invalid) profile is rejected before
        // the manifest is written, so the writer can't commit something the
        // registry reader would fail to deserialize.
        let not_an_object = serde_json::json!("landmark_kinds");
        assert!(build_manifest("lua", &["lua".to_string()], Some(not_an_object)).is_err());

        // Wrong field type: landmark_kinds must be a string array, not a number.
        let wrong_type = serde_json::json!({ "landmark_kinds": 3 });
        assert!(build_manifest("lua", &["lua".to_string()], Some(wrong_type)).is_err());
    }

    #[test]
    fn install_lock_is_mutually_exclusive_and_released_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let name = std::ffi::OsString::from("lua");

        let lock = InstallLock::acquire(tmp.path(), &name, "lua").unwrap();
        // A second acquire for the same name can't win while the first is held.
        // (Timeout is 10s; retry a couple times fast, expecting contention.)
        let held = tmp.path().join(".lock-lua");
        assert!(held.exists());
        assert!(
            std::fs::OpenOptions::new().write(true).create_new(true).open(&held).is_err(),
            "lock file must already exist while held"
        );

        drop(lock);
        // Released on drop, so the name is installable again.
        assert!(!held.exists());
        let lock2 = InstallLock::acquire(tmp.path(), &name, "lua").unwrap();
        drop(lock2);
    }
}
