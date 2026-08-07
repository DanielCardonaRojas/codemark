# Core Concepts

Codemark revolves around a few primitives designed to make code navigation and
context-sharing durable.

## Bookmarks

A **bookmark** is not a line number. It's a semantic anchor: a tree-sitter query
that identifies code by its position in the Abstract Syntax Tree (a function, a
type, a block), plus a normalized content hash for fallback matching.

Because it tracks *structure*, adding lines above the bookmark or refactoring the
surrounding code doesn't lose your place — Codemark re-runs the query against the
current tree and finds the node again. This is **Smart Resolution**, the tiered
matcher covered in [Health Status](./health-status).

Every bookmark carries:

- The **query** — the durable identity, generated once at creation.
- A **content hash** — `sha256:` over whitespace-normalized text, for drift
  recovery.
- **Annotations** — append-only notes and comments with provenance (author,
  timestamp, source).
- **Tags** — a structured, colon-prefixed taxonomy (`feature:auth`,
  `layer:api`, `role:entrypoint`).
- **Health** — `active`, `drifted`, `stale`, or `archived`.
- **Git context** — commit, branch, and breadcrumbs up the enclosing structure.

## Collections

A **collection** (also called a *tour*) is an ordered grouping of bookmarks —
think of it as a playlist or a guided tour of a specific flow in your codebase.

For example, a `request-lifecycle` collection might hold:

1. The API route handler
2. The authentication middleware
3. The service layer
4. The query builder

Collections have their own metadata: a description, tags, links (PRs, issues,
docs), and a rolled-up health (the worst of their bookmarks). Publish and pull
them across a team with the `codetours` sync server.

## Tours

A **tour** is the shareable, publishable form of a collection — the same ordered
bookmarks, packaged for syncing to a remote server with `P` (push) / `p` (pull)
in the TUI. In the CLI, `codemark tour` commands manage them.

## Repositories & identity

Codemark is **Git-aware**. It detects your repository automatically (walking up
from the working directory) and tracks bookmarks by a durable identity derived
from the Git remote (`owner/name`), not just the path on disk. This is what
enables multi-repo queries and lets a moved repo be recognized. See
[Repository Registry](./repository-registry).

## Worktrees

Git worktrees share the main repo's `.git`, so Codemark resolves every worktree
to the same repo root and the same `.codemark/codemark.db`. Your bookmarks are
accessible from any worktree of the same repository — no duplication, no setup.

## Notes vs. comments

Two distinct, durable-vs-ephemeral channels on every bookmark:

- **Notes** (`--note`) — durable, plain-prose explanations of *what the code is
  and why it matters*. Context-independent and reusable across any session.
- **Comments** (`--comment`) — markdown discussion tied to a *task, ticket, or
  debugging session*. Ephemeral and session-scoped.

Keeping them separate prevents task-specific chatter from polluting the durable
explanations an agent loads for context.

## Tags

Use structured, colon-prefixed tags for precise filtering. Apply multiple when
creating a bookmark; filter by one at a time with `--tag`:

| Prefix | Example | Meaning |
|--------|---------|---------|
| `feature:` | `feature:auth` | Feature or domain |
| `layer:` | `layer:api` | Architectural layer |
| `role:` | `role:entrypoint` | Responsibility |
| `type:` | `type:function` | Code-element kind |
| `task:` | `task:fix-123` | Work item |
| `pr:` / `issue:` | `pr:116` | Associated PR/issue |
| `security:` | `security:auth` | Security-sensitive code |
| `module:` / `crate:` / `package:` | `crate:auth` | Module/package context |

See the [Agent Skill](./agent-skills) page for the full taxonomy and per-language
module conventions.
