//! The on-disk install-swap protocol — the single source of truth for the
//! filenames and lock semantics that the *writer* ([`super::install`]) and the
//! *recoverer* ([`crate::parser::registry`]) must agree on.
//!
//! Installing a grammar is a move-aside / rename-in / drop-backup sequence that
//! isn't atomic across a crash. To let a later run recover an interrupted swap,
//! the writer and the recovery-on-discovery pass share a fixed naming scheme
//! (`.staging-<name>-<pid>`, `.bak-<name>`, `.lock-<name>`) and one definition of
//! when a lock is stale. Defining them here — rather than hardcoding the strings
//! on each side — makes it impossible for the two halves to silently drift.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a `.lock-<name>` install lock is honored before it's treated as
/// orphaned (from a killed installer) and reaped. Shared by the installer (which
/// holds the lock) and registry recovery (which must not wait forever on a dead
/// one), so the two can't disagree on staleness.
pub const INSTALL_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Filename prefix for the deterministic move-aside backup of an install being
/// replaced. Discovery scans for these to recover an interrupted swap.
pub const BACKUP_PREFIX: &str = ".bak-";

/// Recover the grammar `name` from a `.bak-<name>` entry, if `file_name` is one.
/// Returns the name so the recoverer doesn't re-derive the prefix.
pub fn backup_name(file_name: &str) -> Option<&str> {
    file_name.strip_prefix(BACKUP_PREFIX)
}

/// Per-name exclusive-install lock path: `<cache>/.lock-<name>`.
///
/// Held across the swap so two concurrent same-name installs can't interleave
/// and clobber the shared deterministic backup.
pub fn lock_path(grammar_dir: &Path, safe_name: &str) -> PathBuf {
    grammar_dir.join(format!(".lock-{safe_name}"))
}

/// Deterministic backup path for an install being replaced: `<cache>/.bak-<name>`.
///
/// Built by appending to the *full* directory name (not `with_extension`, which
/// rewrites the last dot-segment and would collide `my.lang` with `my`). It is
/// **not** pid-suffixed on purpose: if the process dies mid-swap leaving
/// `<cache>/<name>` absent, the next run must find and restore this exact path.
pub fn backup_path(grammar_dir: &Path, safe_name: &str) -> PathBuf {
    grammar_dir.join(format!(".bak-{safe_name}"))
}

/// Unique staging path for a name's in-progress install:
/// `<cache>/.staging-<name>-<pid>`. Hidden and pid-suffixed so it's removed on
/// failure and can never collide with a real grammar name or a concurrent
/// installer's staging dir.
pub fn staging_path(grammar_dir: &Path, safe_name: &str, pid: u32) -> PathBuf {
    grammar_dir.join(format!(".staging-{safe_name}-{pid}"))
}

/// Whether a lock file at `lock_path` is stale — i.e. older than
/// [`INSTALL_LOCK_TIMEOUT`], so its installer is presumed dead and the lock may
/// be reaped. A lock whose mtime can't be read is treated as fresh (`false`) so
/// we never reap a live installer we simply couldn't stat.
pub fn lock_is_stale(lock_path: &Path) -> bool {
    lock_is_stale_after(lock_path, INSTALL_LOCK_TIMEOUT)
}

/// [`lock_is_stale`] with an explicit `timeout`, so recovery tests can force a
/// lock to read as orphaned (`Duration::ZERO`) without waiting out the real
/// [`INSTALL_LOCK_TIMEOUT`]. Production callers use [`lock_is_stale`].
pub fn lock_is_stale_after(lock_path: &Path, timeout: Duration) -> bool {
    std::fs::metadata(lock_path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or_default() > timeout)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_path_and_backup_name_are_inverses() {
        // The writer builds `.bak-<name>`; the recoverer strips it back to
        // `<name>`. This round-trip is the whole point of centralizing the
        // scheme — if either side drifted, a real interrupted swap wouldn't be
        // recovered. Includes a dotted name to guard the `with_extension`
        // collision the prefix scheme deliberately avoids.
        let dir = Path::new("/cache");
        for name in ["lua", "my.lang", "tree-sitter-bash"] {
            let backup = backup_path(dir, name);
            let file_name = backup.file_name().unwrap().to_string_lossy();
            assert_eq!(backup_name(&file_name), Some(name), "round-trip for {name}");
        }
    }

    #[test]
    fn backup_name_ignores_non_backup_entries() {
        // A real grammar dir or a staging/lock entry must not be seen as a backup.
        assert_eq!(backup_name("lua"), None);
        assert_eq!(backup_name(".staging-lua-123"), None);
        assert_eq!(backup_name(".lock-lua"), None);
    }
}
