#!/bin/bash
set -euo pipefail

# Bump the version across every workspace crate and keep Cargo.lock in sync.
#
# Each crate under crates/* carries its own `version = "x.y.z"` in [package]
# (versions are not inherited from [workspace.package]), so a release means
# editing all of them to the same number and refreshing Cargo.lock.
#
# Usage:
#   ./scripts/bump-version.sh <version>          # edit files + sync lock
#   ./scripts/bump-version.sh <version> --commit # also create the bump commit
#
# Example:
#   ./scripts/bump-version.sh 0.7.17 --commit

VERSION=${1:?Version required (e.g. 0.7.17)}
COMMIT=false
if [ "${2:-}" = "--commit" ]; then
  COMMIT=true
fi

# Reject anything that isn't a plain semver-ish "x.y.z" so we don't silently
# write garbage into Cargo.toml.
if ! [[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '${VERSION}' is not a valid version (expected x.y.z)" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

# Collect the per-crate manifests from the workspace `members` list rather than
# globbing, so new/removed members are picked up automatically. Fall back to a
# glob if the parse turns up nothing.
# (read loop instead of mapfile so this works on macOS's bash 3.2)
MANIFESTS=()
while IFS= read -r member; do
  MANIFESTS+=("${member}/Cargo.toml")
done < <(
  awk '
    /^\[workspace\]/      { in_ws = 1; next }
    /^\[/                 { in_ws = 0 }
    in_ws && /members[[:space:]]*=/ { in_members = 1 }
    in_members {
      while (match($0, /"[^"]+"/)) {
        m = substr($0, RSTART + 1, RLENGTH - 2)
        print m
        $0 = substr($0, RSTART + RLENGTH)
      }
      if ($0 ~ /\]/) in_members = 0
    }
  ' Cargo.toml
)

if [ "${#MANIFESTS[@]}" -eq 0 ]; then
  MANIFESTS=(crates/*/Cargo.toml)
fi

echo "Setting version ${VERSION} in:"
for manifest in "${MANIFESTS[@]}"; do
  if [ ! -f "${manifest}" ]; then
    echo "error: manifest not found: ${manifest}" >&2
    exit 1
  fi

  # Replace only the `version = "..."` line inside the [package] table, so we
  # never touch dependency version requirements elsewhere in the file.
  awk -v ver="${VERSION}" '
    /^\[/                 { in_pkg = ($0 ~ /^\[package\]/) }
    in_pkg && /^[[:space:]]*version[[:space:]]*=/ && !done {
      sub(/version[[:space:]]*=.*/, "version = \"" ver "\"")
      done = 1
    }
    { print }
  ' "${manifest}" > "${manifest}.tmp"
  mv "${manifest}.tmp" "${manifest}"

  echo "  ${manifest}"
done

# Refresh Cargo.lock for the workspace members. --offline keeps this from
# hitting the network; the version change only touches local crate entries.
echo "Syncing Cargo.lock..."
cargo update --workspace --offline

if [ "${COMMIT}" = true ]; then
  git add "${MANIFESTS[@]}" Cargo.lock
  git commit -m "Bump version to ${VERSION}"
  echo "Committed: Bump version to ${VERSION}"
else
  echo "Done. Review with 'git diff' (use --commit to auto-commit)."
fi
