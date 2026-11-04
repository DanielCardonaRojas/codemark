//! Helpers for tests, here and in the other workspace crates.
//!
//! Not part of the public API — hidden from docs and subject to change.

use crate::parser::languages::Language;

/// Load a registry grammar by name for a test.
///
/// Tests parse with the same WASM grammars users install, from the pinned cache
/// `.cargo/config.toml` points `CODEMARK_GRAMMARS_DIR` at. Panics with the fix
/// when that cache has not been populated.
pub fn grammar(name: &str) -> Language {
    name.parse().unwrap_or_else(|e| {
        panic!(
            "the {name} grammar is not available to tests ({e}).\n\
             Run scripts/fetch-test-grammars.sh to populate tests/grammars/."
        )
    })
}
