# Agent Workflow Walkthrough

This document walks through a realistic multi-session agent interaction with Codemark, showing the exact CLI commands at each step. Use this to validate the API surface and as README documentation.

## Scenario

An AI coding agent is working on an iOS app. Across three sessions it: (1) explores and bookmarks code during a feature build, (2) resumes work using saved context, and (3) handles code that drifted between sessions.

---

## Session 1: Building a Feature

The agent is tasked with adding rate limiting to the API client.

### Step 1: Agent explores the codebase and bookmarks key code

```bash
# Agent finds the main API client and bookmarks it
codemark add --file src/networking/APIClient.swift --range 1024:1280 --lang swift \
  --tag api --tag networking \
  --note "Main API request dispatcher — all requests flow through here"
```

```json
{
  "success": true,
  "data": {
    "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "query": "(class_declaration name: (type_identifier) @_cls (#eq? @_cls \"APIClient\") (class_body (function_declaration name: (simple_identifier) @_fn (#eq? @_fn \"sendRequest\")) @target))",
    "node_type": "function_declaration",
    "range": { "start": { "line": 42, "col": 4 }, "end": { "line": 67, "col": 5 } },
    "content_hash": "sha256:abcdef..."
  }
}
```

### Step 2: Agent bookmarks related code via snippet

```bash
# Agent found the retry logic and bookmarks it by snippet
echo 'func retryWithBackoff(_ request: URLRequest, attempts: Int) async throws -> Data' | \
  codemark add-from-snippet --lang swift --file src/networking/APIClient.swift \
  --tag api --tag retry \
  --note "Retry logic with exponential backoff — rate limiter should wrap this"
```

### Step 3: Agent bookmarks the configuration

```bash
codemark add --file src/networking/NetworkConfig.swift --range 256:512 --lang swift \
  --tag api --tag config \
  --note "Timeout and retry configuration — add rate limit settings here"
```

### Step 4: Agent groups bookmarks into a collection

```bash
# Create a collection for this feature
codemark collection create rate-limiting --description "Rate limiting feature for API client"

# Add all related bookmarks
codemark collection add rate-limiting a1b2c3d4 b2c3d4e5 c3d4e5f6
```

### Step 5: Agent does its work

The agent implements the rate limiting feature, modifying the bookmarked files.

### End of session

```bash
# Validate all bookmarks (could be triggered by a session-end hook)
codemark validate
```

```
3 active  |  0 drifted  |  0 stale  |  0 archived
```

---

## Session 2: Resuming Work (Next Day)

A new agent session starts. The agent needs to continue work on rate limiting.

### Step 1: Load context from the collection

```bash
# Resolve all bookmarks in the collection to get current locations
codemark collection resolve rate-limiting --json
```

```json
{
  "success": true,
  "data": [
    {
      "id": "a1b2c3d4",
      "file": "src/networking/APIClient.swift",
      "line": 42,
      "column": 4,
      "method": "exact",
      "status": "active",
      "note": "Main API request dispatcher — all requests flow through here",
      "tags": ["api", "networking"]
    },
    {
      "id": "b2c3d4e5",
      "file": "src/networking/APIClient.swift",
      "line": 89,
      "column": 4,
      "method": "exact",
      "status": "active",
      "note": "Retry logic with exponential backoff — rate limiter should wrap this",
      "tags": ["api", "retry"]
    },
    {
      "id": "c3d4e5f6",
      "file": "src/networking/NetworkConfig.swift",
      "line": 15,
      "column": 4,
      "method": "exact",
      "status": "active",
      "note": "Timeout and retry configuration — add rate limit settings here",
      "tags": ["api", "config"]
    }
  ]
}
```

The agent now has file:line locations + semantic context without re-exploring the codebase.

### Step 2: Agent searches for related bookmarks

```bash
# Search across all bookmarks, not just the collection
codemark search "request" --json
```

### Step 3: Agent adds more bookmarks as it works

```bash
# Agent discovers the test file and bookmarks it
codemark add --file tests/APIClientTests.swift --range 2048:2560 --lang swift \
  --tag api --tag tests \
  --note "Existing request tests — add rate limiting test cases here"

# Add to the collection
codemark collection add rate-limiting d4e5f6a7
```

---

## Session 3: Code Drifted Between Sessions

Between sessions, another developer refactored the networking layer:
- Renamed `APIClient.swift` to `NetworkClient.swift`
- Extracted retry logic into a new `RetryHandler.swift`
- Reformatted `NetworkConfig.swift` with a linter

### Step 1: Agent loads context — resolution shows drift

```bash
codemark collection resolve rate-limiting --json
```

```json
{
  "success": true,
  "data": [
    {
      "id": "a1b2c3d4",
      "file": "src/networking/NetworkClient.swift",
      "line": 38,
      "column": 4,
      "method": "hash_fallback",
      "status": "drifted",
      "note": "Main API request dispatcher — all requests flow through here",
      "tags": ["api", "networking"]
    },
    {
      "id": "b2c3d4e5",
      "file": "src/networking/RetryHandler.swift",
      "line": 12,
      "column": 4,
      "method": "hash_fallback",
      "status": "drifted",
      "note": "Retry logic with exponential backoff — rate limiter should wrap this",
      "tags": ["api", "retry"]
    },
    {
      "id": "c3d4e5f6",
      "file": "src/networking/NetworkConfig.swift",
      "line": 15,
      "column": 4,
      "method": "relaxed",
      "status": "drifted",
      "note": "Timeout and retry configuration — add rate limit settings here",
      "tags": ["api", "config"]
    }
  ]
}
```

Key observations:
- `a1b2c3d4`: File was renamed. Hash fallback found the code in `NetworkClient.swift`. Query was regenerated for the new location.
- `b2c3d4e5`: Function was extracted to a new file. Hash fallback found it in `RetryHandler.swift`.
- `c3d4e5f6`: Code was reformatted. Whitespace-normalized content hash still matched. Relaxed query resolved it.

The agent knows exactly where everything moved — no manual searching needed.

### Step 2: Agent checks what changed

```bash
# What bookmarks were affected by recent commits?
codemark diff --since HEAD~5 --json
```

### Step 3: Agent validates and heals

```bash
# After resolution, queries are regenerated for drifted bookmarks
# Running heal updates statuses and records new resolutions
codemark heal --collection rate-limiting --json

# To validate without recording resolution history
codemark heal --validate-only --collection rate-limiting --json
```

---

## Quick Reference: Agent Commands by Phase

### Starting a session
```bash
# Load all active bookmarks
codemark resolve --status active --json

# Load a specific collection
codemark collection resolve <name> --json

# Load bookmarks for a specific area
codemark list --tag <tag> --json
codemark resolve --tag <tag> --json
```

### During a session
```bash
# Bookmark code the agent is working with
codemark add --file <path> --range <start:end> --lang <lang> \
  --tag <tag> --note "<why this matters>" --json

# Bookmark by snippet (useful when agent has code in context but not byte ranges)
echo '<code snippet>' | codemark add-from-snippet --lang <lang> --file <path> \
  --tag <tag> --note "<why>" --json

# Group related bookmarks
codemark collection create <name> --description "<purpose>"
codemark collection add <name> <id1> <id2> ...

# Search existing bookmarks
codemark search "<query>" --json
```

### Ending a session
```bash
codemark validate --auto-archive
```
