# Iroh Integration MVP Plan

Level of Effort, implementation plan, semantic mapping, and cost estimate for the
initial MVP. See `004-iroh-integration-architecture.md` for the crate layout this
plan assumes.

## 1. Level of Effort
**Estimated Effort:** ~2–3 weeks for one developer (revised up from the original
1–2 weeks).

The bump accounts for work the original estimate omitted:
- Pinning and CI-guarding the **`ring`** crypto backend so iroh does not flip the
  workspace to `aws-lc-rs` (protects the openssl-free / cross-compile invariant).
- Standing up the new `codemark-p2p` crate and its feature wiring.
- Measuring and containing the **binary-size** impact of the QUIC/relay/blobs
  stack against the size-optimized `dist` profile.

What the original estimate got right: iroh's high-level APIs are ergonomic, and
the CLI is **already `#[tokio::main]`**, so there is no async-runtime bootstrap
work.

**Complexity:** Low–Medium for the happy path; the real risk is dependency
hygiene (backend + size), not the networking logic.

## 2. Implementation Plan (Experimental, Feature-Gated)

The transport lives in a new leaf crate, `codemark-p2p`, pulled in only when the
`p2p` feature is enabled. iroh never enters `codemark-core` or
`codetours-server`. See `004` for the rationale and full wiring.

### A. Cargo configuration

> **Implemented.** Resolved versions at implementation time (2026-07):
> `iroh 1.0.1`, `iroh-blobs 0.103.0` — confirming the original `0.28` pin was
> badly stale and that the two crates are versioned independently.
>
> **Good news on the backend:** `iroh 1.0.1`'s *default* features already select
> ring (`default = [metrics, fast-apple-datapath, portmapper, tls-ring]`), and
> the aws-lc path (`tls-aws-lc-rs`) is opt-in. So no `default-features = false`
> gymnastics or explicit `rustls` pin was needed — we just take iroh's defaults
> and never enable `tls-aws-lc-rs`. Verified: `grep -c aws-lc Cargo.lock == 0`
> (and 0 for `openssl-sys` / `native-tls`) with the feature both on and off.

**`crates/codemark-p2p/Cargo.toml`** (new crate — always builds with iroh):
```toml
[package]
name = "codemark-p2p"
edition = "2024"

[dependencies]
tokio = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }

# iroh's default features already select the ring rustls backend; tls-aws-lc-rs
# is opt-in and we never enable it. CI asserts the lockfile stays aws-lc-free.
iroh = "1.0"
iroh-blobs = "0.103"
```

**`crates/codemark-cli/Cargo.toml`** (optional dep + feature):
```toml
[features]
p2p = ["dep:codemark-p2p"]

[dependencies]
codemark-p2p = { path = "../codemark-p2p", optional = true }
```

Because `codemark-p2p` is **optional and off by default**, iroh is absent from
the default and server builds entirely — no feature unification, no size hit, no
backend flip unless someone opts in with `--features p2p`.

New CLI code paths are gated with `#[cfg(feature = "p2p")]`.

### B. MVP commands

**Export — `codemark tour push --p2p <name>`**
1. Load the tour and serialize it (JSON/MessagePack) via `codemark-core`.
2. (Optional, see `003`) encrypt the bytes with a shared passphrase.
3. Hand the bytes to `codemark-p2p::push_bytes(...)`, which adds them to an
   embedded `iroh-blobs` node and returns a **ticket** (`NodeId` + `Hash` +
   relay hints).
4. Print the ticket to stdout and **block, serving the blob until Ctrl+C.**
   The CLI must clearly tell the user: *"Keep this running until your teammate
   pulls."*

**Import — `codemark tour pull --p2p <ticket>`**
1. Parse the ticket.
2. `codemark-p2p::pull_bytes(ticket)` dials the peer, downloads + BLAKE3-verifies
   the blob, returns the bytes.
3. (Optional, see `003`) decrypt with the shared passphrase.
4. Deserialize via `codemark-core`, resolve AST locations locally, save the tour
   to the local DB via core.

**Design note:** `codemark-p2p` is **tour-agnostic** — it moves `bytes ↔ ticket`
and knows nothing about tours, serialization, encryption, or the DB. That keeps
iroh's blast radius fully contained and the transport unit-testable with raw
bytes. It therefore needs **no dependency on `codemark-core`.**

## 3. Semantic Mapping: Codemark → Iroh

| Codemark Concept | Iroh Primitive | Notes |
| :--- | :--- | :--- |
| CLI/TUI instance | **Node / Endpoint** | Identified by a `PublicKey` (`NodeId`). |
| Serialized tour (bytes) | **Blob** | Immutable, addressed by BLAKE3 hash. |
| Shareable link | **Ticket** (`iroh…`) | `NodeId` + blob `Hash` + relay hints. Grants integrity; **not** confidentiality on its own (see `003`). |
| Repo grouping *(Phase 2)* | **Gossip `TopicId`** | 32-byte topic; obfuscated derivation discussed in `003`. |
| Live pairing *(Phase 3)* | **`iroh-docs` doc** | Last-write-wins per key; maintenance status unverified. |

## 4. Cost Estimate

**MVP: ~$0.00.** Direct/LAN and successful hole-punches are free. Relay fallback
uses n0's public relays by default.

**Caveat the original draft omitted:** n0's public relays are provided for
development and are **not a production SLA**. Relying on them for a shipped
feature is fine for an experiment, but "free forever" is not guaranteed — n0 may
rate-limit. Plan to run our own relay if the feature graduates.

**Self-hosted relay (if/when needed): ~$5–10/month.** Relays only forward
encrypted bytes (no storage, no heavy compute); a small VPS handles many
concurrent connections, and most connections succeed via direct hole-punching,
so relay traffic is a small fraction.

## 5. Definition of Done for the MVP
- [x] `--features p2p` builds; default build unaffected (iroh absent from the
      default `codemark-cli` dependency graph — verified with `cargo tree`).
- [x] `grep -c aws-lc Cargo.lock` == 0 (also `openssl-sys` / `native-tls`),
      feature on and off. Now CI-checked in `test.yml`.
- [x] `codemark-p2p` unit-tested end-to-end with raw bytes over two in-process
      nodes, fully offline (`presets::Minimal`, direct addresses).
- [x] CLI `--p2p` wired on `tour push` / `tour pull`; default build errors
      clearly ("compiled without p2p support; rebuild with `--features p2p`").
- [x] `push --p2p` output states the "keep this running until the pull
      completes" constraint.
- [ ] **Remaining:** binary-size delta of the `p2p` feature measured on the
      `dist` profile and recorded. (Run `cargo build --profile dist --bin
      codemark` with and without `--features p2p` and diff the sizes.)
