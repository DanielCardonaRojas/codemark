#!/bin/bash
set -euo pipefail

# Manual fallback for updating the Homebrew formula after a release.
#
# The `codemark` formula ships PRECOMPILED binaries for both `codemark`
# (codemark-cli) and `codemark-tui` (codemark-tui), rendered from the archives
# dist uploads to the GitHub release. CI normally does this automatically (the
# `publish-homebrew-formula` job in .github/workflows/release.yml); use this
# script only if you need to render/push the formula by hand.
#
# Run it AFTER the release's archives have been uploaded.
#
# Usage:   ./release.sh <version>
# Env:     TAP_DIR  override the tap checkout (default: ../homebrew-codemark)

VERSION=${1:?Version required}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAP_DIR=${TAP_DIR:-"${SCRIPT_DIR}/../homebrew-codemark"}
FORMULA="${TAP_DIR}/Formula/codemark.rb"

if [ ! -d "${TAP_DIR}" ]; then
  echo "error: tap repo not found at ${TAP_DIR} (set TAP_DIR to override)" >&2
  exit 1
fi

"${SCRIPT_DIR}/scripts/render-homebrew-formula.sh" "${VERSION}" "${FORMULA}"

echo ""
echo "Review the changes and push:"
echo "  cd ${TAP_DIR}"
echo "  git add Formula/codemark.rb"
echo "  git commit -m 'Update codemark to ${VERSION}'"
echo "  git push"
