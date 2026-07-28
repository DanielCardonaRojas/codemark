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

    let lang_dir = grammar_dir.join(&args.name);
    std::fs::create_dir_all(&lang_dir).map_err(|e| {
        Error::Operation(format!("Failed to create directory {}: {}", lang_dir.display(), e))
    })?;

    let target_wasm = lang_dir.join("grammar.wasm");
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
