# Validating Iroh for Codemark Tours

This document explores the viability and benefits of using [iroh](https://github.com/n0-computer/iroh) as the underlying networking and synchronization layer for sharing **Codemark Tours**. 

## What is Iroh?
Iroh is a modular, peer-to-peer (P2P) networking toolkit built in Rust by n0-computer. Instead of dealing with IP addresses, DNS, and central servers, Iroh allows devices to connect directly to each other using **public keys**. It leverages QUIC for fast, multiplexed, and secure connections and includes built-in hole-punching (with relay fallbacks) to traverse NATs and firewalls seamlessly.

Iroh is not a monolithic framework but a collection of composable protocols:
- **`iroh-blobs`**: A protocol for content-addressed, BLAKE3-verified data transfer. It efficiently synchronizes arbitrary binary data (from small JSONs to terabytes of files).
- **`iroh-gossip`**: A publish-subscribe protocol using epidemic broadcast trees for efficient message dissemination across a swarm of peers.
- **`iroh-docs`**: An eventually-consistent, multi-dimensional key-value store built on top of `blobs` and `gossip` using CRDTs (Conflict-free Replicated Data Types).

## How Codemark Tours Work Today
Codemark tours are collections of structural code bookmarks that map to tree-sitter AST queries, along with contextual notes, comments, and tags. They help humans and AI agents share context (e.g., "login-flow", "auth-refactor").
Currently, sharing involves `codemark tour push <name>` and `codemark tour pull <url>`, which implies the need for a centralized host, registry, or at least a static file server to share these collections.

## Why Iroh is a Perfect Fit for Codemark

Integrating Iroh into Codemark would fundamentally shift how developers and AI agents collaborate, moving from a static push/pull model to a dynamic, serverless P2P model.

### 1. True Peer-to-Peer "Push/Pull" (No Central Server)
With `iroh-blobs`, Codemark would no longer need a central registry. 
- **The Workflow:** When a user runs `codemark tour push login-flow`, Codemark creates an Iroh blob of the tour data and outputs an **Iroh ticket** (a self-contained string containing the node's public key and routing info).
- **The Consumer:** Another developer (or an AI agent in a different session) runs `codemark tour pull <ticket>`. Their local Iroh node connects directly to the provider, verifies the BLAKE3 hash, and downloads the tour.
- **Benefit:** Zero infrastructure required. No API keys, no hosting costs, no downtime.

### 2. Live Collaboration with `iroh-docs`
Currently, a tour is a static snapshot. If you use `iroh-docs` as the storage layer for a tour, it becomes a **live, collaborative workspace**.
- Because `iroh-docs` uses CRDTs, multiple developers (or multiple agents) can add bookmarks, notes, and tags to the *same tour concurrently*.
- As soon as a peer comes online or makes a change, it gossips the state update.
- **Benefit:** Real-time pairing. You could have a "Pairing Session" tour where you and an AI agent are both dynamically adding codemarks to a shared document as you discover things.

### 3. Immutability and Trust
Codemarks are highly specific, relying on AST structure and exact file paths. 
- Using `iroh-blobs`, the underlying data is content-addressed (addressed by its BLAKE3 hash).
- **Benefit:** This guarantees cryptographic immutability. When you pull a tour via its hash, you are mathematically guaranteed to receive the exact context the author bookmarked, with zero tampering or transmission errors.

### 4. Seamless Rust Integration
Since `codemark` is written in Rust (as seen in the TUI logging subsystem and `.rs` extension examples in the tool definitions) and Iroh is a Rust-native networking stack, the integration would be incredibly natural. 
- Iroh's crates (`iroh`, `iroh-blobs`, `iroh-docs`) can be imported directly into the Codemark workspace.
- It can run entirely embedded within the `codemark` CLI binary without requiring a separate background daemon (Iroh nodes can be spun up Ephemerally or persistently).

## Potential Implementation Strategy

1. **Phase 1: P2P Export/Import (iroh-blobs)**
   Replace the standard HTTP push/pull with Iroh tickets. When exporting a tour, serialize it to JSON/MessagePack, add it to an embedded Iroh node, and generate a ticket. 

2. **Phase 2: Tour Subscriptions (iroh-gossip / iroh-docs)**
   Allow users to "subscribe" to a tour author's public key. Whenever the author updates the tour, peers automatically fetch the new blob. 

3. **Phase 3: Multi-player Context (iroh-docs)**
   Migrate the underlying storage of active tours to an `iroh-docs` document. This allows a team of engineers to share a continuous, ever-evolving context map of their codebase that syncs automatically in the background.

## Conclusion
Validating the idea: **Highly Recommended.**
Using Iroh for Codemark tours perfectly aligns with the ethos of developer tools: it's local-first, fast, requires zero infrastructure, and leverages the same programming language (Rust). It turns Codemark from a solitary bookmarking tool into a powerful, decentralized multiplayer context engine.
