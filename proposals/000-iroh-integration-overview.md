# Proposal: Serverless Peer-to-Peer Tour Sharing with Iroh

> **Status:** Draft for review. Refined after technical validation — see
> `004-iroh-integration-architecture.md` for the crate layout and
> `003-iroh-integration-security.md` for the (revised) security model.

## The Problem
Codemark builds structural bookmarks and context "tours" of a codebase. That
context is most valuable when **shared** with teammates or AI agents.

Today, sharing already works through a **central registry + server**
(`crates/codetours-server`, driven by `codemark publish` / `codemark pull`
over authenticated HTTP). That model is solid, but it isn't the right tool for
every situation:

- **Requires reachable infrastructure.** No server, no share — which rules out
  isolated networks, air-gapped machines, planes, or ad-hoc "just send me your
  tour" moments.
- **Requires an account/token** on a running registry for the receiver.
- **Not ideal for one-off, ephemeral handoffs** between two people who just
  want to pass a tour directly.

This proposal does **not** replace the registry. It adds a second,
**serverless** channel for direct laptop-to-laptop sharing.

## The Proposed Solution
Embed **[Iroh](https://iroh.computer)** — a modular, peer-to-peer (P2P) QUIC
networking toolkit written in Rust — as an **optional, feature-gated** sharing
transport in the Codemark CLI (and later the TUI).

When a user runs `codemark tour push --p2p`, Codemark generates a cryptographic
**ticket**. When a colleague runs `codemark tour pull --p2p <ticket>`, a direct,
end-to-end-encrypted QUIC tunnel is established between the two machines and the
tour is transferred, verified by its BLAKE3 hash.

## Honest Positioning (corrected from the original draft)
The earlier draft framed this as "avoids building a backend we don't have." That
is inaccurate — **Codemark already ships `codetours-server` with a registry,
tokens, and auth.** The real value of the P2P channel is:

### 1. Works with zero reachable infrastructure
Direct/LAN transfers and successful NAT hole-punches need no server at all.
Great for offline, air-gapped, or "no account required" sharing.

### 2. Strong end-to-end guarantees
Data moves directly between peers over encrypted QUIC. Content-addressing
(BLAKE3) means the receiver is cryptographically guaranteed to get exactly what
the author pushed — no tampering, no corruption.

### 3. A path toward live collaboration (later, unvalidated)
The MVP is simple ticket push/pull. Iroh's `iroh-gossip` and `iroh-docs` *could*
later enable subscriptions and multi-writer tours — but see the caveats in
`001` and `002`: `iroh-docs`' maintenance status must be verified before we
commit to it, and it is last-write-wins per key, not a rich merge CRDT.

## Known Trade-off (must be communicated to users)
The MVP is a **synchronous, both-online handoff**: the sender's process must
stay running to serve the blob until the receiver pulls it (n0 relays forward
live packets, they do not store data). This is a deliberate limitation of the
MVP and a *regression in convenience* from the existing `publish` (push once,
pull anytime). It is fine for "screen-share the ticket" pairing; it is not an
async drop-box. Later phases can add persistence.

## Next Steps
- `001-iroh-integration-analysis.md` — how Iroh works and where it fits.
- `002-iroh-integration-mvp.md` — MVP scope, LoE, dependencies, cost.
- `003-iroh-integration-security.md` — revised access-control / encryption model.
- `004-iroh-integration-architecture.md` — crate structure and dependency wiring.
