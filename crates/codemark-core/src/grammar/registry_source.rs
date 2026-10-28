//! Fetching curated profiles from the grammar registry.
//!
//! The registry ships **profiles, not binaries**: an `index.json` of what is
//! available plus a `manifest.json` per language. The user still builds the
//! `.wasm` themselves, which keeps codemark out of the ABI-pipeline and
//! grammar-licensing business while removing the part that actually hurts —
//! hand-authoring a profile.
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
    /// fields (a future `wasm_url`) are additive and don't bump it.
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

    String::from_utf8(bytes).map_err(|_| Error::Operation(format!("{url} is not valid UTF-8")))
}

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

    let mut text = String::new();
    file.take(cap + 1)
        .read_to_string(&mut text)
        .map_err(|e| Error::Operation(format!("could not read {}: {e}", path.display())))?;
    if text.len() as u64 > cap {
        return Err(Error::Input(format!("{} exceeds the {cap}-byte limit", path.display())));
    }
    Ok(text)
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
