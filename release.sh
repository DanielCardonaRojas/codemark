#!/bin/bash
set -euo pipefail

# Helper script to prepare a Homebrew formula update for a new release
# Usage: ./release.sh <version>

VERSION=${1:?Version required (without v prefix)}
TAG="v${VERSION}"
ARCHIVE_URL="https://github.com/DanielCardonaRojas/codemark/archive/refs/tags/${TAG}.tar.gz"
ARCHIVE_NAME="${TAG}.tar.gz"

echo "Fetching ${ARCHIVE_URL}..."
curl -L -o "${ARCHIVE_NAME}" "${ARCHIVE_URL}"

echo "Calculating SHA256..."
SHA256=$(shasum -a 256 "${ARCHIVE_NAME}" | cut -d' ' -f1)

echo "================================"
echo "Version: ${VERSION}"
echo "SHA256:  ${SHA256}"
echo "================================"
echo ""
echo "To update the Homebrew formula, run:"
echo "  cd ../homebrew-codemark && ./update.sh ${VERSION} ${SHA256}"
echo ""
echo "Then commit and push:"
echo "  git add Formula/codemark.rb"
echo "  git commit -m 'Update codemark to ${VERSION}'"
echo "  git push"
