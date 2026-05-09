#!/bin/bash

# Explicit seed script for codemark
# Usage: ./seed_db.sh

CODEMARK_CMD="codemark"

echo "Seeding codemark database..."

# --- Collections ---
$CODEMARK_CMD collection create "rust-patterns" --description "Rust programming patterns and examples"
$CODEMARK_CMD collection create "python-patterns" --description "Python programming patterns and examples"

# --- Rust Seed Data ---
echo "Seeding rust-patterns..."
$CODEMARK_CMD add --file "tests/fixtures/rust/auth_service.rs" --collection "rust-patterns" \
    --range "6-10" \
    --note "Claims struct for authentication" \
    --note "Defines user identity and permissions" \
    --tag "rust" --tag "auth" --tag "struct"

$CODEMARK_CMD add --file "tests/fixtures/rust/auth_service.rs" --collection "rust-patterns" \
    --range "13-20" \
    --note "AuthError enum for error handling" \
    --tag "rust" --tag "error-handling" --tag "enum"

# Single line target
$CODEMARK_CMD add --file "tests/fixtures/rust/auth_service.rs" --collection "rust-patterns" \
    --range "36" \
    --note "AuthService constructor entry point" \
    --tag "rust" --tag "constructor"

$CODEMARK_CMD add --file "tests/fixtures/rust/auth_service.rs" --collection "rust-patterns" \
    --range "79-88" \
    --note "Check permission logic" \
    --tag "rust" --tag "security"

# --- Python Seed Data ---
echo "Seeding python-patterns..."
$CODEMARK_CMD add --file "tests/fixtures/python/auth_service.py" --collection "python-patterns" \
    --range "20-24" \
    --note "Claims dataclass definition" \
    --tag "python" --tag "dataclass" --tag "model"

$CODEMARK_CMD add --file "tests/fixtures/python/auth_service.py" --collection "python-patterns" \
    --range "36-47" \
    --note "Validate token core method" \
    --note "Uses token cache for performance" \
    --tag "python" --tag "auth" --tag "performance"

# Single line target
$CODEMARK_CMD add --file "tests/fixtures/python/auth_service.py" --collection "python-patterns" \
    --range "86" \
    --note "Get user profile function definition" \
    --tag "python" --tag "endpoint"

$CODEMARK_CMD add --file "tests/fixtures/python/auth_service.py" --collection "python-patterns" \
    --range "73-82" \
    --note "Auth decorator for function wrapping" \
    --tag "python" --tag "decorator"

echo "Seeding complete."
