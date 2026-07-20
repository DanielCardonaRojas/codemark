# Iroh Integration MVP Plan for Codemark

This document outlines the Level of Effort (LoE), an implementation plan for an MVP, the semantic mapping between Codemark and Iroh, and a cost estimation.

## 1. Level of Effort (LoE) for MVP
**Estimated Effort:** 1 to 2 weeks for one developer.
**Complexity:** Low to Medium. Iroh is built in Rust and provides high-level APIs that abstract away the complex cryptography and networking.

The MVP will focus **only on the "Slack Method"** (direct ticket sharing using `iroh-blobs`) to validate the core P2P mechanics without the overhead of implementing local discovery and gossip swarms.

## 2. Implementation Plan (Experimental Feature)

We will gate this behind a Cargo feature flag so it is not compiled into the default binary.

### A. Cargo Configuration
Update `Cargo.toml` to include the feature flag and optional dependencies:
```toml
[features]
default = []
experimental-iroh-sync = ["dep:iroh", "dep:iroh-blobs"]

[dependencies]
iroh = { version = "0.28", optional = true }
iroh-blobs = { version = "0.28", optional = true }
tokio = { version = "1", features = ["full"] } # Required as Iroh is async
```
*Note: The binary will need conditional compilation (`#[cfg(feature = "experimental-iroh-sync")]`) around the new commands.*

### B. MVP Commands
We will introduce two new subcommands (or flags on existing ones):

**1. Exporting a Tour (`codemark tour push --p2p <name>`)**
- **Action:** Serialize the local tour (JSON/MessagePack).
- **Iroh Step:** 
  - Spin up an ephemeral in-memory Iroh node.
  - Add the serialized bytes to the node as a Blob.
  - Generate an Iroh Ticket containing the node's Public Key, the blob's Hash, and Relay hints.
- **Output:** Print the ticket to stdout (`iroh1...`). The CLI must stay alive (wait/block) to serve the file until the user hits `Ctrl+C`.

**2. Importing a Tour (`codemark tour pull --p2p <ticket>`)**
- **Action:** Parse the provided ticket.
- **Iroh Step:**
  - Spin up an ephemeral Iroh node.
  - Connect to the peer using the ticket.
  - Download the blob into memory.
- **Output:** Deserialize the bytes back into a Codemark Tour, resolve the AST locations locally, and save the tour to the local Codemark database.

## 3. Semantic Mapping: Codemark to Iroh

To help mental models, here is how Codemark concepts map directly to Iroh primitives:

| Codemark Concept | Iroh Primitive | Description |
| :--- | :--- | :--- |
| **Codemark CLI Instance** | **Node / Endpoint** | A running instance of the network stack, identified by a cryptographic `PublicKey` (NodeId). |
| **Exported Tour (Data)** | **Blob** | An immutable chunk of binary data addressed by its BLAKE3 hash. |
| **Shareable Link** | **Ticket** | A base32 string (`iroh1...`) given to a colleague. It contains the `NodeId` to connect to and the `Hash` of the blob to ask for. |
| **Git Repository (e.g., origin URL)** | **Gossip TopicId** *(Phase 2)* | A 32-byte hash used to group peers. Nodes working in the same repo subscribe to the same TopicId to discover each other. |
| **Live Pairing Session** | **Iroh-Docs Document** *(Phase 3)* | An eventually-consistent, CRDT-backed key-value store where multiple developers can concurrently append bookmarks. |

## 4. Cost Estimate to Run Iroh

One of the massive benefits of a P2P architecture is that you push the bandwidth and compute costs to the edge (the users' laptops).

**Cost for the MVP: $0.00**
- **Direct Connections:** Local network and successful UDP hole punches are completely free.
- **Relays (DERP):** If hole punching fails (e.g., Symmetric NAT), the connection falls back to a Relay. By default, Iroh ships with hardcoded, public relay servers maintained by `n0-computer` (the creators of Iroh). These are currently **free to use** for open-source and reasonable traffic.

**Cost at Scale (If you run your own Relay): ~$5 to $10 / month**
- If Codemark grows massively and you exceed the fair-use of n0's public relays, or if you want guaranteed uptime and privacy, you can run your own Iroh Relay server.
- Because relays only shuffle encrypted bytes and do not store data or do heavy compute, a single basic $5/month DigitalOcean Droplet or AWS Lightsail instance can handle thousands of concurrent relay connections.
- Furthermore, because 80-90% of connections will succeed in direct hole-punching, the relay only handles the small percentage of fallback traffic.

**Conclusion:** Infrastructure costs for this feature will be functionally zero for the foreseeable future.
