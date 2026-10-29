//! Fetching curated profiles from the grammar registry.
//!
//! The registry **hosts** profiles, not binaries: an `index.json` of what is
//! available plus a `manifest.json` per language. That keeps codemark out of
//! the ABI-pipeline and grammar-licensing business while removing the part that
//! actually hurts — hand-authoring a profile.
//!
//! An entry may additionally *point at* a prebuilt `grammar.wasm` published by
//! the grammar's own project (`wasm_url` + `wasm_sha256`), which is what makes
//! `codemark languages install <lang>` a single step. Codemark fetches but never
//! serves those bytes, and verifies them against the digest pinned in the index
//! before they reach the installer — see [`RegistryEntry::download`].
//!
//! The payoff lands through the seam that already exists:
//! [`install_grammar`](super::install_grammar) takes an optional profile and
//! writes it into the manifest verbatim. Here we resolve one by name; the
//! hardened stage-then-swap install is untouched.
//!
//! **Trust.** A profile is data, not code, but it reaches the manifest writer,
//! so it is schema-validated *and* linted against the grammar before anything is
//! committed, and a registry-supplied name/extension still goes through
//! [`validate_name_and_extensions`](super::validate_name_and_extensions) — a
//! registry entry must not be able to claim `rs` or shadow a built-in.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Cap on an `index.json` (4 MiB). The index is metadata for a few hundred
/// languages at most; larger means something is wrong at the other end.
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// Cap on a single `manifest.json` (1 MiB). A profile is a few hundred strings.
const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

/// Directory under the grammar cache holding registry state. Dot-prefixed so
/// `LanguageRegistry::discover_in` skips it — it already skips every dotted
/// entry for the installer's staging/backup/lock files.
const REGISTRY_CACHE_DIR: &str = ".registry";

/// The registry's `index.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryIndex {
    /// Wire-format version. Bumped only for a breaking change; new optional
    /// fields are additive and do not bump it — `wasm_url` was added this way.
    pub schema_version: u32,
    pub grammars: Vec<RegistryEntry>,
}

/// One language's entry in the index.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryEntry {
    pub name: String,
    pub extensions: Vec<String>,
    /// Version of the curated *profile*, independent of the grammar's version.
    #[serde(default)]
    pub profile_version: Option<String>,
    /// Path to the manifest, relative to the registry base URL.
    pub profile_url: String,
    /// Tree-sitter CLI version the profile was verified against. Informational:
    /// codemark's runtime accepts ABI 13–15 (roughly CLI 0.21–0.25), and the
    /// real check is whether the grammar loads, so this is not a gate.
    #[serde(default)]
    pub ts_abi: Option<String>,
    /// Upstream grammar repository, so the user knows what to build.
    #[serde(default)]
    pub grammar_repo: Option<String>,
    /// The grammar revision this profile was authored against. Node kinds change
    /// between grammar versions, so a profile is only valid against a revision.
    #[serde(default)]
    pub grammar_rev: Option<String>,
    #[serde(default)]
    pub description: Option<String>,

    /// Direct download for a prebuilt `grammar.wasm`, enabling
    /// `codemark languages install <lang>` instead of a manual build.
    ///
    /// Optional: an entry without it still works, the user just supplies the
    /// `.wasm` themselves. It must be given explicitly rather than derived from
    /// `grammar_repo` + `grammar_rev`, because a release can carry several wasm
    /// assets (tree-sitter-php ships both `tree-sitter-php.wasm` and
    /// `tree-sitter-php_only.wasm`) and only the entry knows which is wanted.
    #[serde(default)]
    pub wasm_url: Option<String>,

    /// SHA-256 of the bytes at [`Self::wasm_url`], lowercase hex.
    ///
    /// **Required whenever `wasm_url` is set.** Auto-download moves fetching off
    /// the user's desk, so the bytes codemark is about to load as a WASM module
    /// must be pinned by this reviewed, version-controlled file rather than by
    /// whatever the URL happens to serve today.
    #[serde(default)]
    pub wasm_sha256: Option<String>,

    /// This entry deliberately replaces a codemark built-in of the same name or
    /// extension (C# ships bundled, for instance, and is also served here).
    ///
    /// Installing over a built-in normally requires `--override-builtin`;
    /// setting this lets `languages install <lang>` do it without the flag,
    /// because the registry has already declared the intent. A collision in an
    /// entry that *doesn't* set it still fails, so an accidental one is still
    /// caught.
    ///
    /// The consent is scoped to what this entry actually declares — see
    /// [`Self::consents_to_override`].
    #[serde(default)]
    pub overrides_builtin: bool,
}

impl RegistryEntry {
    /// Whether this entry's `overrides_builtin` covers installing under
    /// `name` with `extensions`.
    ///
    /// The declaration is consent for *this* entry's grammar, not a blanket
    /// permit: it only applies while the effective name and every effective
    /// extension are ones the entry itself declares. Otherwise
    /// `languages install c_sharp --extensions cs,rs` would inherit the C#
    /// entry's consent and quietly take over `.rs` — a dynamic grammar wins
    /// resolution, so every Rust file would then parse as C#. Anything the
    /// entry didn't declare still needs the caller's `--override-builtin`.
    pub fn consents_to_override(&self, name: &str, extensions: &[String]) -> bool {
        if !self.overrides_builtin {
            return false;
        }
        // Compare on the same normalization the extensions were validated
        // under (dotless, lowercase), so a `.CS` in the index still matches.
        let declared = |ext: &str| {
            self.extensions
                .iter()
                .any(|d| d.trim().trim_start_matches('.').eq_ignore_ascii_case(ext))
        };
        name.eq_ignore_ascii_case(&self.name) && extensions.iter().all(|e| declared(e))
    }

    /// Whether this entry's own name or extensions are owned by a built-in, and
    /// so would need an override to install.
    pub fn collides_with_builtin(&self) -> bool {
        use crate::parser::languages::Language;
        Language::static_from_name(&self.name).is_some()
            || self.extensions.iter().any(|e| {
                Language::static_from_extension(&e.trim().trim_start_matches('.').to_lowercase())
                    .is_some()
            })
    }

    /// The download for this entry, if it offers one.
    ///
    /// `Err` when `wasm_url` is present but unusable — a missing checksum, a
    /// non-HTTPS URL, or a host that doesn't match `grammar_repo`. Those are
    /// registry-authoring mistakes, and reporting them beats silently falling
    /// back to "build it yourself".
    pub fn download(&self) -> Result<Option<RemoteWasm>> {
        let Some(url) = self.wasm_url.as_deref() else {
            return Ok(None);
        };

        let sha256 = self.wasm_sha256.as_deref().ok_or_else(|| {
            Error::Input(format!(
                "registry entry '{}' has a wasm_url but no wasm_sha256; refusing to download \
                 unpinned bytes",
                self.name
            ))
        })?;
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::Input(format!(
                "registry entry '{}' has a malformed wasm_sha256 (want 64 hex chars)",
                self.name
            )));
        }

        let parsed = reqwest::Url::parse(url).map_err(|e| {
            Error::Input(format!("registry entry '{}' has a bad wasm_url: {e}", self.name))
        })?;
        if parsed.scheme() != "https" {
            return Err(Error::Input(format!(
                "registry entry '{}' must use https for wasm_url",
                self.name
            )));
        }

        // Keep the binary and the repository it claims to come from on the same
        // host, so a registry entry can't direct a download somewhere unrelated
        // to the grammar it names.
        if let Some(repo) = self.grammar_repo.as_deref() {
            let repo_host =
                reqwest::Url::parse(repo).ok().and_then(|u| u.host_str().map(String::from));
            let wasm_host = parsed.host_str().map(String::from);
            if let (Some(r), Some(w)) = (repo_host, wasm_host)
                && !r.eq_ignore_ascii_case(&w)
            {
                return Err(Error::Input(format!(
                    "registry entry '{}': wasm_url host '{w}' does not match grammar_repo host \
                     '{r}'",
                    self.name
                )));
            }
        }

        Ok(Some(RemoteWasm { url: url.to_string(), sha256: sha256.to_lowercase() }))
    }
}

/// A verified-on-arrival grammar download.
#[derive(Debug, Clone)]
pub struct RemoteWasm {
    pub url: String,
    /// Lowercase hex SHA-256 the downloaded bytes must hash to.
    pub sha256: String,
}

/// Highest `schema_version` this build understands.
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Reads the grammar registry, caching the index on disk.
pub struct RegistryClient {
    base_url: String,
    cache_dir: PathBuf,
    ttl: Duration,
}

impl RegistryClient {
    /// Build a client from configuration.
    pub fn from_config(config: &crate::config::GrammarsConfig) -> Result<Self> {
        let cache_dir = crate::config::global_grammars_dir()
            .ok_or_else(|| {
                Error::Operation("Could not determine global grammars directory".to_string())
            })?
            .join(REGISTRY_CACHE_DIR);
        Ok(Self { base_url: config.registry_url(), cache_dir, ttl: config.index_ttl() })
    }

    /// Point a client at an explicit base URL (tests, fixtures, `--registry`).
    pub fn with_base_url(base_url: impl Into<String>, cache_dir: PathBuf, ttl: Duration) -> Self {
        Self { base_url: base_url.into().trim_end_matches('/').to_string(), cache_dir, ttl }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Where this registry's index is cached.
    ///
    /// Keyed by a hash of the base URL, so switching `registry_url` (or
    /// `CODEMARK_GRAMMAR_REGISTRY`) doesn't serve the previous registry's
    /// entries for the rest of the TTL — which would both list the wrong
    /// languages and join the old registry's `profile_url` paths onto the new
    /// base URL.
    fn index_cache_path(&self) -> PathBuf {
        self.cache_dir.join(format!("index-{}.json", url_key(&self.base_url)))
    }

    /// Fetch the registry index, using the on-disk cache when it is fresh.
    ///
    /// `refresh` forces a network read. A fetch failure falls back to a *stale*
    /// cache rather than erroring: a user who is offline, or behind a flaky
    /// network, should still be able to list and install from what they already
    /// have.
    pub async fn fetch_index(&self, refresh: bool) -> Result<RegistryIndex> {
        if !refresh && let Some(cached) = self.read_cached_index(self.ttl) {
            tracing::debug!(target: "codemark::http", "using cached registry index");
            return Ok(cached);
        }

        match self.fetch_index_uncached().await {
            Ok(index) => {
                self.write_cached_index(&index);
                Ok(index)
            }
            Err(e) => {
                // Any cache beats no registry when the network is unavailable.
                if let Some(stale) = self.read_cached_index(Duration::MAX) {
                    tracing::warn!(
                        target: "codemark::http",
                        error = %e,
                        "registry fetch failed; using the cached index"
                    );
                    eprintln!(
                        "codemark: could not reach the grammar registry ({e}); using the cached index"
                    );
                    return Ok(stale);
                }
                Err(e)
            }
        }
    }

    async fn fetch_index_uncached(&self) -> Result<RegistryIndex> {
        let url = format!("{}/index.json", self.base_url);
        let body = fetch_text(&url, MAX_INDEX_BYTES).await?;
        let index: RegistryIndex = serde_json::from_str(&body).map_err(|e| {
            Error::Operation(format!("registry index at {url} is not valid JSON: {e}"))
        })?;

        if index.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(Error::Operation(format!(
                "the registry uses index schema version {} but this codemark understands up to \
                 {SUPPORTED_SCHEMA_VERSION} — upgrade codemark",
                index.schema_version
            )));
        }
        Ok(index)
    }

    fn read_cached_index(&self, max_age: Duration) -> Option<RegistryIndex> {
        let path = self.index_cache_path();
        let meta = std::fs::metadata(&path).ok()?;
        if max_age != Duration::MAX {
            let age = SystemTime::now().duration_since(meta.modified().ok()?).ok()?;
            if age > max_age {
                return None;
            }
        }
        serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()
    }

    /// Best-effort: a cache we can't write is a performance problem, not an
    /// error worth failing an install over.
    fn write_cached_index(&self, index: &RegistryIndex) {
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        if let Ok(text) = serde_json::to_string_pretty(index) {
            let _ = std::fs::write(self.index_cache_path(), text);
        }
    }

    /// Look up one language's entry (case-insensitive).
    pub async fn find(&self, name: &str, refresh: bool) -> Result<RegistryEntry> {
        let index = self.fetch_index(refresh).await?;
        Self::pick(index, name)
    }

    /// As [`Self::find`], for an install that is about to check the entry's
    /// `overrides_builtin` against a built-in collision.
    ///
    /// A cache written before `overrides_builtin` existed has no such key, so it
    /// deserializes to `false` and the entry looks like it never declared the
    /// override — failing the very install this field was added to enable, for
    /// up to the index TTL. When the cached entry both collides with a built-in
    /// and declares nothing, re-read the index once from the network so the
    /// declaration gets a chance to appear. An entry that genuinely doesn't
    /// declare it ends up refused either way, just after one fetch.
    ///
    /// The refreshed index wins when it can be read at all — including when it
    /// has dropped the entry. Only an unreachable registry falls back to the
    /// cached copy.
    pub async fn find_for_install(&self, name: &str, refresh: bool) -> Result<RegistryEntry> {
        let entry = self.find(name, refresh).await?;
        if refresh || entry.overrides_builtin || !entry.collides_with_builtin() {
            return Ok(entry);
        }
        tracing::debug!(
            target: "codemark::http",
            language = %name,
            "cached entry collides with a built-in and declares no override; re-reading the index"
        );
        // Only a *failed* fetch falls back to the cached entry, so an offline
        // user is no worse off. A refresh that succeeds is authoritative: if it
        // no longer lists the entry, the language has been delisted and saying
        // so beats resurrecting a copy the registry has dropped.
        match self.fetch_index(true).await {
            Ok(index) => Self::pick(index, name),
            Err(_) => Ok(entry),
        }
    }

    fn pick(index: RegistryIndex, name: &str) -> Result<RegistryEntry> {
        let needle = name.to_lowercase();
        index.grammars.into_iter().find(|e| e.name.to_lowercase() == needle).ok_or_else(|| {
            Error::Input(format!(
                "'{name}' is not in the grammar registry — run `codemark languages registry \
                     list` to see what is available"
            ))
        })
    }

    /// Fetch the curated manifest for `entry`.
    ///
    /// Returns the whole manifest; the caller takes `profile` from it and
    /// validates the rest through the normal install path.
    pub async fn fetch_profile(&self, entry: &RegistryEntry) -> Result<serde_json::Value> {
        // A registry-supplied path must not escape the registry base: a
        // `profile_url` of `../../etc/passwd` on a `file://` registry would
        // otherwise read outside it.
        if entry.profile_url.contains("..") || entry.profile_url.starts_with('/') {
            return Err(Error::Input(format!(
                "registry entry '{}' has an unsafe profile_url '{}'",
                entry.name, entry.profile_url
            )));
        }
        let url = format!("{}/{}", self.base_url, entry.profile_url.trim_start_matches('/'));
        let body = fetch_text(&url, MAX_PROFILE_BYTES).await?;
        serde_json::from_str(&body).map_err(|e| {
            Error::Operation(format!("registry manifest at {url} is not valid JSON: {e}"))
        })
    }
}

/// Fetch `url` as text, bounded to `cap` bytes.
///
/// Handles `file://` (and bare paths) locally so a fixture registry, an internal
/// mirror on a network share, or an air-gapped copy need no server.
async fn fetch_text(url: &str, cap: u64) -> Result<String> {
    if let Some(path) = local_path(url) {
        return read_capped(&path, cap);
    }
    let bytes = fetch_bytes(url, cap).await?;
    String::from_utf8(bytes).map_err(|_| Error::Operation(format!("{url} is not valid UTF-8")))
}

/// Fetch `url` as bytes, bounded to `cap`.
///
/// Shared by the text path (index, manifests) and the grammar download, so the
/// HTTPS requirement, credential rejection, redirect policy and size cap are
/// enforced in exactly one place rather than drifting between the two.
///
/// A `file://` or bare path is read locally, which is what lets a fixture
/// registry serve a `.wasm` without a server.
async fn fetch_bytes(url: &str, cap: u64) -> Result<Vec<u8>> {
    if let Some(path) = local_path(url) {
        return read_capped_bytes(&path, cap);
    }

    // Plain HTTP would expose the profile to tampering in transit. Allow it only
    // for loopback, where it's the normal shape of a local test server.
    if url.starts_with("http://") && !is_loopback(url) {
        return Err(Error::Input(format!(
            "refusing to fetch over plain HTTP: {url} — use https:// (http:// is allowed only for \
             localhost)"
        )));
    }
    // Credentials in the URL would be sent to the host; nothing here needs them,
    // and allowing them invites `http://localhost@evil.com` style confusion.
    if let Ok(parsed) = reqwest::Url::parse(url)
        && (!parsed.username().is_empty() || parsed.password().is_some())
    {
        return Err(Error::Input(format!(
            "refusing to fetch a registry URL containing credentials: {url}"
        )));
    }

    let client = build_registry_http_client()?;
    tracing::debug!(target: "codemark::http", %url, "fetching from the grammar registry");
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("could not reach {url}: {e}")))?;

    if !response.status().is_success() {
        return Err(Error::Operation(format!("{url} returned HTTP {}", response.status())));
    }

    // Trust the advertised length only to reject early; the real bound is the
    // streamed count below, since Content-Length can lie or be absent.
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(Error::Operation(format!("{url} is too large ({len} bytes, limit {cap})")));
    }

    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = response;
    while let Some(chunk) =
        stream.chunk().await.map_err(|e| Error::Operation(format!("failed reading {url}: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > cap {
            return Err(Error::Operation(format!("{url} exceeds the {cap}-byte limit")));
        }
    }

    Ok(bytes)
}

/// Download a grammar and verify it against its expected digest.
///
/// The digest is checked **before** the bytes go anywhere near
/// [`install_grammar`](super::install_grammar), so a URL that has started
/// serving something else can't reach the grammar cache. The returned bytes are
/// the ones that hashed correctly, not a re-read — there is no second fetch to
/// disagree with the first.
pub async fn download_wasm(remote: &RemoteWasm) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};

    tracing::debug!(target: "codemark::http", url = %remote.url, "downloading grammar");
    let bytes = fetch_bytes(&remote.url, MAX_WASM_BYTES).await?;

    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != remote.sha256 {
        return Err(Error::Input(format!(
            "checksum mismatch for {}\n  expected {}\n  got      {actual}\nThe registry pins a \
             specific build; refusing to install different bytes.",
            remote.url, remote.sha256
        )));
    }

    tracing::debug!(
        target: "codemark::http",
        bytes = bytes.len(),
        "grammar download verified"
    );
    Ok(bytes)
}

/// Cap on a downloaded grammar (32 MiB), matching the local-file path in
/// [`super::source`].
const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

/// HTTP client for registry fetches.
///
/// Mirrors [`crate::sync::build_sync_http_client`]'s timeouts (and its
/// rustls-only TLS) but adds a redirect policy: reqwest follows up to 10
/// redirects by default, so an `https://` registry could 302 to plain `http://`
/// and slip past the scheme check in [`fetch_text`], which only ever sees the
/// *initial* URL. Each hop is re-checked here, and a downgrade to non-loopback
/// HTTP stops the chain.
fn build_registry_http_client() -> Result<reqwest::Client> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url();
        let downgraded = url.scheme() == "http" && !url.host_str().is_some_and(host_is_loopback);
        if downgraded {
            return attempt.error("registry redirected to plain HTTP");
        }
        attempt.follow()
    });

    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(policy)
        .build()
        .map_err(|e| Error::Operation(format!("failed to build HTTP client: {e}")))
}

/// A short, filesystem-safe key identifying a registry URL.
///
/// Only needs to separate one registry's cache from another's, so a
/// non-cryptographic hash is enough — this guards against staleness, not an
/// adversary. Spelled out (FNV-1a) rather than using `DefaultHasher`, whose
/// output is explicitly not stable across Rust releases: a changed hash would
/// silently orphan every cached index on upgrade.
fn url_key(base_url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in base_url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The filesystem path for a `file://` URL or a bare path, else `None`.
fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        // `file:///abs/path` → `/abs/path`; a host component isn't supported.
        return Some(PathBuf::from(rest));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return None;
    }
    Some(PathBuf::from(url))
}

/// Whether `url`'s **host** is loopback.
///
/// Parsed rather than string-scanned: splitting on the first `:` or `/` reads
/// the userinfo as the host, so `http://localhost:80@example.com` would look
/// like loopback while the request actually goes to `example.com` in the clear.
fn is_loopback(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    // Credentials are rejected separately in `fetch_text`; treating a URL that
    // carries them as non-loopback here keeps this function safe on its own.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    parsed.host_str().is_some_and(host_is_loopback)
}

/// Whether a bare host string names the loopback interface.
///
/// Shared by [`is_loopback`] and the redirect policy so the initial check and
/// each redirect hop can't drift apart.
fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `host_str` keeps IPv6 in bracketed form; strip them before parsing.
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    bare.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Read a local file, bounded to `cap` bytes and requiring a regular file —
/// mirroring the posture [`super::source`] uses for a local `.wasm`.
fn read_capped(path: &Path, cap: u64) -> Result<String> {
    let bytes = read_capped_bytes(path, cap)?;
    String::from_utf8(bytes)
        .map_err(|_| Error::Operation(format!("{} is not valid UTF-8", path.display())))
}

/// Byte-level counterpart to [`read_capped`], so a local fixture registry can
/// serve a `.wasm` as well as JSON.
fn read_capped_bytes(path: &Path, cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path)
        .map_err(|e| Error::Input(format!("could not read {}: {e}", path.display())))?;
    let meta = file
        .metadata()
        .map_err(|e| Error::Operation(format!("could not stat {}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::Input(format!("{} is not a regular file", path.display())));
    }
    if meta.len() > cap {
        return Err(Error::Input(format!(
            "{} is too large ({} bytes, limit {cap})",
            path.display(),
            meta.len()
        )));
    }

    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Operation(format!("could not read {}: {e}", path.display())))?;
    if bytes.len() as u64 > cap {
        return Err(Error::Input(format!("{} exceeds the {cap}-byte limit", path.display())));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a fixture registry on disk and return a client pointed at it.
    fn fixture_registry(tmp: &Path) -> RegistryClient {
        let grammars = tmp.join("registry").join("grammars").join("lua");
        std::fs::create_dir_all(&grammars).unwrap();
        std::fs::write(
            tmp.join("registry").join("index.json"),
            r#"{
              "schema_version": 1,
              "grammars": [
                {
                  "name": "lua",
                  "extensions": ["lua"],
                  "profile_version": "1.0.0",
                  "profile_url": "grammars/lua/manifest.json",
                  "ts_abi": "0.25",
                  "grammar_repo": "https://github.com/tree-sitter-grammars/tree-sitter-lua",
                  "grammar_rev": "v0.4.0",
                  "description": "Lua"
                }
              ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            grammars.join("manifest.json"),
            r#"{
              "name": "lua",
              "version": "0.3.0",
              "extensions": ["lua"],
              "profile": { "landmark_kinds": ["function_declaration"] }
            }"#,
        )
        .unwrap();
        RegistryClient::with_base_url(
            tmp.join("registry").display().to_string(),
            tmp.join("cache"),
            Duration::from_secs(3600),
        )
    }

    #[tokio::test]
    async fn fetches_an_index_and_finds_an_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let client = fixture_registry(tmp.path());

        let index = client.fetch_index(false).await.unwrap();
        assert_eq!(index.schema_version, 1);
        assert_eq!(index.grammars.len(), 1);

        let entry = client.find("LUA", false).await.unwrap();
        assert_eq!(entry.name, "lua");
        assert_eq!(entry.grammar_rev.as_deref(), Some("v0.4.0"));
    }

    #[tokio::test]
    async fn unknown_language_names_the_list_command() {
        let tmp = tempfile::tempdir().unwrap();
        let client = fixture_registry(tmp.path());
        let err = client.find("cobol", false).await.unwrap_err().to_string();
        assert!(err.contains("not in the grammar registry"), "{err}");
    }

    #[tokio::test]
    async fn caches_the_index_and_serves_it_when_the_source_disappears() {
        // The offline path: fetch once, remove the registry, and the cached
        // index must still answer rather than failing the command.
        let tmp = tempfile::tempdir().unwrap();
        let client = fixture_registry(tmp.path());
        client.fetch_index(false).await.unwrap();
        assert!(client.index_cache_path().exists(), "index should be cached");

        std::fs::remove_dir_all(tmp.path().join("registry")).unwrap();
        let index = client.fetch_index(false).await.unwrap();
        assert_eq!(index.grammars.len(), 1, "cached index should still serve");
    }

    #[tokio::test]
    async fn stale_cache_is_used_when_the_fetch_fails() {
        // TTL expired *and* the network is gone: a stale cache still beats no
        // registry at all.
        let tmp = tempfile::tempdir().unwrap();
        let client = RegistryClient::with_base_url(
            tmp.path().join("registry").display().to_string(),
            tmp.path().join("cache"),
            Duration::ZERO, // everything is immediately stale
        );
        let _ = fixture_registry(tmp.path());
        client.fetch_index(true).await.unwrap();

        std::fs::remove_dir_all(tmp.path().join("registry")).unwrap();
        let index = client.fetch_index(false).await.unwrap();
        assert_eq!(index.grammars.len(), 1);
    }

    #[tokio::test]
    async fn switching_registry_does_not_serve_the_previous_cache() {
        // Two registries, one cache directory. Before the cache was keyed by
        // URL, the second client would serve the first's entries for the rest
        // of the TTL — listing the wrong languages and joining the old
        // registry's profile paths onto the new base URL.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");

        let first = fixture_registry(tmp.path());
        first.fetch_index(false).await.unwrap();

        let other = tmp.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("index.json"),
            r#"{"schema_version":1,"grammars":[
               {"name":"ruby","extensions":["rb"],"profile_url":"grammars/ruby/manifest.json"}
            ]}"#,
        )
        .unwrap();
        let second = RegistryClient::with_base_url(
            other.display().to_string(),
            cache,
            Duration::from_secs(3600),
        );

        let index = second.fetch_index(false).await.unwrap();
        assert_eq!(index.grammars.len(), 1);
        assert_eq!(index.grammars[0].name, "ruby", "must not serve the other registry's cache");
        // Both caches coexist rather than overwriting each other.
        assert!(first.index_cache_path().exists());
        assert!(second.index_cache_path().exists());
        assert_ne!(first.index_cache_path(), second.index_cache_path());
    }

    #[tokio::test]
    async fn fetches_a_profile_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let client = fixture_registry(tmp.path());
        let entry = client.find("lua", false).await.unwrap();
        let manifest = client.fetch_profile(&entry).await.unwrap();
        assert_eq!(manifest["name"], "lua");
        assert_eq!(manifest["profile"]["landmark_kinds"][0], "function_declaration");
    }

    #[tokio::test]
    async fn rejects_a_profile_url_that_escapes_the_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let client = fixture_registry(tmp.path());
        let mut entry = client.find("lua", false).await.unwrap();
        entry.profile_url = "../../../etc/passwd".to_string();
        let err = client.fetch_profile(&entry).await.unwrap_err().to_string();
        assert!(err.contains("unsafe profile_url"), "{err}");

        entry.profile_url = "/etc/passwd".to_string();
        assert!(client.fetch_profile(&entry).await.is_err());
    }

    #[tokio::test]
    async fn rejects_an_index_from_a_newer_schema() {
        // A future wire format must fail loudly rather than be half-read.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("registry")).unwrap();
        std::fs::write(
            tmp.path().join("registry").join("index.json"),
            r#"{ "schema_version": 99, "grammars": [] }"#,
        )
        .unwrap();
        let client = RegistryClient::with_base_url(
            tmp.path().join("registry").display().to_string(),
            tmp.path().join("cache"),
            Duration::from_secs(60),
        );
        let err = client.fetch_index(false).await.unwrap_err().to_string();
        assert!(err.contains("upgrade codemark"), "{err}");
    }

    #[tokio::test]
    async fn refuses_plain_http_to_a_non_loopback_host() {
        // A profile fetched in the clear could be tampered with in transit.
        let err = fetch_text("http://example.com/index.json", MAX_INDEX_BYTES)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("plain HTTP"), "{err}");
    }

    #[test]
    fn loopback_detection_covers_the_usual_spellings() {
        assert!(is_loopback("http://localhost:8080/index.json"));
        assert!(is_loopback("http://127.0.0.1/index.json"));
        assert!(is_loopback("http://127.0.0.2/index.json")); // all of 127/8
        assert!(is_loopback("http://[::1]:9000/index.json"));
        assert!(!is_loopback("http://example.com/index.json"));
        // A host that merely starts with "localhost" is not loopback.
        assert!(!is_loopback("http://localhost.evil.com/index.json"));
    }

    #[test]
    fn userinfo_cannot_pose_as_a_loopback_host() {
        // The host here is example.com; `localhost:80` is userinfo. A scan that
        // stops at the first ':' or '/' reads it as the host and would send the
        // request to example.com in the clear.
        assert!(!is_loopback("http://localhost:80@example.com/index.json"));
        assert!(!is_loopback("http://localhost@evil.com/index.json"));
        assert!(!is_loopback("http://127.0.0.1@evil.com/index.json"));
    }

    #[tokio::test]
    async fn refuses_a_url_carrying_credentials() {
        // Even over https: nothing here needs credentials, and allowing them
        // invites exactly the confusion the test above describes.
        let err = fetch_text("https://user:pw@example.com/index.json", MAX_INDEX_BYTES)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("credentials"), "{err}");

        // And the loopback exception must not be reachable through userinfo.
        let err = fetch_text("http://localhost:80@example.com/index.json", MAX_INDEX_BYTES)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("plain HTTP"), "{err}");
    }

    /// Build an entry carrying a download, for the `download()` guard tests.
    fn entry_with_download(url: &str, sha: Option<&str>, repo: Option<&str>) -> RegistryEntry {
        RegistryEntry {
            name: "lua".into(),
            extensions: vec!["lua".into()],
            profile_version: None,
            profile_url: "grammars/lua/manifest.json".into(),
            ts_abi: None,
            grammar_repo: repo.map(String::from),
            grammar_rev: None,
            description: None,
            wasm_url: Some(url.into()),
            wasm_sha256: sha.map(String::from),
            overrides_builtin: false,
        }
    }

    const SHA_OK: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn entry_without_a_wasm_url_offers_no_download() {
        let mut e = entry_with_download("https://example.com/g.wasm", Some(SHA_OK), None);
        e.wasm_url = None;
        assert!(e.download().unwrap().is_none());
    }

    #[test]
    fn download_requires_a_checksum() {
        // Auto-download takes fetching off the user's desk, so unpinned bytes
        // must be refused rather than silently trusted.
        let e = entry_with_download("https://example.com/g.wasm", None, None);
        let err = e.download().unwrap_err().to_string();
        assert!(err.contains("no wasm_sha256"), "{err}");
    }

    #[test]
    fn download_rejects_a_malformed_checksum() {
        for bad in ["abc", &"z".repeat(64)] {
            let e = entry_with_download("https://example.com/g.wasm", Some(bad), None);
            assert!(e.download().is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn download_requires_https() {
        let e = entry_with_download("http://example.com/g.wasm", Some(SHA_OK), None);
        let err = e.download().unwrap_err().to_string();
        assert!(err.contains("https"), "{err}");
    }

    #[test]
    fn download_host_must_match_the_grammar_repo() {
        // A registry entry must not be able to point the download somewhere
        // unrelated to the repository it claims to package.
        let e = entry_with_download(
            "https://evil.example/g.wasm",
            Some(SHA_OK),
            Some("https://github.com/tree-sitter/tree-sitter-lua"),
        );
        let err = e.download().unwrap_err().to_string();
        assert!(err.contains("does not match grammar_repo host"), "{err}");

        // Same host is fine.
        let ok = entry_with_download(
            "https://github.com/tree-sitter/tree-sitter-lua/releases/download/v1/g.wasm",
            Some(SHA_OK),
            Some("https://github.com/tree-sitter/tree-sitter-lua"),
        );
        assert!(ok.download().unwrap().is_some());
    }

    #[tokio::test]
    async fn download_verifies_the_checksum() {
        use sha2::{Digest, Sha256};
        let tmp = tempfile::tempdir().unwrap();
        let wasm = tmp.path().join("g.wasm");
        std::fs::write(&wasm, b"\0asm\x01\x00\x00\x00").unwrap();
        let real = format!("{:x}", Sha256::digest(std::fs::read(&wasm).unwrap()));

        // Matching digest returns exactly the bytes that hashed.
        let good = RemoteWasm { url: wasm.display().to_string(), sha256: real.clone() };
        assert_eq!(download_wasm(&good).await.unwrap(), b"\0asm\x01\x00\x00\x00");

        // A digest that doesn't match must stop the install, not warn.
        let bad = RemoteWasm { url: wasm.display().to_string(), sha256: SHA_OK.into() };
        let err = download_wasm(&bad).await.unwrap_err().to_string();
        assert!(err.contains("checksum mismatch"), "{err}");
    }

    #[test]
    fn overrides_builtin_defaults_to_false() {
        // An entry must opt in. A collision in an entry that doesn't declare
        // the override is still an accident worth refusing.
        let e: RegistryEntry =
            serde_json::from_str(r#"{"name":"lua","extensions":["lua"],"profile_url":"p.json"}"#)
                .unwrap();
        assert!(!e.overrides_builtin);

        let e: RegistryEntry = serde_json::from_str(
            r#"{"name":"c_sharp","extensions":["cs"],"profile_url":"p.json","overrides_builtin":true}"#,
        )
        .unwrap();
        assert!(e.overrides_builtin);
    }

    /// A C#-shaped entry: declares the override, collides with a built-in.
    fn overriding_entry() -> RegistryEntry {
        RegistryEntry {
            name: "c_sharp".into(),
            extensions: vec!["cs".into()],
            profile_version: None,
            profile_url: "p.json".into(),
            ts_abi: None,
            grammar_repo: None,
            grammar_rev: None,
            description: None,
            wasm_url: None,
            wasm_sha256: None,
            overrides_builtin: true,
        }
    }

    #[test]
    fn entry_consent_covers_what_it_declares() {
        let e = overriding_entry();
        assert!(e.consents_to_override("c_sharp", &["cs".to_string()]));
        // Normalization differences in the index must still match.
        let mut dotted = overriding_entry();
        dotted.extensions = vec![".CS".into()];
        assert!(dotted.consents_to_override("c_sharp", &["cs".to_string()]));
    }

    #[test]
    fn entry_consent_stops_at_undeclared_values() {
        let e = overriding_entry();
        // An extension the entry never declared: `--extensions cs,rs` must not
        // inherit C#'s consent and take over every .rs file.
        assert!(!e.consents_to_override("c_sharp", &["cs".to_string(), "rs".to_string()]));
        // Nor a name it never declared.
        assert!(!e.consents_to_override("rust", &["cs".to_string()]));
        // And an entry that declares nothing consents to nothing.
        let mut plain = overriding_entry();
        plain.overrides_builtin = false;
        assert!(!plain.consents_to_override("c_sharp", &["cs".to_string()]));
    }

    /// Drive the real install-path decision (`validate_registry_install`) for
    /// the four cases that matter. Asserting through the function the install
    /// actually calls means dropping either check here turns red.
    #[test]
    fn registry_install_honors_scoped_consent() {
        use crate::grammar::validate_registry_install;

        let declaring = overriding_entry();
        let mut silent = overriding_entry();
        silent.overrides_builtin = false;

        // 1. Entry consent, no flag: the C# install that was failing now works.
        let exts = validate_registry_install(&declaring, "c_sharp", "cs", false).unwrap();
        assert_eq!(exts, vec!["cs".to_string()]);

        // 2. Collision with no consent anywhere: still refused.
        let err = validate_registry_install(&silent, "c_sharp", "cs", false).unwrap_err();
        assert!(err.to_string().contains("built-in"), "{err}");

        // 3. Consent doesn't stretch to an undeclared extension.
        let err = validate_registry_install(&declaring, "c_sharp", "cs,rs", false).unwrap_err();
        assert!(err.to_string().contains("built-in"), "{err}");

        // 4. The caller's explicit flag still wins over everything.
        let exts = validate_registry_install(&silent, "c_sharp", "cs,rs", true).unwrap();
        assert_eq!(exts, vec!["cs".to_string(), "rs".to_string()]);
    }

    #[test]
    fn builtin_collision_detection_reads_name_and_extensions() {
        assert!(overriding_entry().collides_with_builtin()); // c_sharp / cs
        let mut free = overriding_entry();
        free.name = "lua".into();
        free.extensions = vec!["lua".into()];
        assert!(!free.collides_with_builtin());
        // Either half colliding is enough.
        let mut by_ext = free.clone();
        by_ext.extensions = vec!["rs".into()];
        assert!(by_ext.collides_with_builtin());
    }

    #[tokio::test]
    async fn an_upgrade_refetches_a_cache_written_before_overrides_builtin() {
        // A cache from an older codemark has no `overrides_builtin` key, so the
        // entry reads as `false` and the no-flag C# install would fail for a
        // whole TTL. The install lookup must re-read the index to find the
        // declaration the registry has been serving all along.
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&reg).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        // The live registry declares the override.
        std::fs::write(
            reg.join("index.json"),
            r#"{"schema_version":1,"grammars":[
               {"name":"c_sharp","extensions":["cs"],"profile_url":"p.json",
                "overrides_builtin":true}
            ]}"#,
        )
        .unwrap();

        let client = RegistryClient::with_base_url(
            reg.display().to_string(),
            cache.clone(),
            Duration::from_secs(86400),
        );

        // Plant a fresh pre-upgrade cache: same entry, field absent.
        std::fs::write(
            client.index_cache_path(),
            r#"{"schema_version":1,"grammars":[
               {"name":"c_sharp","extensions":["cs"],"profile_url":"p.json"}
            ]}"#,
        )
        .unwrap();

        // Plain `find` honors the (stale) cache and sees no declaration.
        assert!(!client.find("c_sharp", false).await.unwrap().overrides_builtin);

        // The install lookup notices the collision and refetches.
        let entry = client.find_for_install("c_sharp", false).await.unwrap();
        assert!(entry.overrides_builtin, "install lookup should re-read the index");
        assert!(crate::grammar::validate_registry_install(&entry, "c_sharp", "cs", false).is_ok());
    }

    #[tokio::test]
    async fn a_refresh_that_delists_the_entry_does_not_resurrect_it() {
        // The refetch must not collapse "registry dropped this" into "network
        // was down". A delisted entry kept from cache would still install under
        // --override-builtin, resurrecting what the registry removed.
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&reg).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        // Reachable registry that no longer lists c_sharp.
        std::fs::write(
            reg.join("index.json"),
            r#"{"schema_version":1,"grammars":[
               {"name":"lua","extensions":["lua"],"profile_url":"p.json"}
            ]}"#,
        )
        .unwrap();

        let client = RegistryClient::with_base_url(
            reg.display().to_string(),
            cache,
            Duration::from_secs(86400),
        );
        // Cached entry collides with a built-in and declares nothing, so the
        // refetch path is the one under test.
        std::fs::write(
            client.index_cache_path(),
            r#"{"schema_version":1,"grammars":[
               {"name":"c_sharp","extensions":["cs"],"profile_url":"p.json"}
            ]}"#,
        )
        .unwrap();

        let err = client.find_for_install("c_sharp", false).await.unwrap_err();
        assert!(err.to_string().contains("not in the grammar registry"), "{err}");
    }

    #[tokio::test]
    async fn an_unreachable_registry_still_falls_back_to_the_cached_entry() {
        // The offline guarantee the fallback exists for: no index.json to
        // fetch, so the cached entry must survive and let validation report the
        // collision as it always would.
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&reg).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        let client = RegistryClient::with_base_url(
            reg.display().to_string(),
            cache,
            Duration::from_secs(86400),
        );
        std::fs::write(
            client.index_cache_path(),
            r#"{"schema_version":1,"grammars":[
               {"name":"c_sharp","extensions":["cs"],"profile_url":"p.json"}
            ]}"#,
        )
        .unwrap();

        let entry = client.find_for_install("c_sharp", false).await.unwrap();
        assert_eq!(entry.name, "c_sharp");
        assert!(!entry.overrides_builtin);
        // And it is still refused without the flag.
        let err =
            crate::grammar::validate_registry_install(&entry, "c_sharp", "cs", false).unwrap_err();
        assert!(err.to_string().contains("built-in"), "{err}");
    }

    #[tokio::test]
    async fn a_non_colliding_entry_is_served_from_cache_without_a_refetch() {
        // The refetch must be scoped to the collision case, or every install
        // would pay a network round trip and the cache would stop mattering.
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(&reg).unwrap();
        std::fs::create_dir_all(&cache).unwrap();

        let client = RegistryClient::with_base_url(
            reg.display().to_string(),
            cache,
            Duration::from_secs(86400),
        );

        // Source and cache disagree on a field, so the value that comes back
        // says which one was read. A reachable source is the point: if the
        // fetch merely *failed*, the stale-cache fallback would serve the
        // cached entry too and this test would pass without proving anything.
        std::fs::write(
            reg.join("index.json"),
            r#"{"schema_version":1,"grammars":[
               {"name":"lua","extensions":["lua"],"profile_url":"from-source.json"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            client.index_cache_path(),
            r#"{"schema_version":1,"grammars":[
               {"name":"lua","extensions":["lua"],"profile_url":"from-cache.json"}
            ]}"#,
        )
        .unwrap();

        let entry = client.find_for_install("lua", false).await.unwrap();
        assert_eq!(
            entry.profile_url, "from-cache.json",
            "a non-colliding entry must not trigger a refetch"
        );
    }

    #[tokio::test]
    async fn registry_entries_cannot_shadow_a_builtin() {
        // A registry entry gets no more trust than a hand-typed --name: it must
        // not be able to claim `rs` (or any built-in alias/extension), which
        // would install a grammar that could never resolve.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("registry")).unwrap();
        std::fs::write(
            tmp.path().join("registry").join("index.json"),
            r#"{"schema_version":1,"grammars":[
               {"name":"rust","extensions":["rs"],"profile_url":"p.json"}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("registry").join("p.json"),
            r#"{"name":"rust","extensions":["rs"],"profile":{}}"#,
        )
        .unwrap();

        let client = RegistryClient::with_base_url(
            tmp.path().join("registry").display().to_string(),
            tmp.path().join("cache"),
            Duration::from_secs(60),
        );
        let entry = client.find("rust", false).await.unwrap();
        let manifest = client.fetch_profile(&entry).await.unwrap();

        // The install path runs the same validation as `add`, which is what
        // rejects it — assert against that function directly so the guarantee
        // is pinned without needing a real .wasm or the global cache.
        let name = manifest["name"].as_str().unwrap();
        let exts = entry.extensions.join(",");
        let err = crate::grammar::validate_name_and_extensions(name, &exts).unwrap_err();
        assert!(err.to_string().contains("built-in"), "{err}");
    }

    #[test]
    fn caps_an_oversized_local_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.json");
        std::fs::write(&path, "x".repeat(2048)).unwrap();
        assert!(read_capped(&path, 1024).is_err());
        assert!(read_capped(&path, 4096).is_ok());
    }
}
