# Codemark: Bookmark Creation Improvements

## Problem

The current `codemark add --range` flag accepts **byte ranges** (`--range 1024:1280`). This is an implementation detail that's unfriendly to both humans and tools:

- **Humans** think in line numbers, not byte offsets. Nobody runs `wc -c` to figure out where a function starts.
- **Editors** report cursor positions as `file:line:col`, not byte offsets.
- **Git diff** produces hunk headers like `@@ -42,7 +42,9 @@` — line-based ranges.
- **There's no way to preview** what a range will target before committing the bookmark.

## Goals

1. Accept **line ranges** as the primary input format (byte ranges remain available for agents/tooling).
2. Provide a **dry-run mode** that shows exactly what would be bookmarked without writing to the database.
3. Support **diff hunk ranges** so bookmarks can be created directly from `git diff` output.
4. **Auto-detect language** from file extension when `--lang` is omitted.

## Non-Goals

- Changing the storage format (queries and byte ranges are still stored internally as before).
- Adding interactive/TUI selection of nodes (that's a separate feature).

---

## FR-10: Range Format Improvements

### FR-10.1: Line range input

**`--range` accepts both line ranges and byte ranges**, auto-detected by format:

| Format | Interpretation | Example |
|--------|---------------|---------|
| `42` | Single line | Line 42 |
| `42:67` | Line range (inclusive) | Lines 42 through 67 |
| `42:67b` | Byte range (explicit) | Bytes 42 through 67 |
| `b42:67` | Byte range (explicit) | Bytes 42 through 67 |

**Default is line range.** The `b` suffix/prefix opts into byte mode for backwards compatibility and agent use.

**Conversion**: Line ranges are converted to byte ranges at parse time by reading the file and computing byte offsets for the start of the first line and end of the last line.

**Single line**: `--range 42` targets line 42. The engine finds the smallest declaration containing that line — same `walk_to_named_declaration` behavior as today.

**Examples**:
```bash
# Bookmark the function at line 42 (human-friendly)
codemark add --file src/auth.swift --range 42 --lang swift --note "auth entry"

# Bookmark lines 42-67 (function body)
codemark add --file src/auth.swift --range 42:67 --lang swift

# Byte range (agent use, backwards compatible)
codemark add --file src/auth.swift --range b1024:1280 --lang swift
```

### FR-10.2: Dry-run mode

**`--dry-run` flag** on `add` and `add-from-snippet` shows what would be bookmarked without writing to the database.

**Output** (table mode):
```
Dry run — bookmark would target:

  Node type:  function_declaration
  Name:       validateToken
  File:       src/auth/middleware.swift
  Lines:      42-67
  Query:      (class_declaration
                name: (type_identifier) @name0
                (#eq? @name0 "AuthService")
                (class_body
                  (function_declaration
                    name: (simple_identifier) @fn_name
                    (#eq? @fn_name "validateToken")) @target))
  Hash:       sha256:abcd1234abcd1234
  Unique:     yes (1 match in file)

No bookmark created. Remove --dry-run to save.
```

**JSON output** (`--dry-run --json`): Same structure as a normal `add` response but with `"dry_run": true` and no `id` field.

**Use case**: Verify that a line range targets the intended node before committing. Especially valuable when:
- The line range spans part of a function and part of another — which one gets bookmarked?
- The line is inside a nested structure — does it target the inner or outer declaration?
- Checking that the generated query is unique in the file.

### FR-10.3: Diff hunk range input

**`--hunk` flag** accepts a git diff hunk header and extracts the target file and line range.

```bash
# From a hunk header
codemark add --hunk "@@ -42,7 +42,9 @@ func validateToken" --file src/auth.swift --lang swift

# Pipe from git diff
git diff HEAD~1 --unified=0 | grep "^@@" | head -1 | xargs -I{} codemark add --hunk {} --file src/auth.swift --lang swift
```

**Hunk format**: `@@ -old_start,old_count +new_start,new_count @@ [context]`

The `--hunk` flag parses the **new file** side (`+new_start,new_count`) to derive a line range:
- Start line: `new_start`
- End line: `new_start + new_count - 1`

If the hunk header includes a trailing context (function name after `@@`), it's used as a hint for disambiguation but not required.

**`--file` is still required** when using `--hunk` because hunk headers don't include the full file path.

**`--range` and `--hunk` are mutually exclusive.**

### FR-10.4: Auto-detect language

When `--lang` is omitted, infer the language from the file extension:

| Extension | Language |
|-----------|----------|
| `.swift` | swift |
| `.rs` | rust |
| `.ts`, `.tsx` | typescript |
| `.py` | python |

If the extension is unrecognized or ambiguous, error with: `cannot infer language from extension '.xyz'; use --lang to specify`.

`--lang` always takes precedence over auto-detection.

---

## CLI Specification Updates

### `codemark add` (updated)

```
codemark add --file <path> --range <lines-or-bytes>
             [--lang <language>]
             [--tag <tag>]... [--note "annotation"] [--context "..."]
             [--dry-run]

codemark add --file <path> --hunk "@@ ... @@"
             [--lang <language>]
             [--tag <tag>]... [--note "annotation"] [--context "..."]
             [--dry-run]
```

| Flag        | Required | Description |
|-------------|----------|-------------|
| `--file`    | Yes      | Path to the file |
| `--range`   | Yes*     | Line range (`42`, `42:67`) or byte range (`b42:67`). *Mutually exclusive with `--hunk` |
| `--hunk`    | Yes*     | Git diff hunk header (`@@ -a,b +c,d @@`). *Mutually exclusive with `--range` |
| `--lang`    | No       | Language identifier; auto-detected from extension if omitted |
| `--tag`     | No       | Tag label; repeatable |
| `--note`    | No       | Semantic annotation |
| `--context` | No       | Agent context |
| `--dry-run` | No       | Preview what would be bookmarked without saving |

### `codemark add-from-snippet` (updated)

```
codemark add-from-snippet --file <path>
             [--lang <language>]
             [--tag <tag>]... [--note "annotation"] [--context "..."]
             [--dry-run]
```

`--lang` becomes optional (auto-detected from `--file` extension).
`--dry-run` shows the matched node and generated query without saving.

---

## Implementation Notes

### Line-to-byte conversion

```rust
fn line_range_to_bytes(source: &str, start_line: usize, end_line: usize) -> (usize, usize) {
    let mut byte_offset = 0;
    let mut start_byte = 0;
    let mut end_byte = source.len();
    for (i, line) in source.lines().enumerate() {
        let line_num = i + 1; // 1-indexed
        if line_num == start_line {
            start_byte = byte_offset;
        }
        byte_offset += line.len() + 1; // +1 for newline
        if line_num == end_line {
            end_byte = byte_offset;
            break;
        }
    }
    (start_byte, end_byte)
}
```

### Hunk parsing

```rust
fn parse_hunk(hunk: &str) -> Result<(usize, usize)> {
    // Parse: @@ -old_start[,old_count] +new_start[,new_count] @@ [context]
    // Extract new_start and new_count
    let re = regex::Regex::new(r"\+(\d+)(?:,(\d+))?").unwrap();
    let caps = re.captures(hunk).ok_or("invalid hunk format")?;
    let start: usize = caps[1].parse()?;
    let count: usize = caps.get(2).map(|m| m.as_str().parse().unwrap()).unwrap_or(1);
    Ok((start, start + count - 1))
}
```

### Auto-detect language

Extend `Language::from_extension(ext: &str) -> Option<Language>` in `src/parser/languages.rs`. Called from the handler when `--lang` is `None`.

### Dry-run flow

Same as the normal `add` flow up to and including query generation + uniqueness validation. Skip the `db.insert_bookmark()` call. Print the dry-run output instead.

---

## Scope

This is a Phase 2 improvement. Estimated ~200 lines of Rust:
- Range parsing updates: ~50 lines
- Hunk parsing: ~30 lines
- Dry-run output: ~50 lines
- Auto-detect language: ~20 lines
- CLI flag changes: ~30 lines
- Tests: ~50 lines

No schema changes. No new dependencies (regex is already in the project).
