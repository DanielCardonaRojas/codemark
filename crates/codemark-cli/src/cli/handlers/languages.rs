use crate::cli::output::{self, OutputMode};
use crate::cli::{LanguagesAddArgs, LanguagesArgs, LanguagesCommand};
use codemark_core::error::{Error, Result};

pub async fn handle_languages(
    cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesArgs,
) -> Result<()> {
    match &args.command {
        Some(LanguagesCommand::Add(add_args)) => handle_add(cli, mode, add_args).await,
        Some(LanguagesCommand::Validate) => handle_validate(cli, mode).await,
        Some(LanguagesCommand::List) | None => handle_list(cli, mode).await,
    }
}

async fn handle_add(
    _cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesAddArgs,
) -> Result<()> {
    if !args.wasm_file.exists() {
        return Err(Error::Input(format!("WASM file not found: {}", args.wasm_file.display())));
    }

    let Some(grammar_dir) = codemark_core::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()));
    };

    // Constrain the language name to a single safe path component so an
    // absolute or `..`-laden `--name` can't escape the grammar cache and
    // clobber files elsewhere on disk.
    let mut components = std::path::Path::new(&args.name).components();
    let safe_name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(c)), None) => c,
        _ => {
            tracing::debug!(
                target: "codemark::languages",
                name = %args.name,
                "rejected grammar name — not a single normal path component"
            );
            return Err(Error::Input(format!(
                "Invalid grammar name '{}': must be a single path component with no separators or '..'",
                args.name
            )));
        }
    };
    tracing::debug!(
        target: "codemark::languages",
        name = %args.name,
        "accepted grammar name"
    );

    // Validate the name and extensions before any file is written, so a bad
    // request can't truncate a working install (see `validate_name_and_extensions`).
    let extensions = validate_name_and_extensions(&args.name, &args.extensions)?;

    let lang_dir = grammar_dir.join(safe_name);

    // A lexically safe name still can't be trusted if `<cache>/<name>` already
    // exists as a symlink: writes would follow it and land outside the cache.
    reject_symlink(&lang_dir, &args.name)?;

    let manifest_json = serde_json::json!({
        "name": args.name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": {}
    });
    let manifest_str = serde_json::to_string_pretty(&manifest_json).unwrap();

    // Read the source bytes exactly once. Validation and the committed install
    // both use *these* bytes, so a concurrent edit of the source path can't make
    // us validate one grammar and install a different one (TOCTOU).
    let wasm_bytes = std::fs::read(&args.wasm_file)
        .map_err(|e| Error::Operation(format!("Failed to read WASM file: {}", e)))?;

    // Stage the full install in a temp dir, validate the staged grammar loads,
    // then swap it into place. Writing straight into `lang_dir` would expose a
    // truncated wasm before the manifest is committed, so an interrupted write
    // could corrupt a working install; staging keeps the old install intact
    // until a complete, validated new one is ready.
    install_staged(
        &grammar_dir,
        safe_name,
        &lang_dir,
        &wasm_bytes,
        manifest_str.as_bytes(),
        &args.name,
    )?;

    // Loading WASM grammars at runtime requires the `wasm` feature (disabled by
    // default). Without it the grammar is installed on disk but can't actually
    // be used, so don't claim it will be — direct the user to a wasm build.
    let wasm_enabled = cfg!(feature = "wasm");

    let ext_list = extensions.join(", ");

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": args.name,
            "directory": lang_dir,
            "extensions": extensions,
            "runtime_enabled": wasm_enabled,
        }))?;
    } else {
        println!("Successfully added grammar for '{}' to {}", args.name, lang_dir.display());
        if wasm_enabled {
            println!(
                "Codemark will now automatically discover and use this grammar for the following extensions: {ext_list}"
            );
        } else {
            println!(
                "Note: this build was compiled without the 'wasm' feature, so the grammar \
                 cannot be loaded at runtime. Rebuild codemark with --features wasm to use it \
                 for the following extensions: {ext_list}"
            );
        }
    }

    Ok(())
}

/// Validate a grammar `name` and raw comma-separated `raw_extensions`, returning
/// the normalized (trimmed, dot-stripped, lowercased, non-empty) extensions.
///
/// Runs before any file is written so a request that could never resolve — a
/// name shadowed by a built-in (incl. aliases like `rs`/`ts`), or extensions
/// that are all empty/dotted or owned by a built-in — is rejected instead of
/// truncating an existing same-named install.
fn validate_name_and_extensions(name: &str, raw_extensions: &str) -> Result<Vec<String>> {
    use codemark_core::parser::languages::Language;

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
        let lock_path = grammar_dir.join(format!(".lock-{}", safe_name.to_string_lossy()));
        // `create_new` is an atomic "create only if absent", so exactly one
        // process wins. Retry briefly for a concurrent installer to finish;
        // reap an orphaned lock older than the timeout so a killed process can't
        // wedge future installs.
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(10);
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(_) => return Ok(InstallLock(lock_path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&lock_path)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().unwrap_or_default() > timeout)
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&lock_path);
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
    let staging = grammar_dir.join(format!(
        ".staging-{}-{}",
        safe_name.to_string_lossy(),
        std::process::id()
    ));
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
        let backup = backup_dir(grammar_dir, safe_name);
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

/// Deterministic backup path for an install being replaced: `<cache>/.bak-<name>`.
///
/// Built by appending to the *full* directory name (not `with_extension`, which
/// rewrites the last dot-segment and would collide `my.lang` with `my`). It is
/// **not** pid-suffixed on purpose: if the process dies mid-swap leaving
/// `<cache>/<name>` absent, the next run must be able to find and restore this
/// exact path (see `codemark_core::parser::registry` recovery on discovery).
fn backup_dir(grammar_dir: &std::path::Path, safe_name: &std::ffi::OsStr) -> std::path::PathBuf {
    grammar_dir.join(format!(".bak-{}", safe_name.to_string_lossy()))
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

async fn handle_list(_cli: &crate::cli::Cli, mode: &OutputMode) -> Result<()> {
    let languages = codemark_core::parser::languages::Language::all_supported();

    if matches!(mode, OutputMode::Json) {
        let mut out = Vec::new();
        for lang in languages {
            let is_dynamic = matches!(lang, codemark_core::parser::languages::Language::Dynamic(_));
            let path = if let codemark_core::parser::languages::Language::Dynamic(dl) = &lang {
                Some(dl.wasm_path.clone())
            } else {
                None
            };
            out.push(serde_json::json!({
                "name": lang.name(),
                "type": if is_dynamic { "dynamic" } else { "built-in" },
                "extensions": lang.file_extensions(),
                "wasm_path": path,
            }));
        }
        output::write_json_success(&out)?;
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .set_header(vec!["Language", "Type", "Extensions", "WASM Path"]);

    for lang in languages {
        let (type_str, path_str) =
            if let codemark_core::parser::languages::Language::Dynamic(dl) = &lang {
                ("dynamic (WASM)", dl.wasm_path.to_string_lossy().to_string())
            } else {
                ("built-in", "-".to_string())
            };

        table.add_row(vec![
            lang.name().to_string(),
            type_str.to_string(),
            lang.file_extensions().join(", "),
            path_str,
        ]);
    }

    println!("{table}");
    Ok(())
}

/// Load a grammar through the runtime parser path so `validate` can detect an
/// empty or non-WASM `grammar.wasm` that mere existence checks would accept.
/// The `profile` is not needed to prove loadability, so it's left at default.
#[cfg(feature = "wasm")]
fn load_dynamic_grammar(
    name: &str,
    wasm_path: &std::path::Path,
    manifest: &serde_json::Value,
) -> Result<()> {
    use codemark_core::parser::languages::{DynamicLanguage, Language, Parser};

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

async fn handle_validate(_cli: &crate::cli::Cli, mode: &OutputMode) -> Result<()> {
    let Some(grammar_dir) = codemark_core::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()));
    };

    let mut issues = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&grammar_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("manifest.json");
            let wasm_path = entry.path().join("grammar.wasm");

            if !manifest_path.exists() {
                issues.push(format!("{}: missing manifest.json", entry.path().display()));
                continue;
            }

            if !wasm_path.exists() {
                issues.push(format!("{}: missing grammar.wasm", entry.path().display()));
            }

            if let Ok(manifest_text) = std::fs::read_to_string(&manifest_path) {
                match serde_json::from_str::<serde_json::Value>(&manifest_text) {
                    Ok(val) => {
                        let name = val.get("name").and_then(|v| v.as_str());
                        if name.is_none() {
                            issues.push(format!(
                                "{}: manifest.json missing 'name' field",
                                entry.path().display()
                            ));
                        }
                        if val.get("extensions").is_none() {
                            issues.push(format!(
                                "{}: manifest.json missing 'extensions' field",
                                entry.path().display()
                            ));
                        }

                        // Existence checks can't tell an empty or non-WASM
                        // grammar.wasm from a usable one. Load it through the same
                        // parser path used at runtime so `validate` fails for
                        // grammars that can't actually be loaded. Only attempted
                        // on wasm builds; a non-wasm build can't load any dynamic
                        // grammar and would report a misleading feature error.
                        #[cfg(feature = "wasm")]
                        if let (Some(name), true) = (name, wasm_path.exists())
                            && let Err(e) = load_dynamic_grammar(name, &wasm_path, &val)
                        {
                            issues.push(format!(
                                "{}: grammar.wasm failed to load: {}",
                                entry.path().display(),
                                e
                            ));
                        }
                    }
                    Err(e) => {
                        issues.push(format!("{}: invalid JSON: {}", manifest_path.display(), e))
                    }
                }
            }
        }
    }

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "valid": issues.is_empty(),
            "issues": issues,
        }))?;
        return Ok(());
    }

    if issues.is_empty() {
        println!("All installed grammars are valid.");
    } else {
        println!("Found issues with installed grammars:");
        for issue in issues {
            println!("  - {}", issue);
        }
    }

    Ok(())
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
