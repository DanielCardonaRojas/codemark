# Design: Repo-Possession-Gated Sharing via Content Challenges

> Status: proposal. Hardens the p2p sharing feature (`proposals/000–005`) so a
> tour can only be opened by someone who possesses a clone of the associated
> repository. Supersedes the rejected "git root commit as a shared secret" idea
> in `proposals/003` with a stronger construction.

## Motivation & policy

A p2p ticket today is an unauthenticated bearer capability: anyone who obtains it
during the serving window can pull the full tour (file paths, structure, notes).
We want a decentralized gate with this policy:

> A Codemark tour reveals a **subset** of what the repository source reveals.
> Therefore anyone who can read the source may safely receive the tour. We treat
> **possession of a clone of the repo** as sufficient proof of that read
> capability.

The goal is to bind a tour to its repository so that **the ticket alone is
inert** — opening it also requires a local clone of the repo the tour describes.

## Threat model

**In scope.** A leaked/relayed ticket (wrong Slack channel, clipboard scraper,
shoulder-surf) reaching a party who does **not** have the repo. Passive network
eavesdroppers (already covered by the QUIC/TLS transport).

**Explicit non-goals** (inherent to git having no access model of its own — access
is enforced by the host, and a clone is plaintext):

- **No revocation.** A former collaborator's existing clone still opens tours
  forever. If you need revocation, you must consult the host (see §8).
- **Not identity-bound / transferable.** A clone-holder can hand the derived key
  (or a decrypted tour) to anyone. Possession proves *past read capability*, not
  *current authorization* and not *who* the person is.
- **Weak against public forks/templates.** Repos derived from a public repo share
  history; content in the shared portion is not secret. Mitigated, not solved
  (see §6).

This is a **heuristic possession gate, not access control.** It is a large
improvement over a bare bearer ticket, and it is honest about what it is not.

## Core construction: content-derived key (non-interactive)

The "challenge" is realized cryptographically: the payload is encrypted under a
key derived from the git object ids at a set of randomly chosen locations in the
repo's history. Only a clone-holder can recompute those ids, hence the key.

### 1. Challenge selection (push side)
From the sender's clone, pick `N` random **challenges**, each a
`(commit_oid, path)` pair:
- Walk history reachable from the tour's repo default branch.
- Bias toward **recent** commits (more likely private; more likely already
  fetched by the receiver — see §6 for the secrecy/availability tension).
- For each chosen commit, pick a random tracked file `path` that exists in that
  commit's tree (`git ls-tree -r <commit>`).

### 2. Answer computation (both sides)
The **answer** to a challenge is the git blob id of that path at that commit —
identical across all clones, and unknowable without the repo content:

```
answer = git rev-parse <commit_oid>:<path>     # a blob OID (a content hash)
```

(Via `git2`: `repo.revparse_single("<commit>:<path>")?.id()`. No file contents
are read into the process; the blob OID *is* the content hash.)

### 3. Key derivation
Mix a repo binding and a random salt so a ticket for repo A can't be answered
with repo B's content and so identical tours don't reuse a key:

```
ikm  = concat(sort(answers))                       # only a clone-holder has this
key  = HKDF-SHA256(salt, ikm, info = "codemark-p2p-repo-seal:v1|" || repo_id)
```

`repo_id` = a stable repo identifier (e.g. normalized origin URL, or the root
commit OID). It is not relied on for secrecy — only to scope the key.

### 4. Sealing the pack
Encrypt the existing portable pack bytes with an AEAD (ChaCha20-Poly1305):

```
sealed = header || AEAD_encrypt(key, nonce, pack_bytes)
header = { version, repo_id_hint, challenges: [(commit_oid, path)…], salt, nonce }
```

The **header is plaintext**; it names *which* objects to hash but never the
answers. `codemark-p2p` transports `sealed` as opaque bytes exactly as today — the
transport stays tour-agnostic and unchanged.

### 5. Opening (pull side)
The receiver runs the pull from **inside their clone** of the repo:
1. Download `sealed` (content-addressed, as today).
2. Read the plaintext header; recompute the answers from the local repo.
3. Derive `key`; AEAD-decrypt.
4. Failure to resolve the objects, or an AEAD tag mismatch → *"not authorized:
   this tour is sealed to <repo>; open it from a clone of that repository."*

Anyone holding the ticket but lacking the repo downloads only opaque ciphertext.

## Optional interactive gate (defense in depth)

Construction above lets *anyone* download the ciphertext (they just can't read
it). If you also want to withhold the bytes from non-provers, add an interactive
proof before transfer: the provider sends the challenge list, the receiver
returns the answers (or an HMAC over them), the provider verifies against its own
and only then serves `sealed`. This narrows exposure but does **not** replace the
encryption (a relayed-to-accomplice answer or a byte leak elsewhere would defeat
a gate-only scheme). Treat it as optional hardening layered on §Core.

## Robustness: threshold answers

A receiver may legitimately hold the repo but be missing a specific challenged
object (shallow clone, un-fetched recent commit). To tolerate this, use a
`k`-of-`n` threshold: split `key` with Shamir secret sharing into `n` shares,
encrypt each share `i` under a key derived from answer `i`, and store the wrapped
shares in the header. The receiver recovers `key` from any `k` answers it can
compute. An attacker still needs `k` private answers. Recommended defaults:
`n = 6`, `k = 4`.

## Security analysis

- **Leaked ticket, no repo:** attacker gets ciphertext only; cannot derive the
  key. ✅ Primary goal met.
- **Forks / shared history:** if `k` of the sampled `(commit, path)` fall in a
  publicly-shared portion, an attacker with the public fork can answer them.
  Mitigation: prefer recent/private history and sample widely; optionally let the
  author mark a private path prefix to sample from. Residual risk remains for
  heavily-public-derived repos — document it. ⚠️
- **Revocation:** none. Fundamental (see non-goals). ⚠️
- **Transferability:** a clone-holder can share the key/plaintext. Fundamental. ⚠️
- **Metadata leakage:** the header reveals that certain `commit_oid`s and `path`s
  exist. For private repos an outsider generally doesn't know these already;
  still, it is a minor disclosure. If undesirable, commit to challenges via a
  salted hash the receiver can match rather than listing raw OIDs (costs a lookup
  strategy). ⚠️ minor.
- **Availability false-negatives:** a legitimate receiver with a diverged/shallow
  clone may miss objects → threshold scheme (above) mitigates. Choose challenge
  commits old enough to be widely fetched.
- **Entropy / brute force:** blob OIDs are collision-resistant content hashes; the
  key's strength rests on the *secrecy* of the content, not guessable entropy. No
  offline dictionary applies unless the content itself is guessable/public.
- **Transport:** unchanged — QUIC/TLS 1.3, BLAKE3 integrity, ephemeral NodeId.

## Integration

Keep `codemark-p2p` a pure byte transport. Add the seal/open step in a small,
git-aware module (reusing the existing `git2` dependency), sitting between the
pack helpers and the transport:

```
push:  pack_bytes = build_pack_bytes(...)
       sealed     = repo_seal::seal(repo_path, pack_bytes)?      # new
       push_bytes(sealed)

pull:  sealed     = pull_bytes(ticket)?
       pack_bytes = repo_seal::open(repo_path, sealed)?          # new (or clear error)
       import_pack_bytes(pack_bytes, ...)
```

- Home: a new `repo_seal` module in `codemark-core` (has git2 + is shared by CLI
  and TUI), or a dedicated `codemark-repo-seal` crate. It depends on git2 +
  `chacha20poly1305` + `hkdf` (all rustls/ring-free — preserves the openssl-free
  invariant).
- CLI/TUI: seal on push, open on pull; surface the "open from a clone" error.
- Make sealing **opt-in per push** at first (`--seal` / a TUI toggle), so
  unsealed sharing still works and we can validate the UX.

## Parameters (initial defaults)

| Parameter | Default |
| :-- | :-- |
| Challenges sampled `n` | 6 |
| Threshold `k` | 4 |
| History window | commits reachable from default branch, last ~500 |
| AEAD | ChaCha20-Poly1305 |
| KDF | HKDF-SHA256 |

## Open questions

1. **repo_id source:** normalized origin URL (stable, human-meaningful, but can
   change) vs. root commit OID (immutable, but shared by forks). Possibly both.
2. **Private-path hint:** let authors restrict sampling to a path prefix to dodge
   fork-shared content? Adds config surface.
3. **Interactive gate:** ship it, or is encryption-only sufficient for v1?
4. **Shallow/partial clones:** is the threshold enough, or do we also constrain
   challenge commits to a guaranteed-present range (e.g. first-parent, older than
   T)?
5. **UX on failure:** distinguish "you're not in a repo", "wrong repo", and
   "repo present but too diverged/shallow" for a good error message.

## Relationship to prior proposals

- Replaces `proposals/003`'s rejected static root-commit key with a randomized,
  content-derived, threshold construction that avoids the "identifiers-as-secrets"
  and single-static-key pitfalls.
- Composes with `proposals/005` (TUI): the seal/open step is transport-agnostic,
  so both CLI and TUI get it by sealing before `push_bytes` and opening after
  `pull_bytes`.
- Still not a substitute for host-based authorization when **revocation** is
  required; that path (present a short-lived host token) remains the only fully
  revocable option and can be offered alongside.
