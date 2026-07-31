use crate::cli::output::{self, OutputMode};
use crate::cli::{LanguagesAddArgs, LanguagesArgs, LanguagesCommand};
use codemark_core::error::{Error, Result};
use codemark_core::grammar::{self, InstallOutcome, InstallOverrides};

pub async fn handle_languages(
    cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesArgs,
) -> Result<()> {
    match &args.command {
        Some(LanguagesCommand::Add(add_args)) => handle_add(cli, mode, add_args).await,
        Some(LanguagesCommand::Validate(_)) => handle_validate(cli, mode).await,
        Some(LanguagesCommand::List(_)) | None => handle_list(cli, mode).await,
    }
}

async fn handle_add(
    _cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesAddArgs,
) -> Result<()> {
    // A local .wasm carries no metadata, so name + extensions are required
    // overrides; the source layer reads the bytes and the shared pipeline
    // validates, stages, and swaps them in.
    let overrides = InstallOverrides {
        name: Some(args.name.clone()),
        extensions: Some(args.extensions.clone()),
    };
    // Pass the PathBuf straight through (not a lossy string) so a non-UTF-8 path
    // isn't mangled.
    let outcome = grammar::install_from_path(&args.wasm_file, overrides).await?;
    print_install_outcome(mode, &outcome)
}

/// Render a successful grammar install for the active [`OutputMode`]. Kept in the
/// CLI so `core::grammar` stays presentation-free.
fn print_install_outcome(mode: &OutputMode, outcome: &InstallOutcome) -> Result<()> {
    let InstallOutcome { name, directory, extensions, runtime_enabled } = outcome;
    let ext_list = extensions.join(", ");

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": name,
            "directory": directory,
            "extensions": extensions,
            "runtime_enabled": runtime_enabled,
        }))?;
    } else {
        println!("Successfully added grammar for '{name}' to {}", directory.display());
        if *runtime_enabled {
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
        println!(
            "The grammar has an empty profile — parsing works now, but for better breadcrumbs \
             and query summaries fill in `profile` in {}/manifest.json (see the adding-wasm-grammars guide).",
            directory.display()
        );
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

async fn handle_validate(_cli: &crate::cli::Cli, mode: &OutputMode) -> Result<()> {
    let Some(grammar_dir) = codemark_core::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()));
    };

    let mut issues = Vec::new();

    // A missing cache dir just means no grammars installed; any other read error
    // (permissions, etc.) must be surfaced, not silently reported as "all valid".
    let entries = match std::fs::read_dir(&grammar_dir) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(Error::Operation(format!(
                "Failed to read grammar cache {}: {e}",
                grammar_dir.display()
            )));
        }
    };

    if let Some(entries) = entries {
        for entry in entries.flatten() {
            // Skip transient/temp dirs the installer writes (`.staging-`,
            // `.bak-`, `.lock-`); they aren't grammars.
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let manifest_path = entry.path().join("manifest.json");
            let wasm_path = entry.path().join("grammar.wasm");

            if !manifest_path.exists() {
                issues.push(format!("{}: missing manifest.json", entry.path().display()));
                continue;
            }

            if !wasm_path.exists() {
                issues.push(format!("{}: missing grammar.wasm", entry.path().display()));
            }

            let manifest_text = match std::fs::read_to_string(&manifest_path) {
                Ok(text) => text,
                Err(e) => {
                    // manifest.json exists (checked above) but couldn't be read —
                    // record it rather than silently dropping the grammar.
                    issues.push(format!(
                        "{}: manifest.json could not be read: {e}",
                        manifest_path.display()
                    ));
                    continue;
                }
            };
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
                        && let Err(e) = grammar::load_dynamic_grammar(name, &wasm_path, &val)
                    {
                        issues.push(format!(
                            "{}: grammar.wasm failed to load: {}",
                            entry.path().display(),
                            e
                        ));
                    }
                }
                Err(e) => issues.push(format!("{}: invalid JSON: {}", manifest_path.display(), e)),
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
