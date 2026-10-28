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
        Some(LanguagesCommand::Registry(r)) => match &r.command {
            crate::cli::RegistryCommand::List(a) => handle_registry_list(cli, mode, a).await,
        },
        #[cfg(feature = "profile-authoring")]
        Some(LanguagesCommand::Profile(crate::cli::ProfileCommand::Init(a))) => {
            handle_profile_init(mode, a).await
        }
        Some(LanguagesCommand::List(_)) | None => handle_list(cli, mode).await,
    }
}

/// Propose a profile by parsing sample files with the given grammar.
///
/// Deliberately writes nothing to the grammar cache: the proposal is a draft for
/// a human to review (that's what the evidence table is for), and installing is
/// a separate explicit step.
#[cfg(feature = "profile-authoring")]
async fn handle_profile_init(mode: &OutputMode, args: &crate::cli::ProfileInitArgs) -> Result<()> {
    use codemark_core::grammar::profile_gen;
    use codemark_core::parser::languages::{DynamicLanguage, Language};

    let name = args.name.clone().unwrap_or_else(|| {
        args.wasm_file
            .file_stem()
            .map(|s| s.to_string_lossy().trim_start_matches("tree-sitter-").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    });

    let mut samples = Vec::new();
    for path in &args.sample {
        let source = std::fs::read_to_string(path)
            .map_err(|e| Error::Input(format!("could not read sample {}: {e}", path.display())))?;
        samples.push((path.display().to_string(), source));
    }

    // Load the grammar through the same path the runtime uses, so an
    // ABI-incompatible .wasm fails here rather than producing a silent
    // half-proposal.
    let language = Language::Dynamic(std::sync::Arc::new(DynamicLanguage {
        name: name.clone(),
        extensions: Vec::new(),
        wasm_path: args.wasm_file.clone(),
        profile: Default::default(),
    }));

    let proposal = profile_gen::propose_profile(&language, &samples)?;

    let extensions: Vec<String> = args
        .extensions
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let manifest = serde_json::json!({
        "name": name,
        "version": "0.1.0",
        "extensions": extensions,
        "profile": proposal.profile,
    });

    if let Some(out) = &args.out {
        let text = serde_json::to_string_pretty(&manifest).unwrap();
        std::fs::write(out, format!("{text}\n"))
            .map_err(|e| Error::Operation(format!("could not write {}: {e}", out.display())))?;
    }

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "manifest": manifest,
            "evidence": proposal.evidence,
            "samples_with_errors": proposal.samples_with_errors,
            "written_to": args.out,
        }))?;
        return Ok(());
    }

    if !proposal.samples_with_errors.is_empty() {
        eprintln!(
            "Warning: these samples did not parse cleanly, so the proposal may be incomplete:"
        );
        for s in &proposal.samples_with_errors {
            eprintln!("  - {s}");
        }
        eprintln!("Check that they are source files for this grammar.\n");
    }

    let mut table = comfy_table::Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .set_header(vec!["Node kind", "Seen", "Landmark", "Name field", "Why"]);
    for e in &proposal.evidence {
        table.add_row(vec![
            e.kind.clone(),
            e.occurrences.to_string(),
            if e.landmark { "yes" } else { "-" }.to_string(),
            e.name_field.clone().unwrap_or_else(|| "-".to_string()),
            e.reason.clone(),
        ]);
    }
    println!("{table}");
    println!();

    match &args.out {
        Some(out) => println!("Wrote the proposed manifest to {}", out.display()),
        None => println!("{}", serde_json::to_string_pretty(&manifest).unwrap()),
    }
    // `languages add` on a local file always writes an empty profile — it reads
    // only the .wasm. So saying "install it with add" would quietly throw the
    // proposal away. Spell out the copy step instead.
    let manifest_hint = match &args.out {
        Some(out) => out.display().to_string(),
        None => "the profile above".to_string(),
    };
    println!(
        "\nReview the proposal, then install the grammar and apply the profile:\n  \
         codemark languages add {} --name {name} --extensions <exts>\n  \
         # `add` writes an empty profile, so copy {manifest_hint} over the\n  \
         # `profile` field of the installed manifest (its path is printed above), then:\n  \
         codemark languages validate",
        args.wasm_file.display()
    );

    Ok(())
}

async fn handle_add(
    cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesAddArgs,
) -> Result<()> {
    // A local .wasm carries no metadata, so name + extensions come from the
    // caller — or, with --from-registry, from the curated entry. Either way the
    // source layer reads the bytes and the shared pipeline validates, stages,
    // and swaps them in.
    let overrides =
        InstallOverrides { name: args.name.clone(), extensions: args.extensions.clone() };

    // Pass the PathBuf straight through (not a lossy string) so a non-UTF-8 path
    // isn't mangled.
    let outcome = match &args.from_registry {
        Some(language) => {
            let config = crate::cli::handlers::load_config(cli);
            let client = codemark_core::grammar::registry_source::RegistryClient::from_config(
                &config.grammars,
            )?;
            grammar::install_from_registry(
                &args.wasm_file,
                language,
                overrides,
                &client,
                args.refresh,
            )
            .await?
        }
        None => grammar::install_from_path(&args.wasm_file, overrides).await?,
    };
    print_install_outcome(mode, &outcome, args.from_registry.is_some())
}

async fn handle_registry_list(
    cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &crate::cli::RegistryListArgs,
) -> Result<()> {
    use codemark_core::grammar::registry_source::RegistryClient;

    // Layered config, falling back to defaults outside a repo — browsing the
    // registry must not require `codemark init`.
    let config = crate::cli::handlers::load_config(cli);
    let client = RegistryClient::from_config(&config.grammars)?;
    let index = client.fetch_index(args.refresh).await?;

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "registry": client.base_url(),
            "grammars": index.grammars,
        }))?;
        return Ok(());
    }

    if index.grammars.is_empty() {
        println!("The registry at {} lists no grammars.", client.base_url());
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table
        .load_preset(comfy_table::presets::UTF8_FULL)
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .set_header(vec!["Language", "Extensions", "Profile", "Built against"]);
    for e in &index.grammars {
        table.add_row(vec![
            e.name.clone(),
            e.extensions.join(", "),
            e.profile_version.clone().unwrap_or_else(|| "-".to_string()),
            e.grammar_rev.clone().unwrap_or_else(|| "-".to_string()),
        ]);
    }
    println!("{table}");
    println!("\nRegistry: {}", client.base_url());
    println!(
        "The registry ships profiles, not grammars — build the .wasm yourself, then:\n  codemark \
         languages add <grammar.wasm> --from-registry <language>"
    );
    Ok(())
}

/// Render a successful grammar install for the active [`OutputMode`]. Kept in the
/// CLI so `core::grammar` stays presentation-free.
fn print_install_outcome(
    mode: &OutputMode,
    outcome: &InstallOutcome,
    curated_profile: bool,
) -> Result<()> {
    let InstallOutcome { name, directory, extensions, runtime_enabled } = outcome;
    let ext_list = extensions.join(", ");

    if matches!(mode, OutputMode::Json) {
        output::write_json_success(&serde_json::json!({
            "message": "Grammar added successfully",
            "name": name,
            "directory": directory,
            "extensions": extensions,
            "runtime_enabled": runtime_enabled,
            "curated_profile": curated_profile,
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
        if curated_profile {
            println!(
                "Installed with a curated profile from the registry, so breadcrumbs and query \
                 summaries work out of the box."
            );
        } else {
            // `profile init` only exists on a `profile-authoring` build, so
            // recommending it unconditionally sends most users to an
            // unknown-command error.
            let generate_hint = if cfg!(feature = "profile-authoring") {
                "Otherwise generate one from sample files rather than hand-writing it:\n  \
                 codemark languages profile init <grammar.wasm> --sample <file>\n"
                    .to_string()
            } else {
                "Otherwise write one by hand (see the adding-wasm-grammars guide), or build \
                 codemark with `--features profile-authoring` to generate one from sample \
                 files.\n"
                    .to_string()
            };
            println!(
                "The grammar has an empty profile — parsing works now, but breadcrumbs and query \
                 summaries need one. Check whether the registry already has one:\n  codemark \
                 languages registry list\n{generate_hint}Put the result in {}/manifest.json and \
                 run `codemark languages validate`.",
                directory.display()
            );
        }
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

    // `issues` are blocking (a grammar that can't load, or a profile entry that
    // can never match); `warnings` are advisory and deliberately don't flip
    // `valid`, so a profile that merely lacks containers still reports valid.
    let mut issues = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

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
                    if let (Some(name), true) = (name, wasm_path.exists()) {
                        match grammar::load_dynamic_grammar(name, &wasm_path, &val) {
                            // Loading proves the .wasm is usable but says nothing
                            // about the profile: a node kind borrowed from another
                            // language parses as a string and then never matches.
                            // Lint it against the grammar we just loaded.
                            Ok(loaded) => {
                                use codemark_core::parser::profile::{LintSeverity, lint};
                                let profile =
                                    val.get("profile").cloned().unwrap_or(serde_json::json!({}));
                                for finding in lint(&profile, &loaded) {
                                    let line = format!("{}: {finding}", entry.path().display());
                                    // A warning (no containers, an unknown key) is
                                    // advisory — the grammar still works — so it
                                    // must not flip `valid` to false.
                                    match finding.severity {
                                        LintSeverity::Error => issues.push(line),
                                        LintSeverity::Warning => warnings.push(line),
                                    }
                                }
                            }
                            Err(e) => issues.push(format!(
                                "{}: grammar.wasm failed to load: {}",
                                entry.path().display(),
                                e
                            )),
                        }
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
            "warnings": warnings,
        }))?;
        return Ok(());
    }

    if issues.is_empty() && warnings.is_empty() {
        println!("All installed grammars are valid.");
    } else {
        if !issues.is_empty() {
            println!("Found issues with installed grammars:");
            for issue in &issues {
                println!("  - {}", issue);
            }
        }
        if !warnings.is_empty() {
            if !issues.is_empty() {
                println!();
            }
            println!("Warnings (these grammars still work):");
            for warning in &warnings {
                println!("  - {}", warning);
            }
        }
    }

    Ok(())
}
