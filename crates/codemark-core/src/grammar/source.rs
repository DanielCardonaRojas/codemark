//! Resolving a grammar from some *source* into installable bytes + metadata.
//!
//! Everything above the [`install_grammar`](super::install_grammar) seam is
//! "turn a user spec into a [`ResolvedGrammar`]", hidden behind the
//! [`GrammarSource`] trait. Today the only source is [`LocalFileSource`] — a
//! compiled `.wasm` the user built and pointed us at (`codemark languages add`).
//! The trait is kept as the seam a future grammar *registry* source would slot
//! into without disturbing the hardened install path.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Cap on a local `grammar.wasm` (32 MiB). Real grammars are well under this; a
/// larger file means a wrong/corrupt artifact.
const MAX_WASM_BYTES: u64 = 32 * 1024 * 1024;

/// A grammar resolved from some source, ready to install. Source-agnostic: a
/// local file today (a registry later) produces this same shape, so the install
/// pipeline is written once.
#[derive(Debug)]
pub struct ResolvedGrammar {
    /// The language name to install under (may be overridden by the caller).
    pub name: Option<String>,
    /// Raw, comma-separated extensions as the source reported them (may be
    /// overridden by the caller). Normalized later by
    /// [`validate_name_and_extensions`](super::validate_name_and_extensions).
    pub raw_extensions: Option<String>,
    /// Curated structural profile to write into the manifest. `None` means an
    /// empty profile (local files); a future registry could supply a real one.
    pub profile: Option<serde_json::Value>,
    /// The grammar's `grammar.wasm` bytes.
    pub wasm: WasmPayload,
}

/// Where a resolved grammar's `grammar.wasm` bytes come from. Currently always
/// [`WasmPayload::Bytes`] (a local file read into memory); kept as an enum so a
/// future registry source can add a deferred/remote variant without reshaping
/// [`ResolvedGrammar`].
#[derive(Debug)]
pub enum WasmPayload {
    /// Already in hand (a local `add`).
    Bytes(Vec<u8>),
}

impl WasmPayload {
    /// Materialize the bytes.
    pub async fn into_bytes(self) -> Result<Vec<u8>> {
        match self {
            WasmPayload::Bytes(b) => Ok(b),
        }
    }
}

/// Options that steer a [`GrammarSource::resolve`] beyond the bare spec. Empty
/// for the local-file source; kept as the extension point a registry source
/// (name/version disambiguation) would grow into.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResolveOptions<'a> {
    /// The caller's `--name` override. Unused by the local-file source (which
    /// requires an explicit name anyway); reserved for sources that resolve a
    /// name themselves and may need to disambiguate.
    pub requested_name: Option<&'a str>,
}

/// A place codemark can resolve a grammar from. One `resolve` per source kind;
/// the install pipeline treats them uniformly.
#[async_trait::async_trait]
pub trait GrammarSource {
    /// Resolve a user `spec` into installable metadata + wasm bytes.
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
/// pointed us at (`codemark languages add /dev/zero`) and block or fill memory.
/// Requiring a regular file rejects that; reading through a `take(cap + 1)`
/// reader bounds memory and catches a file that grows past the cap between the
/// stat and EOF.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
