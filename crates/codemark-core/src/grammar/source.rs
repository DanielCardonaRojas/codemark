//! Resolving a grammar from some *source* into installable bytes + metadata.
//!
//! Everything above the [`install_grammar`](super::install_grammar) seam is
//! "turn a user spec into a [`ResolvedGrammar`]", hidden behind the
//! [`GrammarSource`] trait so the install pipeline doesn't know or care whether
//! a grammar came from a local file, a GitHub release, or (later) a grammar
//! registry. Adding a registry is then `impl GrammarSource for RegistrySource`
//! plus one arm in [`select_source`] — no change to the hardened install path.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Cap on a downloaded `grammar.wasm` (32 MiB). Real grammars are well under
/// this; a larger response means a wrong/corrupt asset.
const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

/// A grammar resolved from some source, ready to install. Source-agnostic: a
/// local file, a GitHub release, or a registry all produce this same shape, so
/// the install pipeline is written once.
#[derive(Debug)]
pub struct ResolvedGrammar {
    /// The language name to install under (may be overridden by the caller).
    pub name: Option<String>,
    /// Raw, comma-separated extensions as the source reported them (may be
    /// overridden by the caller). Normalized later by
    /// [`validate_name_and_extensions`](super::validate_name_and_extensions).
    pub raw_extensions: Option<String>,
    /// Curated structural profile to write into the manifest. `None` means an
    /// empty profile (local files and GitHub releases); a registry can supply a
    /// real one — the whole point of a registry.
    pub profile: Option<serde_json::Value>,
    /// Where the wasm bytes come from. Deferred so `resolve` stays cheap
    /// (metadata only) and the big download happens once, after checks.
    pub wasm: WasmPayload,
}

/// Where a resolved grammar's `grammar.wasm` bytes come from.
#[derive(Debug)]
pub enum WasmPayload {
    /// Already in hand (a local `add`).
    Bytes(Vec<u8>),
    /// Fetch lazily via [`download_capped`], after name/version checks pass.
    Url(String),
}

impl WasmPayload {
    /// Materialize the bytes, downloading (capped) if this is a [`WasmPayload::Url`].
    pub async fn into_bytes(self, client: &reqwest::Client) -> Result<Vec<u8>> {
        match self {
            WasmPayload::Bytes(b) => Ok(b),
            WasmPayload::Url(url) => download_capped(client, &url).await,
        }
    }
}

/// Options that steer a [`GrammarSource::resolve`] beyond the bare spec.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResolveOptions<'a> {
    /// The caller's `--name` override, which a source may need to disambiguate a
    /// multi-grammar repo (selects the grammar entry, matches the release asset).
    pub requested_name: Option<&'a str>,
    /// The caller's `--release <tag>` override, pinning a specific GitHub release
    /// (ignored by sources that don't have releases).
    pub requested_release: Option<&'a str>,
    /// `--allow-version-mismatch`: skip the up-front ABI gate (a source's
    /// package.json `tree-sitter-cli` check). The staged-load validation before
    /// the install swap is still the final backstop.
    pub allow_version_mismatch: bool,
}

/// A place codemark can resolve a grammar from. One `resolve` per source kind;
/// the install pipeline treats them uniformly.
#[async_trait::async_trait]
pub trait GrammarSource {
    /// Resolve a user `spec` into installable metadata.
    ///
    /// Does **not** download the wasm — that is deferred to [`WasmPayload`] so
    /// name/version checks run before the large transfer.
    async fn resolve(&self, spec: &str, opts: ResolveOptions<'_>) -> Result<ResolvedGrammar>;
}

/// A grammar sitting on the local filesystem as a compiled `.wasm`
/// (`codemark languages add`). Carries no metadata beyond the bytes, so name and
/// extensions must be supplied by the caller.
pub struct LocalFileSource;

impl LocalFileSource {
    /// Resolve directly from a filesystem [`Path`](std::path::Path), preserving a
    /// non-UTF-8 path exactly (the string-`spec` trait method would lossily
    /// convert it). This is the path `codemark languages add` uses.
    pub fn resolve_path(&self, path: &std::path::Path) -> Result<ResolvedGrammar> {
        // Read the bytes once — validation and the committed install use these
        // same bytes, so a concurrent edit of the source path can't make us
        // validate one grammar and install a different one (TOCTOU).
        let bytes = read_capped_file(path)?;
        Ok(ResolvedGrammar {
            name: None,
            raw_extensions: None,
            profile: None,
            wasm: WasmPayload::Bytes(bytes),
        })
    }
}

/// Read a local `.wasm` file into memory, requiring a **regular file** and
/// bounding the read to [`MAX_WASM_BYTES`].
///
/// A plain `std::fs::read` would happily open a FIFO/device/socket the user
/// pointed us at (`codemark languages add /dev/zero`) and block or fill memory,
/// and would ignore the size cap the remote download path enforces. Requiring a
/// regular file rejects the former; reading through a `take(cap + 1)` reader
/// bounds memory and catches a file that grows past the cap between the stat and
/// EOF.
///
/// The regular-file / size checks run against the **opened descriptor's**
/// metadata (not the pathname), so the path can't be swapped for a device
/// between a check and the open. We deliberately don't go further (a
/// nonblocking/no-follow open): this is a single-user, local `add` of a
/// user-chosen path with no privilege boundary, and the read is already capped —
/// the residual (a user replacing their own path with a slow blocking device
/// mid-call) isn't worth the platform-specific `O_NONBLOCK`/`O_NOFOLLOW` code.
fn read_capped_file(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|e| {
        Error::Input(format!("WASM file not found or unreadable: {}: {e}", path.display()))
    })?;
    // Check the *handle's* metadata, so the type/size we validate is the file we
    // actually opened, not whatever the pathname resolved to a moment earlier.
    let meta =
        file.metadata().map_err(|e| Error::Operation(format!("Failed to stat WASM file: {e}")))?;
    if !meta.is_file() {
        return Err(Error::Input(format!(
            "{} is not a regular file — pass a compiled .wasm grammar",
            path.display()
        )));
    }
    if meta.len() > MAX_WASM_BYTES {
        return Err(Error::Input(format!(
            "{} is too large ({} bytes, limit {MAX_WASM_BYTES})",
            path.display(),
            meta.len()
        )));
    }

    // `take(cap + 1)` bounds the read and lets us distinguish "exactly at the
    // cap" from "grew past it" if the file changed after the stat above.
    let mut bytes = Vec::new();
    file.take(MAX_WASM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Operation(format!("Failed to read WASM file: {e}")))?;
    if bytes.len() as u64 > MAX_WASM_BYTES {
        return Err(Error::Input(format!(
            "{} is too large (exceeds limit {MAX_WASM_BYTES} bytes)",
            path.display()
        )));
    }
    // Byte count only — never echo the user-provided local path into logs.
    tracing::trace!(target: "codemark::languages", bytes = bytes.len(), "read local grammar.wasm");
    Ok(bytes)
}

#[async_trait::async_trait]
impl GrammarSource for LocalFileSource {
    async fn resolve(&self, spec: &str, _opts: ResolveOptions<'_>) -> Result<ResolvedGrammar> {
        self.resolve_path(&PathBuf::from(spec))
    }
}

/// A Tree-sitter grammar GitHub repo shipping a prebuilt `.wasm` release asset
/// (`codemark languages install owner/repo`). Metadata (name, file-types,
/// version) comes from the repo's `tree-sitter.json`.
pub struct GithubSource {
    client: reqwest::Client,
}

impl GithubSource {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
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

    /// Fetch and parse `tree-sitter.json` at a specific git ref (a release tag).
    ///
    /// Returns `None` if the repo has no `tree-sitter.json` at that ref (404) so
    /// the release scan can skip it; other HTTP/JSON failures are errors.
    /// `requested_name` (an explicit `--name`) selects the matching grammar entry
    /// in a multi-grammar repo; see [`parse_tree_sitter_meta`].
    async fn fetch_tree_sitter_json_at(
        &self,
        owner: &str,
        repo: &str,
        git_ref: &str,
        requested_name: Option<&str>,
    ) -> Result<Option<TreeSitterMeta>> {
        let url =
            format!("https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/tree-sitter.json");
        tracing::debug!(target: "codemark::http", %url, "fetching tree-sitter.json at ref");
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Operation(format!("failed to fetch tree-sitter.json: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
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
        Ok(Some(parse_tree_sitter_meta(&json, requested_name)))
    }

    /// Fetch a single release: the one tagged `tag` if given, else the latest.
    /// Returns the raw release JSON (with its `assets` and `tag_name`).
    async fn fetch_release(
        &self,
        owner: &str,
        repo: &str,
        tag: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = match tag {
            Some(t) => format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{t}"),
            None => format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"),
        };
        tracing::debug!(target: "codemark::http", %url, "fetching release");
        let resp = self
            .client
            .get(&url)
            // GitHub's API requires a User-Agent.
            .header(reqwest::header::USER_AGENT, "codemark")
            .send()
            .await
            .map_err(|e| Error::Operation(format!("failed to query release: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::Input(match tag {
                Some(t) => format!("{owner}/{repo} has no release tagged '{t}'"),
                None => format!("{owner}/{repo} has no releases"),
            }));
        }
        if !resp.status().is_success() {
            return Err(Error::Operation(format!(
                "failed to fetch release of {owner}/{repo}: HTTP {}",
                resp.status()
            )));
        }
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| Error::Operation(format!("release JSON invalid: {e}")))
    }

    /// Fetch and parse a release tag's `package.json`, returning the
    /// `tree-sitter-cli` version as a normalized `(major, minor)`.
    ///
    /// This — **not** `tree-sitter.json`'s `metadata.version` — is the true ABI
    /// signal: it's the CLI that compiled the grammar. `metadata.version` is the
    /// grammar package's own semver (e.g. `elm` at `5.9.4` while built with the
    /// 0.26 CLI), so keying on it would misjudge compatibility.
    ///
    /// Returns `None` if the repo ships no readable `package.json` or no
    /// parseable `tree-sitter-cli`, so the caller can fall through to the
    /// staged-load validation rather than falsely rejecting the grammar.
    async fn fetch_cli_minor_at(&self, owner: &str, repo: &str, tag: &str) -> Option<(u64, u64)> {
        let url = format!("https://raw.githubusercontent.com/{owner}/{repo}/{tag}/package.json");
        tracing::debug!(target: "codemark::http", %url, "fetching package.json at ref");
        let resp = self.client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        parse_tree_sitter_cli_minor(&json)
    }

    /// Choose the `.wasm` asset for `grammar_name` from an already-fetched
    /// release's `assets` array. `grammar_entry_count` is how many grammars the
    /// selected release's `tree-sitter.json` declared, which decides how strictly
    /// the asset must match the name.
    ///
    /// A release may carry several `.wasm` modules (multi-grammar repos, or
    /// debug/release builds). Grabbing an asset independently of the chosen
    /// grammar can pair the manifest with a different module, so:
    ///
    /// - **Multiple `.wasm` assets:** require an unambiguous match on the grammar
    ///   name (Tree-sitter's convention is `tree-sitter-<name>.wasm` /
    ///   `<name>.wasm`).
    /// - **One `.wasm` asset, single-grammar repo:** accept it — the sole asset
    ///   *is* the grammar, and repos name it arbitrarily (`parser.wasm`,
    ///   `<repo>.wasm`), so a name check would only cause false negatives.
    /// - **One `.wasm` asset, multi-grammar repo:** the user chose one grammar
    ///   via `--name`, but a lone asset can't be assumed to be *that* grammar's
    ///   module, so require the asset name to match and otherwise refuse rather
    ///   than install a mismatched module.
    ///
    /// Otherwise error with the candidate list rather than guessing.
    fn pick_wasm_asset(
        assets: &[serde_json::Value],
        owner: &str,
        repo: &str,
        tag: &str,
        grammar_name: &str,
        grammar_entry_count: usize,
    ) -> Result<ReleaseAsset> {
        let wasm_assets: Vec<&serde_json::Value> = assets
            .iter()
            .filter(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(is_wasm_asset))
            .collect();

        let chosen: &serde_json::Value = if wasm_assets.is_empty() {
            // select_compatible_release only picks releases with a .wasm, so this
            // is unreachable in the normal flow — kept as a defensive error.
            return Err(Error::Input(format!(
                "release {tag} of {owner}/{repo} has no .wasm asset — build it from source with \
                 `tree-sitter build --wasm` and use `codemark languages add` instead."
            )));
        } else if let [only] = wasm_assets.as_slice() {
            let asset_name = only.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if grammar_entry_count > 1 && !wasm_name_matches(asset_name, grammar_name) {
                // Multi-grammar repo with a single published .wasm: we can't
                // assume that lone asset is the module for the grammar the user
                // chose via --name. Installing it would write a manifest for
                // '{grammar_name}' over a different grammar's module. Refuse.
                return Err(Error::Input(format!(
                    "{owner}/{repo} declares multiple grammars but release {tag} ships a single \
                     .wasm ('{asset_name}') that doesn't match grammar '{grammar_name}'. It may be \
                     a different grammar's module — download the correct .wasm and use \
                     `codemark languages add`."
                )));
            }
            // Single-grammar repo (or a matching lone asset): the sole asset *is*
            // the grammar. Repos name it arbitrarily (`parser.wasm`,
            // `<repo>.wasm`), so no further name check — that would only cause
            // false negatives.
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
                let names: Vec<&str> = wasm_assets
                    .iter()
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .collect();
                return Err(Error::Input(format!(
                    "release {tag} of {owner}/{repo} has multiple .wasm assets and none \
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
}

#[async_trait::async_trait]
impl GrammarSource for GithubSource {
    async fn resolve(&self, spec: &str, opts: ResolveOptions<'_>) -> Result<ResolvedGrammar> {
        let (owner, repo) = Self::parse_source(spec)?;
        let requested_name = opts.requested_name;

        // Take the pinned `--release <tag>` if given, else the latest release.
        // (We don't scan older releases: the tag's package.json tells us up front
        // whether it's a 0.25 build, so we fail fast with a --release hint instead
        // of downloading many .wasm modules to probe.)
        let release = self.fetch_release(&owner, &repo, opts.requested_release).await?;
        let tag = release
            .get("tag_name")
            .and_then(|t| t.as_str())
            .ok_or_else(|| Error::Operation("release has no tag_name".to_string()))?
            .to_string();
        let assets = release.get("assets").and_then(|a| a.as_array()).cloned().unwrap_or_default();

        // ABI gate via that tag's package.json `tree-sitter-cli` — the CLI that
        // compiled the grammar, which is the real ABI signal (unlike
        // tree-sitter.json's metadata.version, which is the grammar's own semver).
        // Absent/unparseable → don't block; the staged-load validation is the
        // backstop. Too old → refuse. Newer than 0.25 → refuse with a --release
        // hint so the user can pin an older 0.25 build. `--allow-version-mismatch`
        // skips this up-front gate entirely (staged-load still validates).
        if !opts.allow_version_mismatch {
            match self.fetch_cli_minor_at(&owner, &repo, &tag).await {
                Some((0, 25)) => {}
                Some((major, minor)) if (major, minor) < (0, 25) => {
                    return Err(Error::Input(format!(
                        "release {tag} of {owner}/{repo} was built with Tree-sitter {major}.{minor} \
                         (from package.json), but codemark loads 0.25 grammars. Rebuild it from \
                         source with the 0.25 CLI and use `codemark languages add`, or pass \
                         --allow-version-mismatch to try anyway."
                    )));
                }
                Some((major, minor)) => {
                    return Err(Error::Input(format!(
                        "release {tag} of {owner}/{repo} was built with Tree-sitter {major}.{minor} \
                         (from package.json), but codemark loads 0.25 grammars. Pass `--release \
                         <tag>` to pick an older 0.25 release, use `codemark languages add` with a \
                         0.25 `.wasm`, or pass --allow-version-mismatch to try anyway."
                    )));
                }
                None => {
                    tracing::debug!(target: "codemark::languages", %tag, "no package.json tree-sitter-cli; relying on staged-load validation");
                }
            }
        }

        // Name / extensions from that tag's tree-sitter.json.
        let meta = self
            .fetch_tree_sitter_json_at(&owner, &repo, &tag, requested_name)
            .await?
            .ok_or_else(|| {
                Error::Input(format!(
                    "no tree-sitter.json found in {owner}/{repo} at {tag} — is this a Tree-sitter \
                     grammar repo?"
                ))
            })?;

        // A multi-grammar repo (e.g. typescript + tsx) exposes several grammar
        // entries but often ships a single .wasm. Without --name we'd default to
        // the first entry's name/extensions, which may not correspond to the
        // published module — installing files that parse with the wrong grammar.
        // Require the caller to disambiguate rather than guessing.
        if meta.grammar_count > 1 && requested_name.is_none() {
            return Err(Error::Input(format!(
                "{owner}/{repo} declares {} grammars in tree-sitter.json — pass --name to choose \
                 which one to install (its name must match a grammar entry).",
                meta.grammar_count
            )));
        }

        // --name was given but matches no declared grammar. `meta.name`/
        // `file_types` then fell back to the first entry, so installing under the
        // requested name would label the real grammar (and its extensions) with
        // an unrelated identity — bookmarks and lookups for that name would
        // silently miss. Refuse and point at the declared name. (Installing under
        // a custom local name is deferred to a follow-up; keep this unambiguous.)
        if requested_name.is_some() && meta.grammar_count > 0 && !meta.requested_name_matched {
            let requested = requested_name.unwrap_or_default();
            let declared = meta.name.as_deref().unwrap_or("<unknown>");
            return Err(Error::Input(format!(
                "--name '{requested}' doesn't match any grammar declared in {owner}/{repo}'s \
                 tree-sitter.json (found '{declared}'). Install it under its declared name \
                 '{declared}'."
            )));
        }

        // The name the install will use: the --name override (already validated
        // to match a declared entry above) or the resolved declared name. Drives
        // which release asset is matched.
        let name =
            requested_name.map(str::to_string).or_else(|| meta.name.clone()).ok_or_else(|| {
                Error::Input(
                    "could not determine the language name; pass --name explicitly".to_string(),
                )
            })?;

        let asset = Self::pick_wasm_asset(&assets, &owner, &repo, &tag, &name, meta.grammar_count)?;
        tracing::debug!(target: "codemark::languages", asset = %asset.name, %tag, "resolved github grammar asset");
        Ok(ResolvedGrammar {
            name: meta.name,
            raw_extensions: (!meta.file_types.is_empty()).then(|| meta.file_types.join(",")),
            profile: None,
            wasm: WasmPayload::Url(asset.url),
        })
    }
}

/// Pick the [`GrammarSource`] for a user `spec`: an existing path on disk is a
/// local file, anything else is treated as a GitHub grammar source. (A future
/// registry would slot in here — e.g. a bare `name`/`name@version` with no `/`.)
pub fn select_source(spec: &str, client: reqwest::Client) -> Box<dyn GrammarSource> {
    if std::path::Path::new(spec).exists() {
        Box::new(LocalFileSource)
    } else {
        Box::new(GithubSource::new(client))
    }
}

/// A Tree-sitter grammar repo's `tree-sitter.json` metadata (only the fields we
/// use). Unknown fields are ignored.
///
/// Deliberately does **not** carry `metadata.version`: that is the grammar
/// package's own semver (e.g. `elm` at `5.9.4`), not the Tree-sitter ABI, so it
/// must never drive compatibility. The ABI comes from `package.json`'s
/// `tree-sitter-cli` (see [`parse_tree_sitter_cli_minor`]).
#[derive(Debug, Default)]
struct TreeSitterMeta {
    name: Option<String>,
    file_types: Vec<String>,
    /// How many grammar entries `tree-sitter.json` declared. >1 means a
    /// multi-grammar repo where `name`/`file_types` (taken from a single entry)
    /// can't be assumed to match the published `.wasm` unless the caller named
    /// the grammar explicitly.
    grammar_count: usize,
    /// Whether the caller's `--name` matched a declared grammar entry. `false`
    /// (with a name requested) means `name`/`file_types` fell back to the first
    /// entry and don't describe the requested grammar.
    requested_name_matched: bool,
}

/// Extract the name and file-types from a parsed `tree-sitter.json`.
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
    TreeSitterMeta { name, file_types, grammar_count, requested_name_matched }
}

/// A release asset we can download.
struct ReleaseAsset {
    name: String,
    url: String,
}

/// Whether a release asset filename is a WASM grammar module.
fn is_wasm_asset(asset_name: &str) -> bool {
    asset_name.ends_with(".wasm")
}

/// Extract the `tree-sitter-cli` version from a `package.json` as a normalized
/// `(major, minor)` — the CLI that compiled the grammar, hence its WASM ABI.
///
/// Reads `devDependencies` then `dependencies`, strips a leading `^`/`~`/`>=`/
/// `=`/`v`, and parses the leading `major.minor`. Returns `None` if absent or
/// unparseable so the caller falls through to the staged-load backstop rather
/// than falsely rejecting.
fn parse_tree_sitter_cli_minor(package_json: &serde_json::Value) -> Option<(u64, u64)> {
    let spec = ["devDependencies", "dependencies"]
        .iter()
        .find_map(|k| package_json.get(k).and_then(|d| d.get("tree-sitter-cli")))
        .and_then(|v| v.as_str())?;

    // Drop a leading range/prefix operator and any `v`, then take major.minor.
    let s = spec.trim().trim_start_matches(['^', '~', '=', '>', '<', 'v', ' ']);
    let mut parts = s.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    // A missing minor (bare `1`) reads as `.0`.
    let minor = parts.next().and_then(|m| {
        // Stop at the first non-digit (e.g. `6-rc.1`).
        let digits: String = m.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u64>().ok()
    })?;
    Some((major, minor))
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
    // memory before the check (a plain `bytes()` would read it all first).
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
            let (owner, repo) =
                GithubSource::parse_source(input).unwrap_or_else(|_| panic!("{input}"));
            assert_eq!((owner.as_str(), repo.as_str()), ("tree-sitter", "tree-sitter-bash"));
        }
    }

    #[test]
    fn parse_source_rejects_malformed() {
        assert!(GithubSource::parse_source("bash").is_err());
        assert!(GithubSource::parse_source("a/b/c").is_err());
        assert!(GithubSource::parse_source("").is_err());
    }

    #[test]
    fn parse_tree_sitter_meta_extracts_name_and_filetypes() {
        let json = serde_json::json!({
            "grammars": [{ "name": "bash", "file-types": ["sh", "bash", ".bashrc"] }],
            "metadata": { "version": "0.25.1" }
        });
        let meta = parse_tree_sitter_meta(&json, None);
        assert_eq!(meta.name.as_deref(), Some("bash"));
        assert_eq!(meta.file_types, vec!["sh", "bash", ".bashrc"]);
        assert_eq!(meta.grammar_count, 1);
    }

    #[test]
    fn parse_tree_sitter_meta_tolerates_missing_fields() {
        let meta = parse_tree_sitter_meta(&serde_json::json!({}), None);
        assert!(meta.name.is_none());
        assert!(meta.file_types.is_empty());
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
        // metadata but reports requested_name_matched = false, so the source can
        // refuse rather than mislabel the grammar.
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

    #[test]
    fn read_capped_file_reads_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grammar.wasm");
        std::fs::write(&path, b"\0asm\x01\x00\x00\x00").unwrap();
        assert_eq!(read_capped_file(&path).unwrap(), b"\0asm\x01\x00\x00\x00");
    }

    #[test]
    fn read_capped_file_rejects_a_directory() {
        // A non-regular path (here a directory) is refused rather than read —
        // stands in for FIFOs/devices/sockets that could block or fill memory.
        let dir = tempfile::tempdir().unwrap();
        assert!(read_capped_file(dir.path()).is_err());
    }

    #[test]
    fn read_capped_file_rejects_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_capped_file(&dir.path().join("nope.wasm")).is_err());
    }

    // --- release selection ---

    fn release(tag: &str, asset_names: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "assets": asset_names.iter().map(|n| serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://example.test/{tag}/{n}"),
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn parse_tree_sitter_cli_minor_reads_devdependencies() {
        // The real ABI signal, across the specifier forms grammars use in the
        // wild (verified against scala/bash/elm/typescript).
        let cases = [
            (serde_json::json!({"devDependencies": {"tree-sitter-cli": "^0.26.8"}}), Some((0, 26))),
            (serde_json::json!({"devDependencies": {"tree-sitter-cli": "^0.25.6"}}), Some((0, 25))),
            (serde_json::json!({"devDependencies": {"tree-sitter-cli": "^0.24.4"}}), Some((0, 24))),
            (serde_json::json!({"devDependencies": {"tree-sitter-cli": "~0.25.1"}}), Some((0, 25))),
            (serde_json::json!({"devDependencies": {"tree-sitter-cli": "0.25.9"}}), Some((0, 25))),
            (
                serde_json::json!({"devDependencies": {"tree-sitter-cli": ">=0.25.0"}}),
                Some((0, 25)),
            ),
            // dependencies (not dev) is also honored.
            (serde_json::json!({"dependencies": {"tree-sitter-cli": "^0.25.0"}}), Some((0, 25))),
            // elm-style: grammar semver in metadata is irrelevant; only the CLI matters.
            (
                serde_json::json!({"version": "5.9.4", "devDependencies": {"tree-sitter-cli": "^0.26.10"}}),
                Some((0, 26)),
            ),
        ];
        for (json, expected) in cases {
            assert_eq!(parse_tree_sitter_cli_minor(&json), expected, "for {json}");
        }
    }

    #[test]
    fn parse_tree_sitter_cli_minor_none_when_absent_or_unparseable() {
        assert_eq!(parse_tree_sitter_cli_minor(&serde_json::json!({})), None);
        assert_eq!(
            parse_tree_sitter_cli_minor(
                &serde_json::json!({"devDependencies": {"other": "1.2.3"}})
            ),
            None
        );
        // A bare major with no minor isn't a usable ABI signal.
        assert_eq!(
            parse_tree_sitter_cli_minor(
                &serde_json::json!({"devDependencies": {"tree-sitter-cli": "1"}})
            ),
            None
        );
        // Non-semver garbage.
        assert_eq!(
            parse_tree_sitter_cli_minor(
                &serde_json::json!({"devDependencies": {"tree-sitter-cli": "latest"}})
            ),
            None
        );
    }

    #[test]
    fn pick_wasm_asset_selects_the_sole_asset() {
        // scala-shaped release: one wasm, single-grammar repo.
        let assets = release("v0.25.1", &["tree-sitter-scala.wasm"])
            .get("assets")
            .and_then(|a| a.as_array())
            .unwrap()
            .clone();
        let asset = GithubSource::pick_wasm_asset(
            &assets,
            "tree-sitter",
            "tree-sitter-scala",
            "v0.25.1",
            "scala",
            1,
        )
        .unwrap();
        assert_eq!(asset.name, "tree-sitter-scala.wasm");
        assert!(asset.url.ends_with("/v0.25.1/tree-sitter-scala.wasm"));
    }

    #[test]
    fn pick_wasm_asset_matches_the_named_grammar_among_many() {
        // Multi-.wasm release: pick the one matching --name.
        let assets = release("v0.25.0", &["tree-sitter-typescript.wasm", "tree-sitter-tsx.wasm"])
            .get("assets")
            .and_then(|a| a.as_array())
            .unwrap()
            .clone();
        let asset = GithubSource::pick_wasm_asset(
            &assets,
            "tree-sitter",
            "tree-sitter-typescript",
            "v0.25.0",
            "tsx",
            2,
        )
        .unwrap();
        assert_eq!(asset.name, "tree-sitter-tsx.wasm");
    }

    #[test]
    fn pick_wasm_asset_errors_when_multi_grammar_lone_asset_mismatches() {
        // Multi-grammar repo, single .wasm that isn't the --name'd grammar → refuse.
        let assets = release("v0.25.0", &["tree-sitter-typescript.wasm"])
            .get("assets")
            .and_then(|a| a.as_array())
            .unwrap()
            .clone();
        let err = GithubSource::pick_wasm_asset(
            &assets,
            "tree-sitter",
            "tree-sitter-typescript",
            "v0.25.0",
            "tsx",
            2,
        );
        assert!(err.is_err());
    }

    #[test]
    fn pick_wasm_asset_errors_when_no_wasm() {
        let assets = release("v0.25.0", &["src.tar.gz"])
            .get("assets")
            .and_then(|a| a.as_array())
            .unwrap()
            .clone();
        assert!(GithubSource::pick_wasm_asset(&assets, "o", "r", "v0.25.0", "x", 1).is_err());
    }
}
