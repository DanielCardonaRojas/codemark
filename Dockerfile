# Stage 1: Build
FROM rust:1.88-bookworm AS builder

WORKDIR /build

# Copy manifests first to cache dependency downloads
COPY Cargo.toml Cargo.lock ./
COPY crates/codemark-core/Cargo.toml crates/codemark-core/Cargo.toml
COPY crates/codetours-server/Cargo.toml crates/codetours-server/Cargo.toml

# Create dummy source files so cargo can resolve the workspace and fetch deps
RUN mkdir -p crates/codemark-core/src && echo "" > crates/codemark-core/src/lib.rs \
    && mkdir -p crates/codetours-server/src && echo "fn main() {}" > crates/codetours-server/src/main.rs

# Stub out workspace members we don't need (cli, tui) so cargo doesn't fail
RUN mkdir -p crates/codemark-cli/src && echo "fn main() {}" > crates/codemark-cli/src/main.rs
COPY crates/codemark-cli/Cargo.toml crates/codemark-cli/Cargo.toml
RUN mkdir -p crates/codemark-tui/src && echo "fn main() {}" > crates/codemark-tui/src/main.rs
COPY crates/codemark-tui/Cargo.toml crates/codemark-tui/Cargo.toml

# Pre-build dependencies (this layer is cached as long as Cargo.toml/lock don't change)
RUN cargo build --release -p codetours-server 2>&1 || true

# Copy actual source code and files referenced by include_str! in codemark-core
COPY config config
COPY migrations migrations
COPY registry_migrations registry_migrations
COPY templates templates
COPY crates/codemark-core crates/codemark-core
COPY crates/codetours-server crates/codetours-server

# Touch source files to invalidate the cached dummy build
RUN touch crates/codemark-core/src/lib.rs crates/codetours-server/src/main.rs

# Build the real binary
RUN cargo build --release -p codetours-server

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libgit2-1.5 \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system codetours && useradd --system --gid codetours codetours

COPY --from=builder /build/target/release/codetours /usr/local/bin/codetours

RUN mkdir -p /data && chown codetours:codetours /data

USER codetours

EXPOSE 8080

ENTRYPOINT ["codetours"]
CMD ["--json-logs"]
