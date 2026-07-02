# Security & Confidentiality for P2P Codemark

> **This document was rewritten after review.** The original "Git history as a
> shared secret" design had fundamental flaws (documented below so we don't
> reintroduce them). The recommended MVP model is simpler and stronger.

## Threat model — scoped correctly
The security concern depends heavily on **which phase** we're in. The original
draft solved a Phase-2 problem while proposing a Phase-1 MVP.

- **MVP (`iroh-blobs`, ticket sharing):** there is **no discovery swarm**. To
  fetch a blob you need the ticket, which contains the `NodeId` to dial *and*
  the blob `Hash`. Without the ticket you cannot even locate the peer. So the
  only realistic risk is a **leaked ticket** (e.g., pasted in a public channel).
  A leaked ticket grants read access to that one tour while the sender is online.
- **Phase 2 (`iroh-gossip` discovery):** here a *guessable* `TopicId` is a real
  attack surface — an attacker who can derive the topic could join the swarm.
  This is the only place topic-obfuscation matters.

## MVP recommendation: capability + optional passphrase

### 1. Baseline — the ticket is the capability
Treat the ticket as a bearer token. Share it over a **trusted channel**
(Slack DM, 1Password, signal). This is exactly how the existing registry tokens
are handled and is sufficient for the "send me your tour" use case. Document the
leak risk plainly in `--help`.

### 2. Optional end-to-end encryption — author-generated passphrase
For sensitive tours, add a symmetric encryption layer *before* the bytes become
an iroh blob, keyed by a **passphrase the author generates and shares
out-of-band** (separate from the ticket):

```text
push:  key = KDF(passphrase, random_salt)        # e.g. Argon2id
       ciphertext = AEAD_encrypt(key, tour_bytes) # ChaCha20-Poly1305 / AES-256-GCM
       blob = salt || nonce || ciphertext
       -> share ticket (channel A) + passphrase (channel B)

pull:  download blob -> derive key from passphrase+salt -> AEAD_decrypt
```

Why a passphrase and **not** git history (see failures below): a passphrase has
real entropy, is **rotatable**, is **revocable** (rotate it and re-push), and is
independent of who has cloned the repo. Even if the ticket leaks, an attacker
without the passphrase gets an opaque blob.

## Why the original "Git history as a shared secret" design was rejected

The proposal derived topic IDs and encryption keys from the repository's root
commit hash (`git rev-list --max-parents=0 HEAD`). This fails on several axes:

1. **The root commit hash is not a secret.**
   - Forks and template-derived repos **share their root commit with a public
     repo** — the "secret" is then public.
   - It appears in CI logs, on every clone, and on every disposable/CI machine.
2. **No revocation, no rotation.** Anyone who *ever* cloned the repo (former
   employees, contractors) keeps the derived key **forever**. A central auth
   server can revoke access; a git-derived key cannot. This is strictly weaker
   than the token auth Codemark already has.
3. **No forward secrecy.** The key is a static function of the root commit, so a
   single leak compromises **all** past and future tours for that repo.
4. **Static salt = no added entropy.** If the salt ships in the binary, the
   "AES-256 key" inherits the exact low-secrecy/leakage profile of the root
   commit hash.
5. **Correctness bug — repos can have multiple root commits.** Grafted/merged
   histories and orphan branches make `--max-parents=0 HEAD` return several
   hashes; two clones can then derive *different* keys and pulls fail silently.
   Any git-derived scheme must deterministically pick one root
   (e.g., the root of HEAD's first-parent chain).

**Conclusion on git-as-secret:** acceptable at most as *obscurity* for Phase-2
topic derivation — never as access control or confidentiality. Do not present it
as "enterprise-grade access control."

## Phase 2 note (when we add gossip discovery)
If/when discovery via `iroh-gossip` is added, obfuscate the `TopicId` so the
swarm isn't trivially enumerable — e.g. `TopicId = HMAC(secret, "codemark:" ||
repo_url)`. Use a **rotatable team secret** (shared like the passphrase above),
*not* the root commit hash, and document it explicitly as **obscurity, not
authorization**. Real confidentiality stays with the AEAD layer from §2.

## Summary
- MVP: ticket-as-capability over a trusted channel; **optional** author-generated
  passphrase for AEAD encryption. No git-derived secrets.
- Never rely on the root commit hash for access control or key derivation.
- Defer topic-obfuscation to Phase 2, keyed by a rotatable team secret.
