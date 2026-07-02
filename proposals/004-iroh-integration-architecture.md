# Architecture: Crate Structure for the P2P Feature

This document answers: *where should the Iroh code live?* It is the structural
foundation the MVP plan (`002`) assumes.

> **Status: implemented.** The `codemark-p2p` crate exists, the CLI wires it
> behind the `p2p` feature, and CI enforces the guardrails below. The API sketch
> at the bottom reflects the shipped signatures.

## Current workspace
```
crates/
  codemark-core/     shared base — Tour types, serialization, AST resolution, DB.
                     No internal deps.
  codemark-cli/      depends on codemark-core. Already #[tokio::main].
  codemark-tui/      depends on codemark-core.
  codetours-server/  depends on codemark-core. The existing registry backend.
```
Every crate depends on `codemark-core`; `codemark-core` depends on nothing
internal. Lockfile is **`ring`-only, zero `aws-lc`**.

## Recommendation: a new leaf crate `codemark-p2p`
```
crates/
  codemark-core/     unchanged.
  codemark-p2p/      NEW. All iroh deps. ring-pinned. Tour-agnostic (bytes ↔ ticket).
                     Depends on: tokio, anyhow, iroh, iroh-blobs, rustls[ring].
                     Does NOT depend on codemark-core.
  codemark-cli/      + optional dep `codemark-p2p`, gated by feature `p2p`.
  codemark-tui/      + optional dep `codemark-p2p`, gated by feature `p2p` (later).
  codetours-server/  unchanged. NEVER depends on codemark-p2p.
```

### Dependency direction
```
codemark-cli  --(optional, feature "p2p")-->  codemark-p2p
codemark-tui  --(optional, feature "p2p")-->  codemark-p2p
codemark-cli / codemark-tui / codetours-server  -->  codemark-core
codemark-p2p  -->  (iroh, iroh-blobs, rustls[ring])   # no codemark-core
```
`codemark-cli` orchestrates: it uses `codemark-core` for tour (de)serialization
and DB, and `codemark-p2p` purely to move bytes. The two never know about each
other.

## Why a separate crate — not a module in `codemark-core`

1. **Blast-radius isolation.** iroh drags in a large QUIC/relay/blobs tree. In
   `codemark-core` it would land in **every** crate — including
   `codetours-server` and the default CLI/TUI binaries.
2. **Feature unification is workspace-wide.** A feature flag *inside* core would
   not stop the server from inheriting iroh's transitive features when anything
   in the build graph enables them. An **optional dependency that is off by
   default** keeps iroh out of the dependency graph entirely for default and
   server builds — the only reliable protection for the `openssl-free`
   invariant.
3. **Backend safety.** Isolating iroh in one crate gives a single, obvious place
   to pin the **`ring`** rustls backend and to reason about what feeds
   quinn/rustls — instead of hunting through core's feature matrix.
4. **Binary size.** The default `codemark` binary and the `dist` size profile
   stay untouched unless `--features p2p` is set.
5. **Testability.** A tour-agnostic `bytes ↔ ticket` crate is unit-testable with
   two in-process nodes and raw byte buffers — no tours, no DB, no fixtures.
6. **Convention fit.** Mirrors the existing shape: `codemark-core` is the shared
   base; siblings are focused crates that depend on it (or, here, stand alone).

## Public API (`codemark-p2p`, as shipped)
Minimal and tour-agnostic:
```rust
/// Shareable, self-contained ticket string ("blob…").
pub type Ticket = String;

/// Publish `bytes`; returns the ticket plus a guard that keeps the node serving
/// the blob until dropped or `shutdown()`-ed.
pub async fn push_bytes(bytes: Vec<u8>) -> anyhow::Result<(Ticket, Provider)>;

/// Dial the peer named in `ticket`, download + BLAKE3-verify the blob, return bytes.
pub async fn pull_bytes(ticket: &str) -> anyhow::Result<Vec<u8>>;

pub struct Provider { /* … */ }
impl Provider { pub async fn shutdown(self) -> anyhow::Result<()>; }
```
Serialization, encryption (`003`), AST resolution, and DB writes stay in the CLI
+ `codemark-core`. The CLI bridges the two via two new public core helpers,
`codemark_core::sync::build_pack_bytes(...)` and `import_pack_bytes(...)`, which
reuse the existing portable pack format — so a p2p transfer and an HTTP transfer
import identically. `codemark-p2p` itself moves opaque bytes and has **no
dependency on `codemark-core`.**

## Guardrails (enforced in CI — `test.yml`)
- ✅ A step asserting `Cargo.lock` contains no `aws-lc-sys` / `aws-lc-rs` /
  `native-tls` / `openssl-sys` (runs in the default `test` job, so it holds
  regardless of feature selection).
- ✅ A dedicated `p2p-experimental` job that runs `cargo test -p codemark-p2p`
  and `cargo clippy -p codemark-p2p -p codemark-cli --features p2p -- -D
  warnings` (the default jobs don't enable the feature, so the gated CLI code
  would otherwise never be compiled or linted).
- Still to add: a `cargo tree` assertion that `codetours-server` never contains
  iroh, and the binary-size delta measurement (see `002` §5).

## Naming
`codemark-p2p` is recommended (describes the transport). Alternatives:
`codemark-share`, `codemark-sync`. Avoid `codemark-net` (too generic).

## Wiring order
1. Create `codemark-p2p` with `push_bytes` / `pull_bytes` + in-process tests.
2. Add optional dep + `p2p` feature to `codemark-cli`; wire `--p2p` flags.
3. Add CI guardrails.
4. (Later) wire the same optional dep into `codemark-tui`.
