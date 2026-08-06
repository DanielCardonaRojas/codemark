# Handlebars Template Design

This document describes the Handlebars template system for codemark's markdown
output. Codemark uses [Handlebars](https://handlebarsjs.com/) templates to format
the output of commands like `codemark show` as well as the markdown previews shown
in the TUI.

## Customizing Templates

Templates resolve in priority order:

1. **User config directory** (highest priority):
   `~/.config/codemark/templates/<template>.md`
   - Create a file here to override the default. The directory is created
     automatically if it doesn't exist.
   - On macOS the directory is `~/Library/Application Support/codemark/templates/`;
     if `$XDG_CONFIG_HOME` is set it is `$XDG_CONFIG_HOME/codemark/templates/`.
2. **Compiled defaults** (fallback): the templates bundled in `./templates/`,
   shown in [Default Template](#default-template) below.

To create your own template, copy the bundled default and edit it:

```bash
mkdir -p ~/.config/codemark/templates
cp ./templates/codemark_show.md ~/.config/codemark/templates/
$EDITOR ~/.config/codemark/templates/codemark_show.md
```

> **Note:** Edited templates are cached. If a change doesn't appear at runtime,
> clear the on-disk cache (or unset/clean `XDG_CONFIG_HOME`).

## Template Placeholders

### Top-level Bookmark fields (`show` command)

| Placeholder | Type | Description |
|-------------|------|-------------|
| `{{short_id}}` | String | First 8 chars of bookmark ID (computed) |
| `{{id}}` | String | Full bookmark ID |
| `{{file_path}}` | String | Path to the file |
| `{{file_name}}` | String | Just the filename (computed) |
| `{{language}}` | String | Programming language |
| `{{status}}` | String | `active`, `drifted`, `stale`, or `archived` |
| `{{query}}` | String | Tree-sitter query |
| `{{created_at}}` | String | Creation timestamp |
| `{{created_by}}` | String? | Creator (optional) |
| `{{commit_hash}}` | String? | Git commit hash (optional) |
| `{{short_commit}}` | String? | First 8 chars of commit (computed) |
| `{{last_resolved_at}}` | String? | Last resolution time (optional) |
| `{{resolution_method}}` | String? | `exact`, `relaxed`, `hash_fallback`, `failed` (optional) |
| `{{stale_since}}` | String? | When it became stale (optional) |

### Tags (`{{#each tags}}` loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| `{{this}}` | String | Individual tag name |

### Annotations (`{{#each annotations}}` loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| `{{added_at}}` | String | When annotation was added |
| `{{added_by}}` | String? | Who added it |
| `{{source}}` | String? | Source (e.g., "annotate" command) |
| `{{notes}}` | String? | Annotation notes |
| `{{context}}` | String? | Code context snippet |

### Resolution History (`{{#each resolutions}}` loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| `{{resolved_at}}` | String | When resolution occurred |
| `{{method}}` | String | Resolution method |
| `{{file_path}}` | String? | Resolved file path |
| `{{line_range}}` | String? | Line range (e.g., "10-20") |
| `{{line_range_colon}}` | String? | Line range with colon for tools (e.g., "10:20") |
| `{{match_count}}` | Number? | Number of matches |
| `{{commit_hash}}` | String? | Resolution commit |
| `{{short_commit}}` | String? | First 8 chars (computed) |

### Collection Overview (`codemark_collection_overview.md`)

Rendered in the TUI right pane as a live preview while browsing the Collections
tab (before a collection is entered with Enter). Uses a different context than
the bookmark templates:

| Placeholder | Type | Description |
|-------------|------|-------------|
| `{{name}}` | String | Collection name |
| `{{description}}` | String? | Collection description |
| `{{visibility}}` | String | `public` or `private` |
| `{{health}}` | String? | `active`, `drifted`, or `stale` |
| `{{created_at}}` | String | Creation timestamp |
| `{{created_by}}` | String? | Creator |
| `{{branch}}` | String? | Branch the collection was created on |
| `{{published}}` | Bool | Whether the collection has been published |
| `{{published_at}}` | String? | Publish timestamp |
| `{{repo_url}}` | String? | Source repository URL |
| `{{step_count}}` | Number | Number of bookmarks in the collection |

Loops: `{{#each tags}}` (each `{{this}}`), `{{#each links}}` (each `{{kind}}`,
`{{label}}`, `{{url}}`), and `{{#each steps}}` (each `{{index}}`, `{{file_path}}`,
`{{file_name}}`, `{{language}}`, `{{summary}}`).

### Custom Helpers

- `{{escape_markdown value}}` - Escapes special markdown characters
- `{{truncate value}}` - Truncates a string to 8 characters
- `{{format_date value "%Y-%m-%d %H:%M:%S"}}` - Formats a timestamp

## Default Template

This is the default template used when no custom template is provided:

```handlebars
# Bookmark: {{short_id}}

## Metadata
| Property | Value |
|----------|-------|
| **File** | {{file_path}} |
| **Language** | {{language}} |
| **Status** | {{status}} |
| **Created** | {{created_at}} |
{{#if created_by}}| **Author** | {{escape_markdown created_by}} |{{/if}}
{{#if last_resolved_at}}| **Last Resolved** | {{last_resolved_at}} |{{/if}}
{{#if resolution_method}}| **Resolution Method** | {{resolution_method}} |{{/if}}
{{#if commit_hash}}| **Commit** | `{{short_commit}}` |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}

## Tree-sitter Query
```scheme
{{query}}
```

{{#if tags}}
## Tags
{{#each tags}}
- `{{escape_markdown this}}`
{{/each}}
{{/if}}

{{#if annotations}}
## Annotations
{{#each annotations}}
### {{added_by}}
*{{source}}* added: {{added_at}}

{{#if notes}}{{escape_markdown notes}}{{/if}}

{{#if context}}
```
{{escape_markdown context}}
```
{{/if}}
{{/each}}
{{/if}}

{{#if resolutions}}
## Resolution History
| Time | Method | File | Lines | Matches | Commit |
|------|--------|------|-------|---------|--------|
{{#each resolutions}}
| {{resolved_at}} | {{method}} | {{file_path}} | {{line_range}} | {{match_count}} | {{#if commit_hash}}`{{short_commit}}`{{else}}-{{/if}} |
{{/each}}
{{/if}}
```

## Template Storage

Templates are stored in `.codemark/templates/` directory:
- `codemark_show.md` - Template for `codemark show` command (default shown above)
- `details_panel.md` - Template for the TUI bottom Details pane (annotations/notes)
- `codemark_collection_overview.md` - Template for the TUI live collection overview
- `list.md` - Template for `codemark list` command (optional, simple format)

Users can override these by creating their own files in this directory.
