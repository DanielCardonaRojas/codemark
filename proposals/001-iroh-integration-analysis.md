# Analysis: Iroh as a P2P Transport for Codemark Tours

This document explores the viability of [iroh](https://github.com/n0-computer/iroh)
as a **serverless, optional** networking layer for sharing **Codemark Tours**,
alongside the existing registry (`codetours-server`).

## What is Iroh?
Iroh is a modular P2P networking toolkit in Rust by n0-computer. Instead of IP
addresses, DNS, and central servers, devices connect directly using **public
keys** (a `NodeId` is an ed25519 public key). It uses **QUIC** for fast,
multiplexed, encrypted connections and includes NAT hole-punching with relay
fallback.

Since n0's "new direction" restructuring, iroh's core is the **connection layer**
(the `Endpoint` / dialing / hole-punching), and the higher-level protocols live
in **separately versioned, separately maintained** crates:

- **`iroh-blobs`** — content-addressed, BLAKE3-verified data transfer. This is
  the only protocol the MVP needs.
- **`iroh-gossip`** — publish/subscribe over epidemic broadcast trees. (Phase 2.)
- **`iroh-docs`** — a multi-writer key-value store on top of blobs + gossip.
  (Phase 3 — **status must be verified; see caveat below.**)

> ⚠️ **Version note:** because these were split out, `iroh` and `iroh-blobs`
> **no longer share a version number**. Do not copy a pinned version from an old
> tutorial; run `cargo add iroh iroh-blobs` and take current, mutually
> compatible releases, checking their MSRV against Codemark's toolchain.

## How Codemark Tour Sharing Works Today
Sharing is **not** hypothetical — it already exists:

- `codemark publish` (`crates/codemark-cli/src/cli/handlers/publish.rs`) resolves
  a server + token from the registry and uploads a tour to `codetours-server`.
- `codemark pull` (`.../handlers/pull.rs`) fetches a tour over HTTP (`GET
  /tours/...`), optionally using a registry token.
- Tour types, serialization, AST resolution, and DB storage all live in
  `codemark-core`.

So the P2P work is **additive**: a second transport, not a rewrite. The existing
registry keeps its role as the durable, async, authenticated channel.

## Where Iroh Fits

### 1. Serverless push/pull (`iroh-blobs`) — the MVP
- `codemark tour push --p2p <name>`: serialize the tour, add the bytes to an
  embedded `iroh-blobs` node, print a **ticket** (`NodeId` + blob `Hash` + relay
  hints). The process stays alive to serve the blob.
- `codemark tour pull --p2p <ticket>`: dial the peer from the ticket, download
  the blob, verify BLAKE3, deserialize, resolve AST locally, save via core.
- **Benefit:** no reachable server required; no account needed by the receiver.
- **Cost:** sender and receiver must be online at the same time (see `000`).

### 2. Immutability and trust
`iroh-blobs` addresses data by its BLAKE3 hash, which is embedded in the ticket.
The receiver is cryptographically guaranteed to get exactly the author's bytes.
Note this gives **integrity**, not **confidentiality** — anyone holding the
ticket can fetch the blob. Confidentiality is handled in `003`.

### 3. Live collaboration (Phase 2/3 — treat as unvalidated)
`iroh-gossip` could power "subscribe to a tour author" and `iroh-docs` could back
a multi-writer tour. **Two caveats before betting on this:**

- **Maintenance risk:** after n0's refocus on the connection layer, `iroh-docs`
  is not a first-tier maintained protocol. Verify its current status and release
  cadence before designing Phase 3 around it.
- **Semantics:** `iroh-docs` is **last-write-wins per key**, not a rich CRDT that
  merges concurrent edits to the *same* value. Concurrent edits to *different*
  bookmarks merge cleanly; concurrent edits to the *same* field do not
  auto-merge. "Google Docs for codebases" overstates this — frame it as
  "eventually-consistent shared key-value tours."

### 4. Rust fit — and one sharp edge
Iroh is Rust-native and embeds in the CLI with no daemon; the CLI is already
`#[tokio::main]`, so **no new async runtime work is needed**. The sharp edge is
the **TLS/crypto backend**: iroh's QUIC stack (quinn + rustls) can pull in
`aws-lc-rs`, which needs a C toolchain/cmake and complicates musl/Windows
cross-compilation. Codemark's lockfile is currently **`ring`-only with zero
`aws-lc`**, and the project maintains an openssl-free, clean-cross-compile
invariant. **The p2p crate must force the `ring` backend and CI must assert the
lockfile stays `aws-lc`-free.** See `002` and `004`.

## Conclusion
Adding Iroh as an **optional, feature-gated, serverless** transport is a sound,
additive enhancement. Phase 1 (`iroh-blobs`) is well-supported and low-risk once
the `ring` backend and binary-size questions are handled. Phases 2–3 are
promising but should be treated as **unvalidated** pending verification of
`iroh-docs`' maintenance status.
