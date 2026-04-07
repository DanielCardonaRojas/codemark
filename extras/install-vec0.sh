#!/bin/bash
# Install sqlite-vec extension for manual database inspection.
# This script downloads and installs the vec0.dylib for macOS (arm64).

set -e

VERSION="v0.1.9"
REPO="asg017/sqlite-vec"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

if [[ "$OS" == "Darwin" ]]; then
    if [[ "$ARCH" == "arm64" ]]; then
        FILE="vec0-osx-arm64.dylib"
    else
        FILE="vec0-osx-x86_64.dylib"
    fi
    SQLITE_LIB_DIR="/opt/homebrew/Cellar/sqlite"
    if [[ "$ARCH" == "x86_64" ]]; then
        SQLITE_LIB_DIR="/usr/local/opt/sqlite"
    fi
elif [[ "$OS" == "Linux" ]]; then
    if [[ "$ARCH" == "aarch64" ]]; then
        FILE="vec0-linux-aarch64.so"
    else
        FILE="vec0-linux-x86_64.so"
    fi
    SQLITE_LIB_DIR="/usr/local/lib"
else
    echo "Unsupported OS: $OS"
    exit 1
fi

# Find latest sqlite version in homebrew
if [[ -d "$SQLITE_LIB_DIR" ]]; then
    SQLITE_VERSION=$(ls -t "$SQLITE_LIB_DIR" | head -1)
    TARGET_DIR="$SQLITE_LIB_DIR/$SQLITE_VERSION/lib"
else
    echo "Warning: sqlite not found in $SQLITE_LIB_DIR"
    echo "Installing to current directory instead"
    TARGET_DIR="."
fi

echo "Downloading $FILE..."
curl -fsSL "${BASE_URL}/${FILE}" -o "/tmp/vec0.dylib"

echo "Installing to $TARGET_DIR/..."
cp "/tmp/vec0.dylib" "$TARGET_DIR/vec0.dylib"
rm "/tmp/vec0.dylib"

echo ""
echo "Installed vec0 extension to $TARGET_DIR/vec0.dylib"
echo ""
echo "Usage with sqlite3:"
echo "  $ /opt/homebrew/opt/sqlite/bin/sqlite3 .codemark/codemark.db"
echo "  sqlite> .load $TARGET_DIR/vec0"
echo "  sqlite> SELECT bookmark_id FROM bookmark_embeddings;"
