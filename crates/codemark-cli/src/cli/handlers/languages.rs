use crate::cli::output::{self, OutputMode};
use crate::cli::{LanguagesAddArgs, LanguagesArgs, LanguagesCommand, LanguagesInstallArgs};
use codemark_core::error::{Error, Result};
use codemark_core::grammar::{self, InstallOutcome};

/// Cap on a downloaded grammar.wasm (32 MiB). Real grammars are well under this;
/// a larger response means a wrong/corrupt asset.
const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

pub async fn handle_languages(
    cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesArgs,
) -> Result<()> {
    match &args.command {
        Some(LanguagesCommand::Add(add_args)) => handle_add(cli, mode, add_args).await,
        Some(LanguagesCommand::Install(install_args)) => {
            handle_install(cli, mode, install_args).await
        }
        Some(LanguagesCommand::Validate(_)) => handle_validate(cli, mode).await,
        Some(LanguagesCommand::List(_)) | None => handle_list(cli, mode).await,
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

    // Read the source bytes once — validation and the committed install use these
    // same bytes, so a concurrent edit of the source path can't make us validate
    // one grammar and install a different one (TOCTOU).
    let wasm_bytes = std::fs::read(&args.wasm_file)
        .map_err(|e| Error::Operation(format!("Failed to read WASM file: {}", e)))?;

    // Reject names/extensions that could never resolve (built-in collisions)
    // before any file is written, then hand the normalized extensions to the
    // shared install path.
    let extensions = grammar::validate_name_and_extensions(&args.name, &args.extensions)?;
    let outcome = grammar::install_grammar(&args.name, &extensions, wasm_bytes)?;
    print_install_outcome(mode, &outcome)
}

async fn handle_install(
    _cli: &crate::cli::Cli,
    mode: &OutputMode,
    args: &LanguagesInstallArgs,
) -> Result<()> {
    let (owner, repo) = parse_source(&args.source)?;
    let client = codemark_core::sync::build_sync_http_client()?;

    // 1. Read the repo's tree-sitter.json for the grammar name, file-types, and
    //    the Tree-sitter version (which governs WASM ABI compatibility). An
    //    explicit --name selects the matching entry in a multi-grammar repo.
    let meta = fetch_tree_sitter_json(&client, &owner, &repo, args.name.as_deref()).await?;

    // A multi-grammar repo (e.g. typescript + tsx) exposes several grammar
    // entries but often ships a single .wasm. Without --name we'd default to the
    // first entry's name/extensions, which may not correspond to the published
    // module — installing files that parse with the wrong grammar. Require the
    // caller to disambiguate rather than guessing.
    if meta.grammar_count > 1 && args.name.is_none() {
        return Err(Error::Input(format!(
            "{owner}/{repo} declares {} grammars in tree-sitter.json — pass --name to choose which \
             one to install (its name must match a grammar entry).",
            meta.grammar_count
        )));
    }

    // --name was given but matches no declared grammar. `meta.name`/`file_types`
    // then fell back to the first entry, so installing under the requested name
    // would label the real grammar (and its extensions) with an unrelated
    // identity — bookmarks and lookups for that name would silently miss. Refuse
    // and point at the declared name. (Installing a grammar under a custom local
    // name is deferred to a follow-up; keep the common case unambiguous.)
    if args.name.is_some() && meta.grammar_count > 0 && !meta.requested_name_matched {
        let requested = args.name.as_deref().unwrap_or_default();
        let declared = meta.name.as_deref().unwrap_or("<unknown>");
        return Err(Error::Input(format!(
            "--name '{requested}' doesn't match any grammar declared in {owner}/{repo}'s \
             tree-sitter.json (found '{declared}'). Install it under its declared name '{declared}'."
        )));
    }

    if !args.allow_version_mismatch
        && let Some(v) = &meta.version
        && !v.starts_with("0.25")
    {
        return Err(Error::Input(format!(
            "grammar '{owner}/{repo}' targets Tree-sitter {v}, but codemark loads 0.25 grammars — \
             the downloaded .wasm would likely fail to load. Rebuild it from source with the 0.25 \
             CLI, or pass --allow-version-mismatch to try anyway."
        )));
    }

    // 2. Resolve name/extensions (CLI overrides win over tree-sitter.json).
    let name = args.name.clone().or_else(|| meta.name.clone()).ok_or_else(|| {
        Error::Input("could not determine the language name; pass --name explicitly".to_string())
    })?;
    let raw_extensions = match &args.extensions {
        Some(e) => e.clone(),
        None if !meta.file_types.is_empty() => meta.file_types.join(","),
        None => {
            return Err(Error::Input(
                "could not determine file extensions from tree-sitter.json; pass --extensions"
                    .to_string(),
            ));
        }
    };
    let extensions = grammar::validate_name_and_extensions(&name, &raw_extensions)?;

    // 3. Find and download the .wasm release asset (matched to this grammar so a
    //    multi-.wasm release — or a lone asset in a multi-grammar repo — can't
    //    pair the manifest with the wrong module).
    let asset = find_wasm_asset(&client, &owner, &repo, &name, meta.grammar_count).await?;
    if !matches!(mode, OutputMode::Json) {
        println!("Downloading {} …", asset.name);
    }
    let wasm_bytes = download_capped(&client, &asset.url).await?;

    // 4. Reuse the hardened install path (validation + stage-then-swap). The
    //    manifest gets an empty profile — see the note printed on success.
    let outcome = grammar::install_grammar(&name, &extensions, wasm_bytes)?;
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

/// A Tree-sitter grammar repo's `tree-sitter.json` metadata (only the fields we
/// use). Unknown fields are ignored.
#[derive(Debug, Default)]
struct TreeSitterMeta {
    name: Option<String>,
    file_types: Vec<String>,
    version: Option<String>,
    /// How many grammar entries `tree-sitter.json` declared. >1 means a
    /// multi-grammar repo where `name`/`file_types` (taken from a single entry)
    /// can't be assumed to match the published `.wasm` unless the caller named
    /// the grammar explicitly.
    grammar_count: usize,
    /// Whether the caller's `--name` matched a declared grammar entry. `false`
    /// (with a name requested) means `name`/`file_types` fell back to the first
    /// entry and don't describe the requested grammar — the caller must then
    /// supply `--extensions` to take ownership of that identity explicitly.
    requested_name_matched: bool,
}

/// Parse a grammar source into `(owner, repo)`. Accepts `owner/repo`,
/// `github:owner/repo`, and `https://github.com/owner/repo[.git]`.
fn parse_source(source: &str) -> Result<(String, String)> {
    let s = source.trim();
    let s = s.strip_prefix("github:").unwrap_or(s);
    let s = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("git@github.com:"))
        .unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.trim_matches('/');

    let mut parts = s.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty() => {
            Ok((owner.to_string(), repo.to_string()))
        }
        _ => Err(Error::Input(format!(
            "invalid grammar source '{source}': expected `owner/repo`, `github:owner/repo`, or a github.com URL"
        ))),
    }
}

/// Fetch and parse `tree-sitter.json` from the repo's default branch.
///
/// `requested_name` (an explicit `--name`) selects the matching grammar entry
/// in a multi-grammar repo; see [`parse_tree_sitter_meta`].
async fn fetch_tree_sitter_json(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    requested_name: Option<&str>,
) -> Result<TreeSitterMeta> {
    // Probe the repo's *actual* default branch first, then the conventional
    // fallbacks. Without this, a repo whose default is neither `master` nor
    // `main` (e.g. `develop`) reports the file missing, and a repo that still
    // has an obsolete `master` alongside `main` would read stale metadata from
    // `master` before `main` is ever tried. The API lookup is best-effort — on
    // failure we fall back to the conventional branches so a token isn't
    // required and rate-limited/offline cases still work.
    let default_branch = fetch_default_branch(client, owner, repo).await;

    // Ordered, de-duplicated: the real default (if known) leads, then the
    // conventions. `main` before `master` so a stale `master` can't shadow it.
    let mut branches: Vec<String> = Vec::new();
    for b in default_branch.into_iter().chain(["main".to_string(), "master".to_string()]) {
        if !branches.contains(&b) {
            branches.push(b);
        }
    }

    for branch in &branches {
        let url =
            format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/tree-sitter.json");
        tracing::debug!(target: "codemark::http", %url, "fetching tree-sitter.json");
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Operation(format!("failed to fetch tree-sitter.json: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        if !resp.status().is_success() {
            return Err(Error::Operation(format!(
                "failed to fetch tree-sitter.json: HTTP {}",
                resp.status()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Operation(format!("tree-sitter.json is not valid JSON: {e}")))?;
        return Ok(parse_tree_sitter_meta(&json, requested_name));
    }
    Err(Error::Input(format!(
        "no tree-sitter.json found in {owner}/{repo} (looked on {}); \
         is this a Tree-sitter grammar repo?",
        branches.join(", ")
    )))
}

/// Best-effort lookup of a repo's default branch via the GitHub API. Returns
/// `None` on any failure (network, rate limit, non-2xx, malformed body) so the
/// caller falls back to the conventional branch names.
async fn fetch_default_branch(client: &reqwest::Client, owner: &str, repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    tracing::debug!(target: "codemark::http", %url, "fetching repo default branch");
    let resp =
        client.get(&url).header(reqwest::header::USER_AGENT, "codemark").send().await.ok()?;
    if !resp.status().is_success() {
        tracing::debug!(target: "codemark::http", status = %resp.status(), "default-branch lookup failed; using conventional fallbacks");
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("default_branch").and_then(|b| b.as_str()).map(str::to_string)
}

/// Extract the name, file-types, and Tree-sitter version from a parsed
/// `tree-sitter.json`.
///
/// When `requested_name` is given and matches a grammar entry, that entry's
/// name/file-types are used — so a multi-grammar repo (e.g. `typescript` + `tsx`)
/// installed with `--name tsx` doesn't silently pick up the first entry's
/// metadata. Otherwise the first entry is used, and `grammar_count` reports how
/// many entries existed so the caller can detect the ambiguous case.
fn parse_tree_sitter_meta(
    json: &serde_json::Value,
    requested_name: Option<&str>,
) -> TreeSitterMeta {
    let grammars = json.get("grammars").and_then(|g| g.as_array());
    let grammar_count = grammars.map(|a| a.len()).unwrap_or(0);

    // Prefer the entry matching an explicit --name; fall back to the first, and
    // record whether the requested name actually matched.
    let matched = grammars.and_then(|arr| {
        requested_name.and_then(|want| {
            arr.iter().find(|g| g.get("name").and_then(|n| n.as_str()).is_some_and(|n| n == want))
        })
    });
    let requested_name_matched = matched.is_some();
    let grammar = matched.or_else(|| grammars.and_then(|arr| arr.first()));

    let name = grammar.and_then(|g| g.get("name")).and_then(|n| n.as_str()).map(str::to_string);
    let file_types = grammar
        .and_then(|g| g.get("file-types"))
        .and_then(|f| f.as_array())
        .map(|arr| arr.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let version = json
        .get("metadata")
        .and_then(|m| m.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    TreeSitterMeta { name, file_types, version, grammar_count, requested_name_matched }
}

/// A release asset we can download.
struct ReleaseAsset {
    name: String,
    url: String,
}

/// Find the `.wasm` release asset for `grammar_name` on the repo's latest
/// release. `grammar_entry_count` is how many grammars `tree-sitter.json`
/// declared, which decides how strictly the asset must match the name.
///
/// A release may carry several `.wasm` modules (multi-grammar repos, or debug +
/// release builds). Grabbing an asset independently of the chosen grammar can
/// pair the manifest with a different module, so:
/// - **Multiple `.wasm` assets:** require an unambiguous match on the grammar
///   name (Tree-sitter's convention is `tree-sitter-<name>.wasm` / `<name>.wasm`).
/// - **One `.wasm` asset, single-grammar repo:** accept it — the sole asset *is*
///   the grammar, and repos name it arbitrarily (`parser.wasm`, `<repo>.wasm`),
///   so a name check would only cause false negatives.
/// - **One `.wasm` asset, multi-grammar repo:** the user chose one grammar via
///   `--name`, but a lone asset can't be assumed to be *that* grammar's module,
///   so require the asset name to match and otherwise refuse rather than install
///   a mismatched module.
///
/// Otherwise error with the candidate list rather than guessing.
async fn find_wasm_asset(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    grammar_name: &str,
    grammar_entry_count: usize,
) -> Result<ReleaseAsset> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    tracing::debug!(target: "codemark::http", %url, "fetching latest release");
    let resp = client
        .get(&url)
        // GitHub's API requires a User-Agent.
        .header(reqwest::header::USER_AGENT, "codemark")
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to query releases: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Operation(format!(
            "failed to query latest release of {owner}/{repo}: HTTP {}",
            resp.status()
        )));
    }
    let release: serde_json::Value =
        resp.json().await.map_err(|e| Error::Operation(format!("release JSON invalid: {e}")))?;

    let assets = release.get("assets").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    let wasm_assets: Vec<&serde_json::Value> = assets
        .iter()
        .filter(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.ends_with(".wasm")))
        .collect();

    let chosen: &serde_json::Value = if wasm_assets.is_empty() {
        return Err(Error::Input(format!(
            "the latest release of {owner}/{repo} has no .wasm asset — this grammar doesn't ship a \
             prebuilt WASM. Build it from source with `tree-sitter build --wasm` and use \
             `codemark languages add` instead."
        )));
    } else if let [only] = wasm_assets.as_slice() {
        let asset_name = only.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if grammar_entry_count > 1 && !wasm_name_matches(asset_name, grammar_name) {
            // Multi-grammar repo with a single published .wasm: we can't assume
            // that lone asset is the module for the grammar the user chose via
            // --name. Installing it would write a manifest for '{grammar_name}'
            // over a different grammar's module. Refuse rather than mismatch.
            return Err(Error::Input(format!(
                "{owner}/{repo} declares multiple grammars but its latest release ships a single \
                 .wasm ('{asset_name}') that doesn't match grammar '{grammar_name}'. It may be a \
                 different grammar's module — download the correct .wasm and use \
                 `codemark languages add`."
            )));
        }
        // Single-grammar repo (or a matching lone asset): the sole asset *is* the
        // grammar. Repos name it arbitrarily (`parser.wasm`, `<repo>.wasm`), so
        // no further name check — that would only cause false negatives.
        only
    } else {
        // Multiple .wasm — require an unambiguous match on the grammar name.
        let matches: Vec<&serde_json::Value> = wasm_assets
            .iter()
            .copied()
            .filter(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| wasm_name_matches(n, grammar_name))
            })
            .collect();
        if let [one] = matches.as_slice() {
            one
        } else {
            let names: Vec<&str> =
                wasm_assets.iter().filter_map(|a| a.get("name").and_then(|n| n.as_str())).collect();
            return Err(Error::Input(format!(
                "the latest release of {owner}/{repo} has multiple .wasm assets and none \
                 unambiguously matches grammar '{grammar_name}' ({}). Rename the grammar with \
                 --name to match an asset, or download the right .wasm and use \
                 `codemark languages add`.",
                names.join(", ")
            )));
        }
    };

    Ok(ReleaseAsset {
        name: chosen.get("name").and_then(|n| n.as_str()).unwrap_or("grammar.wasm").to_string(),
        url: chosen
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| Error::Operation("release asset has no download URL".to_string()))?
            .to_string(),
    })
}

/// Whether a release asset filename `asset_name` corresponds to grammar
/// `grammar_name`. Compares the `.wasm` stem case-insensitively against the name
/// and the Tree-sitter `tree-sitter-<name>` convention, tolerating `_`/`-`
/// differences (e.g. `c_sharp` vs `c-sharp`).
fn wasm_name_matches(asset_name: &str, grammar_name: &str) -> bool {
    let stem = asset_name.strip_suffix(".wasm").unwrap_or(asset_name);
    let norm = |s: &str| s.to_lowercase().replace('_', "-");
    let stem = norm(stem);
    let name = norm(grammar_name);
    stem == name || stem == format!("tree-sitter-{name}")
}

/// Download `url` into memory, rejecting an oversized body.
async fn download_capped(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    tracing::debug!(target: "codemark::http", %url, "downloading grammar.wasm");
    let mut resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "codemark")
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to download grammar.wasm: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Operation(format!(
            "failed to download grammar.wasm: HTTP {}",
            resp.status()
        )));
    }
    // Fast reject when the server advertises a length.
    if let Some(len) = resp.content_length()
        && len > MAX_WASM_BYTES
    {
        return Err(Error::Operation(format!(
            "grammar.wasm is too large ({len} bytes, limit {MAX_WASM_BYTES})"
        )));
    }
    // Stream the body and enforce the cap as we go, so a missing/lying
    // Content-Length or a chunked response can't buffer an unbounded body into
    // memory before the check (the earlier `bytes()` path read it all first).
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| Error::Operation(format!("failed to read grammar.wasm body: {e}")))?
    {
        if bytes.len() as u64 + chunk.len() as u64 > MAX_WASM_BYTES {
            return Err(Error::Operation(format!(
                "grammar.wasm is too large (exceeds limit {MAX_WASM_BYTES} bytes)"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_accepts_slug_prefix_and_url_forms() {
        for input in [
            "tree-sitter/tree-sitter-bash",
            "github:tree-sitter/tree-sitter-bash",
            "https://github.com/tree-sitter/tree-sitter-bash",
            "https://github.com/tree-sitter/tree-sitter-bash.git",
            "git@github.com:tree-sitter/tree-sitter-bash.git",
            "tree-sitter/tree-sitter-bash/",
        ] {
            let (owner, repo) = parse_source(input).unwrap_or_else(|_| panic!("{input}"));
            assert_eq!((owner.as_str(), repo.as_str()), ("tree-sitter", "tree-sitter-bash"));
        }
    }

    #[test]
    fn parse_source_rejects_malformed() {
        assert!(parse_source("bash").is_err());
        assert!(parse_source("a/b/c").is_err());
        assert!(parse_source("").is_err());
    }

    #[test]
    fn parse_tree_sitter_meta_extracts_name_filetypes_version() {
        let json = serde_json::json!({
            "grammars": [{ "name": "bash", "file-types": ["sh", "bash", ".bashrc"] }],
            "metadata": { "version": "0.25.1" }
        });
        let meta = parse_tree_sitter_meta(&json, None);
        assert_eq!(meta.name.as_deref(), Some("bash"));
        assert_eq!(meta.file_types, vec!["sh", "bash", ".bashrc"]);
        assert_eq!(meta.version.as_deref(), Some("0.25.1"));
        assert_eq!(meta.grammar_count, 1);
    }

    #[test]
    fn parse_tree_sitter_meta_tolerates_missing_fields() {
        let meta = parse_tree_sitter_meta(&serde_json::json!({}), None);
        assert!(meta.name.is_none());
        assert!(meta.file_types.is_empty());
        assert!(meta.version.is_none());
        assert_eq!(meta.grammar_count, 0);
    }

    #[test]
    fn parse_tree_sitter_meta_selects_entry_matching_requested_name() {
        // A multi-grammar repo (typescript + tsx). Without a name we default to
        // the first entry; with --name tsx we must pick the tsx entry's metadata,
        // not typescript's — otherwise a single .wasm install writes the wrong
        // extensions.
        let json = serde_json::json!({
            "grammars": [
                { "name": "typescript", "file-types": ["ts"] },
                { "name": "tsx", "file-types": ["tsx"] }
            ]
        });

        let first = parse_tree_sitter_meta(&json, None);
        assert_eq!(first.name.as_deref(), Some("typescript"));
        assert_eq!(first.file_types, vec!["ts"]);
        assert_eq!(first.grammar_count, 2);

        let picked = parse_tree_sitter_meta(&json, Some("tsx"));
        assert_eq!(picked.name.as_deref(), Some("tsx"));
        assert_eq!(picked.file_types, vec!["tsx"]);
        assert_eq!(picked.grammar_count, 2);
        assert!(picked.requested_name_matched);
    }

    #[test]
    fn parse_tree_sitter_meta_flags_unmatched_requested_name() {
        // A requested name that matches no entry falls back to the first entry's
        // metadata but reports requested_name_matched = false, so handle_install
        // can refuse (or demand --extensions) rather than mislabel the grammar.
        let json = serde_json::json!({
            "grammars": [{ "name": "bash", "file-types": ["sh"] }]
        });
        let meta = parse_tree_sitter_meta(&json, Some("nonexistent"));
        assert_eq!(meta.name.as_deref(), Some("bash"));
        assert_eq!(meta.file_types, vec!["sh"]);
        assert!(!meta.requested_name_matched);

        // No name requested is not a "mismatch".
        let none = parse_tree_sitter_meta(&json, None);
        assert!(!none.requested_name_matched);

        // An exact match sets the flag.
        let matched = parse_tree_sitter_meta(&json, Some("bash"));
        assert!(matched.requested_name_matched);
    }

    #[test]
    fn wasm_name_matches_bare_and_tree_sitter_prefixed() {
        // Bare `<name>.wasm` and the `tree-sitter-<name>.wasm` convention both match.
        assert!(wasm_name_matches("lua.wasm", "lua"));
        assert!(wasm_name_matches("tree-sitter-lua.wasm", "lua"));
        // Case-insensitive, and `_`/`-` are treated the same (c_sharp vs c-sharp).
        assert!(wasm_name_matches("tree-sitter-c-sharp.wasm", "c_sharp"));
        assert!(wasm_name_matches("Lua.wasm", "lua"));
    }

    #[test]
    fn wasm_name_does_not_match_unrelated_or_substring() {
        // A different grammar's asset must not match.
        assert!(!wasm_name_matches("tree-sitter-python.wasm", "lua"));
        // Substring is not enough — `lua` must not match `lualatex`.
        assert!(!wasm_name_matches("lualatex.wasm", "lua"));
    }
}
