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

    let lang_dir = grammar_dir.join(safe_name);

    // A lexically safe name still can't be trusted if `<cache>/<name>` already
    // exists as a symlink: `create_dir_all`/`copy`/`write` would follow it and
    // land the files outside the cache. Reject any pre-existing symlink at the
    // target so writes always stay within the grammar cache.
    reject_symlink(&lang_dir, &args.name)?;

    std::fs::create_dir_all(&lang_dir).map_err(|e| {
        Error::Operation(format!("Failed to create directory {}: {}", lang_dir.display(), e))
    })?;

    // Defence in depth: after creation, confirm the resolved directory is still
    // contained in the resolved grammar cache before writing into it.
    let canonical_dir = std::fs::canonicalize(&lang_dir).map_err(|e| {
        Error::Operation(format!("Failed to resolve {}: {}", lang_dir.display(), e))
    })?;
    let canonical_cache = std::fs::canonicalize(&grammar_dir).map_err(|e| {
        Error::Operation(format!("Failed to resolve {}: {}", grammar_dir.display(), e))
    })?;
    if !canonical_dir.starts_with(&canonical_cache) {
        return Err(Error::Input(format!(
            "Refusing to install grammar '{}': {} escapes the grammar cache",
            args.name,
            canonical_dir.display()
        )));
    }

    // The target files themselves may be (or be raced into becoming) symlinks
    // pointing outside the cache; a plain `copy`/`write` follows them and would
    // overwrite the external target. Open each destination with `O_NOFOLLOW` so
    // the write fails atomically if the final component is a symlink — closing
    // the check-then-write (TOCTOU) window rather than pre-checking.
    let target_wasm = lang_dir.join("grammar.wasm");
    let wasm_bytes = std::fs::read(&args.wasm_file)
        .map_err(|e| Error::Operation(format!("Failed to read WASM file: {}", e)))?;
    write_no_follow(&target_wasm, &wasm_bytes, &args.name)?;

    let extensions: Vec<String> =
        args.extensions.split(',').map(|s| s.trim().to_string()).collect();
    let manifest_json = serde_json::json!({
        "name": args.name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": {}
    });

    let manifest_path = lang_dir.join("manifest.json");
    let manifest_str = serde_json::to_string_pretty(&manifest_json).unwrap();
    write_no_follow(&manifest_path, manifest_str.as_bytes(), &args.name)?;

    // Loading WASM grammars at runtime requires the `wasm` feature (disabled by
    // default). Without it the grammar is installed on disk but can't actually
    // be used, so don't claim it will be — direct the user to a wasm build.
    let wasm_enabled = cfg!(feature = "wasm");

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": args.name,
            "directory": lang_dir,
            "runtime_enabled": wasm_enabled,
        }))?;
    } else {
        println!("Successfully added grammar for '{}' to {}", args.name, lang_dir.display());
        if wasm_enabled {
            println!(
                "Codemark will now automatically discover and use this grammar for the following extensions: {}",
                args.extensions
            );
        } else {
            println!(
                "Note: this build was compiled without the 'wasm' feature, so the grammar \
                 cannot be loaded at runtime. Rebuild codemark with --features wasm to use it \
                 for the following extensions: {}",
                args.extensions
            );
        }
    }

    Ok(())
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

/// Non-Unix fallback: `O_NOFOLLOW` isn't available, so fall back to a
/// symlink pre-check plus a plain write. (Codemark's grammar cache is a
/// single-user local directory; the residual TOCTOU window matters only on
/// Unix-style symlinks, which this platform lacks in the same form.)
#[cfg(not(unix))]
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
