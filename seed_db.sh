#!/bin/bash

# Seed script for codemark using fixtures
# Usage: ./seed_db.sh

FIXTURES_DIR="./tests/fixtures"
COLLECTION="demo-collection"

# Ensure codemark is in path or set the command
CODEMARK_CMD="codemark"

echo "Seeding codemark database..."

# Helper to add files
add_bookmark() {
    local file=$1
    local collection=$2
    local range="1-10"

    echo "Adding $file to $collection..."
    $CODEMARK_CMD add --file "$file" --collection "$collection" --range "$range" --note "Fixtured bookmark from $file"
}

# Recursively add files from fixtures (excluding README.md)
find "$FIXTURES_DIR" -type f ! -name "README.md" | while read -r file; do
    add_bookmark "$file" "$COLLECTION"
done

echo "Seeding complete."
