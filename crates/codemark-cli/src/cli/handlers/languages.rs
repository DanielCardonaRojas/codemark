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
            return Err(Error::Input(format!(
                "Invalid grammar name '{}': must be a single path component with no separators or '..'",
                args.name
            )));
        }
    };

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

    // The target files themselves may be pre-existing symlinks pointing outside
    // the cache; `copy`/`write` follow them and would overwrite the external
    // target. Reject any symlinked child before writing.
    let target_wasm = lang_dir.join("grammar.wasm");
    reject_symlink(&target_wasm, &args.name)?;
    std::fs::copy(&args.wasm_file, &target_wasm)
        .map_err(|e| Error::Operation(format!("Failed to copy WASM file: {}", e)))?;

    let extensions: Vec<String> =
        args.extensions.split(',').map(|s| s.trim().to_string()).collect();
    let manifest_json = serde_json::json!({
        "name": args.name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": {}
    });

    let manifest_path = lang_dir.join("manifest.json");
    reject_symlink(&manifest_path, &args.name)?;
    let manifest_str = serde_json::to_string_pretty(&manifest_json).unwrap();
    std::fs::write(&manifest_path, manifest_str)
        .map_err(|e| Error::Operation(format!("Failed to write manifest.json: {}", e)))?;

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": args.name,
            "directory": lang_dir,
        }))
        .unwrap();
    } else {
        println!("Successfully added grammar for '{}' to {}", args.name, lang_dir.display());
        println!(
            "Codemark will now automatically discover and use this grammar for the following extensions: {}",
            args.extensions
        );
    }

    Ok(())
}

/// Reject `path` if it already exists as a symlink, so a subsequent
/// symlink-following write (`create_dir_all`/`copy`/`write`) can't be redirected
/// to overwrite a target outside the grammar cache.
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
        output::write_json_success(&out).unwrap();
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
                        if val.get("name").is_none() {
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
        }))
        .unwrap();
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
