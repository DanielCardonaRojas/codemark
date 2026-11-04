#!/usr/bin/env bash
# Populate the grammar cache the test suite runs against.
#
# Codemark no longer compiles most grammars into the binary: they are loaded as
# WASM at runtime from the grammar registry. The tests exercise that same path,
# so they need a grammar cache. This script fills `tests/grammars/` by running
# codemark's own `languages install` against a *pinned* registry commit, so
# every developer and CI run parses with byte-identical grammars (each .wasm is
# verified against the sha256 the pinned index records).
#
# `.cargo/config.toml` points `CODEMARK_GRAMMARS_DIR` at that directory for
# every `cargo test` / `cargo run` in this repo.
#
# Usage: scripts/fetch-test-grammars.sh [--force]
#   --force   reinstall even if a grammar is already present
#
# To move to newer grammars, bump REGISTRY_REV, re-run with --force, and review
# the snapshot diffs: a changed snapshot is a bookmark that would stop matching
# for users upgrading to the new grammar.
set -euo pipefail

REGISTRY_REV="${CODEMARK_TEST_REGISTRY_REV:-334e54f0a307f16587cd17b462f6bb6d9de74bff}"
REGISTRY_URL="https://raw.githubusercontent.com/DanielCardonaRojas/codemark-grammars/${REGISTRY_REV}"

# Languages the test suite parses. Dart is still compiled in and needs nothing.
LANGUAGES=(rust swift typescript tsx python go java c_sharp)

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${ROOT}/tests/grammars"
FORCE=0
[[ "${1:-}" == "--force" ]] && FORCE=1

mkdir -p "$DEST"

missing=()
for lang in "${LANGUAGES[@]}"; do
  if [[ $FORCE -eq 1 || ! -f "$DEST/$lang/grammar.wasm" || ! -f "$DEST/$lang/manifest.json" ]]; then
    missing+=("$lang")
  fi
done

if [[ ${#missing[@]} -eq 0 ]]; then
  echo "test grammars up to date in $DEST"
  exit 0
fi

# Build once, then call the binary directly — `cargo run` per language would
# re-check the build each time.
cargo build --quiet -p codemark-cli --manifest-path "$ROOT/Cargo.toml"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/codemark"

for lang in "${missing[@]}"; do
  echo "installing $lang (registry @ ${REGISTRY_REV:0:10})"
  CODEMARK_GRAMMARS_DIR="$DEST" \
    CODEMARK_GRAMMAR_REGISTRY="$REGISTRY_URL" \
    "$BIN" languages install "$lang" --refresh >/dev/null
done

echo "test grammars installed in $DEST"
