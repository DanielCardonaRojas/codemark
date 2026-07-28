use codemark_core::error::{Error, Result};
use crate::cli::output::{self, OutputMode};
use crate::cli::{GrammarsCommand, GrammarsAddArgs};


pub async fn handle_grammars(cli: &crate::cli::Cli, mode: &OutputMode, cmd: &GrammarsCommand) -> Result<()> {
    match cmd {
        GrammarsCommand::Add(args) => handle_add(cli, mode, args).await,
        GrammarsCommand::List => handle_list(cli, mode).await,
        GrammarsCommand::Validate => handle_validate(cli, mode).await,
    }
}

async fn handle_add(_cli: &crate::cli::Cli, mode: &OutputMode, args: &GrammarsAddArgs) -> Result<()> {
    if !args.wasm_file.exists() {
        return Err(Error::Input(format!("WASM file not found: {}", args.wasm_file.display())).into());
    }

    let Some(grammar_dir) = codemark_core::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()).into());
    };

    let lang_dir = grammar_dir.join(&args.name);
    std::fs::create_dir_all(&lang_dir).map_err(|e| {
        Error::Operation(format!("Failed to create directory {}: {}", lang_dir.display(), e))
    })?;

    let target_wasm = lang_dir.join("grammar.wasm");
    std::fs::copy(&args.wasm_file, &target_wasm).map_err(|e| {
        Error::Operation(format!("Failed to copy WASM file: {}", e))
    })?;

    let extensions: Vec<String> = args.extensions.split(',').map(|s| s.trim().to_string()).collect();
    let manifest_json = serde_json::json!({
        "name": args.name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": {}
    });

    let manifest_path = lang_dir.join("manifest.json");
    let manifest_str = serde_json::to_string_pretty(&manifest_json).unwrap();
    std::fs::write(&manifest_path, manifest_str).map_err(|e| {
        Error::Operation(format!("Failed to write manifest.json: {}", e))
    })?;

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": args.name,
            "directory": lang_dir,
        })).unwrap();
    } else {
        println!("Successfully added grammar for '{}' to {}", args.name, lang_dir.display());
        println!("Codemark will now automatically discover and use this grammar for the following extensions: {}", args.extensions);
    }

    Ok(())
}

async fn handle_list(_cli: &crate::cli::Cli, mode: &OutputMode) -> Result<()> {
    let dynamic_langs = codemark_core::parser::registry::LanguageRegistry::dynamic_languages();
    
    if matches!(mode, OutputMode::Json) {
        let mut out = Vec::new();
        for lang in dynamic_langs {
            if let codemark_core::parser::languages::Language::Dynamic(dl) = lang {
                out.push(serde_json::json!({
                    "name": dl.name,
                    "extensions": dl.extensions,
                    "wasm_path": dl.wasm_path,
                }));
            }
        }
        output::write_json_success(&out).unwrap();
        return Ok(());
    }

    if dynamic_langs.is_empty() {
        println!("No dynamic WASM grammars installed.");
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .set_header(vec!["Name", "Extensions", "WASM Path"]);

    for lang in dynamic_langs {
        if let codemark_core::parser::languages::Language::Dynamic(dl) = lang {
            table.add_row(vec![
                dl.name.clone(),
                dl.extensions.join(", "),
                dl.wasm_path.to_string_lossy().to_string(),
            ]);
        }
    }

    println!("{table}");
    Ok(())
}

async fn handle_validate(_cli: &crate::cli::Cli, mode: &OutputMode) -> Result<()> {
    let Some(grammar_dir) = codemark_core::config::global_grammars_dir() else {
        return Err(Error::Operation("Could not determine global grammars directory".to_string()).into());
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
                            issues.push(format!("{}: manifest.json missing 'name' field", entry.path().display()));
                        }
                        if val.get("extensions").is_none() {
                            issues.push(format!("{}: manifest.json missing 'extensions' field", entry.path().display()));
                        }
                    },
                    Err(e) => issues.push(format!("{}: invalid JSON: {}", manifest_path.display(), e)),
                }
            }
        }
    }

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "valid": issues.is_empty(),
            "issues": issues,
        })).unwrap();
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
