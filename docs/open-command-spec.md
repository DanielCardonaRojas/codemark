# Open Command Specification

## Overview

The `codemark open` command allows users to quickly open bookmarked files in their preferred editor. It resolves the bookmark to find the current file location and line numbers, then executes a configurable editor command.

## Configuration

The open command is configured via the `[open]` section in `codemark.toml`:

```toml
[open]
# The default command to run if no extension matches.
# Supports placeholder substitution (see below).
default = "xed --line {LINE_START} {FILE}"

# Extension-specific overrides. These take precedence over the default.
[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
md = "typora {FILE}"
py = "code --goto {FILE}:{LINE_START}:{LINE_END}"
swift = "xed --line {LINE_START} {FILE}"
ts = "code --goto {FILE}:{LINE_START}:{LINE_END}"
```

### Default Behavior

If no `[open]` configuration is present, the command uses sensible defaults:
- Checks the `$EDITOR` environment variable
- Falls back to `vim` if `$EDITOR` is not set
- Default format: `{EDITOR} +{LINE_START} {FILE}`

## Placeholders

The following placeholders are available in command templates:

| Placeholder | Description | Example |
|-------------|-------------|---------|
| `{FILE}` | Absolute path to the file | `/Users/user/project/src/main.rs` |
| `{LINE_START}` | Starting line number (1-indexed) | `42` |
| `{LINE_END}` | Ending line number (1-indexed) | `67` |
| `{ID}` | Bookmark ID | `a1b2c3d4-...` |

### Notes

- Line numbers are 1-indexed (matching standard editor conventions)
- `{LINE_END}` may equal `{LINE_START}` for single-line bookmarks
- Paths are always absolute (relative paths from bookmarks are resolved against the repo root)

## Usage

```bash
# Open a bookmark by ID (or unambiguous prefix)
codemark open <id>

# Examples
codemark open a1b2c3d4
codemark open a1b2  # prefix match
```

## Editor Command Examples

### Xcode (macOS)
```toml
default = "xed --line {LINE_START} {FILE}"
```

### VS Code
```toml
default = "code --goto {FILE}:{LINE_START}:{LINE_END}"
```

### Neovim / Vim
```toml
default = "nvim +{LINE_START} {FILE}"
# or
default = "vim +{LINE_START} {FILE}"
```

### IntelliJ IDEA
```toml
default = "idea --line {LINE_START} {FILE}"
```

### Sublime Text
```toml
default = "subl {FILE}:{LINE_START}"
```

### Typora (for Markdown)
```toml
[open.extensions]
md = "typora {FILE}"
```

## Implementation Notes

### Command Execution

- Commands are tokenized using `shlex` for safe shell parsing
- The editor process is spawned directly (not via shell) for security
- The command inherits stdin/stdout/stderr from the parent process
- The command blocks until the editor exits (for GUI editors, this typically returns immediately)

### Extension Matching

- File extensions are extracted from the bookmark's file path
- Matching is case-insensitive (e.g., `.MD` matches `md`)
- If no extension match is found, the `default` command is used
- Files without extensions use the `default` command

### Resolution

The open command first resolves the bookmark to find its current location:
1. Runs the bookmark through the standard resolution process
2. Extracts the resolved file path and line range
3. Substitutes placeholders in the command template
4. Spawns the editor process

If resolution fails, the command exits with an error message indicating the failure reason.

### Error Handling

- Invalid bookmark ID: exits with error
- Ambiguous prefix: exits with error showing matching IDs
- Failed resolution: exits with error explaining the failure
- File not found: still attempts to open (editor may show an error)
- Command not found: exits with error indicating the editor wasn't found
