# Proposal: Decentralized Context Sharing with Iroh

## The Problem
Codemark is an incredibly powerful tool for building structural bookmarks and context "tours" of a codebase. However, the true value of context is unlocked when it is **shared** with teammates or AI agents.

Currently, sharing a tour (e.g., `codemark tour push` and `pull`) implies the existence of a centralized server, a registry, or at least a hosted file endpoint. This introduces friction:
- **Infrastructure Overhead:** Someone has to build, host, maintain, and pay for the backend server.
- **Privacy/Security Concerns:** Teams may be hesitant to upload their codebase context and structural metadata to a third-party cloud.
- **Availability:** If the registry goes down, or if developers are working on an isolated local network or airplane, sharing breaks.

## The Proposed Solution
We propose embedding **[Iroh](https://iroh.computer)**—a modular, peer-to-peer (P2P) networking toolkit written in Rust—directly into the Codemark CLI. 

Instead of pushing tours to a cloud server, Codemark will allow developers to share tours **directly from laptop to laptop**. 
When a user runs `codemark tour push --p2p`, Codemark generates a cryptographic "ticket". When a colleague runs `codemark tour pull <ticket>`, a direct, encrypted QUIC tunnel is established between the two machines, and the tour is transferred.

## Why This is Highly Relevant

Integrating Iroh perfectly aligns with the local-first ethos of developer tools while solving the distribution problem:

### 1. Zero Infrastructure Costs
By moving to a peer-to-peer model, the project avoids the heavy engineering burden of building a backend API, managing databases, and handling user authentication. The network scales infinitely for free because the users' machines *are* the network.

### 2. Ultimate Privacy & Security
Tours never touch a central database. The data moves directly between developers via end-to-end encrypted QUIC connections. Furthermore, because Iroh uses content-addressed storage (BLAKE3 hashing), the downloaded tour is cryptographically guaranteed to be exactly what the author pushed—no tampering is possible.

### 3. "Google Docs" for Codebases
While the MVP focuses on simple push/pull ticket sharing, embedding Iroh's networking stack paves the way for live collaboration. By eventually leveraging `iroh-gossip` and `iroh-docs` (CRDTs), multiple developers could subscribe to a shared tour and add bookmarks simultaneously, with their Codemark UIs updating in real-time.

### 4. Seamless Technical Fit
Because both Codemark and Iroh are built in Rust, the integration is natural. Iroh can run entirely embedded within the Codemark binary without requiring a separate background daemon, keeping the user experience simple and lightweight.

## Next Steps
Please refer to the following documents for technical details and implementation strategies:
- `001-iroh-integration-analysis.md`: Deep dive into Iroh's mechanics and how they benefit Codemark.
- `002-iroh-integration-mvp.md`: Implementation plan, Level of Effort, semantic mapping, and cost estimation for the initial MVP.
