# Proposal: Security & Authorization for P2P Codemark

## The Security Challenge
By moving from a centralized registry to a Peer-to-Peer (P2P) model using Iroh, we lose the ability to use a central server for authentication and authorization (e.g., checking if a user has access to a private GitHub repository before allowing them to download a tour).

If we simply use the repository URL to derive the Iroh `TopicId` (for discovery) or the Ticket, a malicious actor could guess the `TopicId` of a private repository, subscribe to the swarm, and siphon Codemark tours. These tours may contain sensitive file paths, AST structure, code snippets, and architectural notes.

## The Solution: "Git History as a Shared Secret"
We can solve this problem in a purely decentralized, local-first manner by utilizing the mathematical properties of Git. 

If a developer is legally attempting to read or write a Codemark tour for a repository, they must, by definition, have that repository cloned on their local machine. We can use the repository's immutable Git history as a **cryptographic proof of access**.

### 1. The Root Commit Hash
Every Git repository contains a unique "Root Commit" (the initial commit with no parents). 
- It can be efficiently retrieved locally via: `git rev-list --max-parents=0 HEAD`
- This hash is universally identical for all developers who have cloned the repository.
- **Security Guarantee:** It is impossible to know this hash unless you have been granted read access to the private repository.

### 2. Securing Swarm Discovery (Topic IDs)
When utilizing `iroh-gossip` for discovery, we must ensure that attackers cannot even locate or join the swarm.
Instead of deriving the `TopicId` from the public repository URL, we derive it using an HMAC with the Root Commit Hash as the secret key.

```rust
// Conceptual implementation
let root_commit_hash = get_git_root_commit();
let repo_url = get_git_remote_url();

let topic_id = hmac_sha256(
    key: root_commit_hash,
    message: format!("codemark-swarm-{}", repo_url)
);
```
**Result:** The Iroh swarm becomes effectively "dark" to anyone who does not possess the repository's Git history.

### 3. Securing the Data (End-to-End Encryption)
For the actual Iroh Blobs (the pushed tours) and Gossip messages, we add a layer of symmetric encryption before handing the data to the Iroh network stack.

1. Codemark derives an AES-256 (or ChaCha20) encryption key from the Git history (e.g., combining the Root Commit Hash with a salt).
2. When a user runs `codemark tour push`, the tour JSON is encrypted using this key before it becomes an Iroh Blob.
3. When a user runs `codemark tour pull`, the blob is downloaded via Iroh, and Codemark attempts to decrypt it using the local Git history.

**Result:** Even if an Iroh ticket is accidentally leaked on a public forum, the downloader receives an opaque, encrypted blob. If they do not have the private repository cloned locally, the Codemark CLI will fail to decrypt it, rendering the leaked ticket harmless.

## Conclusion
By leveraging the existing cryptographic foundation of Git, Codemark can provide enterprise-grade access control and zero-knowledge encryption in a fully peer-to-peer environment, without requiring any central auth servers, OAuth integrations, or API keys.
