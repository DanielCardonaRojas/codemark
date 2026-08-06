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
| <code v-pre>&#123;&#123;short_id&#125;&#125;</code> | String | First 8 chars of bookmark ID (computed) |
| <code v-pre>&#123;&#123;id&#125;&#125;</code> | String | Full bookmark ID |
| <code v-pre>&#123;&#123;file_path&#125;&#125;</code> | String | Path to the file |
| <code v-pre>&#123;&#123;file_name&#125;&#125;</code> | String | Just the filename (computed) |
| <code v-pre>&#123;&#123;language&#125;&#125;</code> | String | Programming language |
| <code v-pre>&#123;&#123;status&#125;&#125;</code> | String | `active`, `drifted`, `stale`, or `archived` |
| <code v-pre>&#123;&#123;query&#125;&#125;</code> | String | Tree-sitter query |
| <code v-pre>&#123;&#123;created_at&#125;&#125;</code> | String | Creation timestamp |
| <code v-pre>&#123;&#123;created_by&#125;&#125;</code> | String? | Creator (optional) |
| <code v-pre>&#123;&#123;commit_hash&#125;&#125;</code> | String? | Git commit hash (optional) |
| <code v-pre>&#123;&#123;short_commit&#125;&#125;</code> | String? | First 8 chars of commit (computed) |
| <code v-pre>&#123;&#123;last_resolved_at&#125;&#125;</code> | String? | Last resolution time (optional) |
| <code v-pre>&#123;&#123;resolution_method&#125;&#125;</code> | String? | `exact`, `relaxed`, `hash_fallback`, `failed` (optional) |
| <code v-pre>&#123;&#123;stale_since&#125;&#125;</code> | String? | When it became stale (optional) |

### Tags (<code v-pre>&#123;&#123;#each tags&#125;&#125;</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>&#123;&#123;this&#125;&#125;</code> | String | Individual tag name |

### Annotations (<code v-pre>&#123;&#123;#each annotations&#125;&#125;</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>&#123;&#123;added_at&#125;&#125;</code> | String | When annotation was added |
| <code v-pre>&#123;&#123;added_by&#125;&#125;</code> | String? | Who added it |
| <code v-pre>&#123;&#123;source&#125;&#125;</code> | String? | Source (e.g., "annotate" command) |
| <code v-pre>&#123;&#123;notes&#125;&#125;</code> | String? | Annotation notes |
| <code v-pre>&#123;&#123;context&#125;&#125;</code> | String? | Code context snippet |

### Resolution History (<code v-pre>&#123;&#123;#each resolutions&#125;&#125;</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>&#123;&#123;resolved_at&#125;&#125;</code> | String | When resolution occurred |
| <code v-pre>&#123;&#123;method&#125;&#125;</code> | String | Resolution method |
| <code v-pre>&#123;&#123;file_path&#125;&#125;</code> | String? | Resolved file path |
| <code v-pre>&#123;&#123;line_range&#125;&#125;</code> | String? | Line range (e.g., "10-20") |
| <code v-pre>&#123;&#123;line_range_colon&#125;&#125;</code> | String? | Line range with colon for tools (e.g., "10:20") |
| <code v-pre>&#123;&#123;match_count&#125;&#125;</code> | Number? | Number of matches |
| <code v-pre>&#123;&#123;commit_hash&#125;&#125;</code> | String? | Resolution commit |
| <code v-pre>&#123;&#123;short_commit&#125;&#125;</code> | String? | First 8 chars (computed) |

### Collection Overview (`codemark_collection_overview.md`)

Rendered in the TUI right pane as a live preview while browsing the Collections
tab (before a collection is entered with Enter). Uses a different context than
the bookmark templates:

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>&#123;&#123;name&#125;&#125;</code> | String | Collection name |
| <code v-pre>&#123;&#123;description&#125;&#125;</code> | String? | Collection description |
| <code v-pre>&#123;&#123;visibility&#125;&#125;</code> | String | `public` or `private` |
| <code v-pre>&#123;&#123;health&#125;&#125;</code> | String? | `active`, `drifted`, or `stale` |
| <code v-pre>&#123;&#123;created_at&#125;&#125;</code> | String | Creation timestamp |
| <code v-pre>&#123;&#123;created_by&#125;&#125;</code> | String? | Creator |
| <code v-pre>&#123;&#123;branch&#125;&#125;</code> | String? | Branch the collection was created on |
| <code v-pre>&#123;&#123;published&#125;&#125;</code> | Bool | Whether the collection has been published |
| <code v-pre>&#123;&#123;published_at&#125;&#125;</code> | String? | Publish timestamp |
| <code v-pre>&#123;&#123;repo_url&#125;&#125;</code> | String? | Source repository URL |
| <code v-pre>&#123;&#123;step_count&#125;&#125;</code> | Number | Number of bookmarks in the collection |

Loops: <code v-pre>&#123;&#123;#each tags&#125;&#125;</code> (each <code v-pre>&#123;&#123;this&#125;&#125;</code>), <code v-pre>&#123;&#123;#each links&#125;&#125;</code> (each <code v-pre>&#123;&#123;kind&#125;&#125;</code>,
<code v-pre>&#123;&#123;label&#125;&#125;</code>, <code v-pre>&#123;&#123;url&#125;&#125;</code>), and <code v-pre>&#123;&#123;#each steps&#125;&#125;</code> (each <code v-pre>&#123;&#123;index&#125;&#125;</code>, <code v-pre>&#123;&#123;file_path&#125;&#125;</code>,
<code v-pre>&#123;&#123;file_name&#125;&#125;</code>, <code v-pre>&#123;&#123;language&#125;&#125;</code>, <code v-pre>&#123;&#123;summary&#125;&#125;</code>).

### Custom Helpers

- <code v-pre>&#123;&#123;escape_markdown value&#125;&#125;</code> - Escapes special markdown characters
- <code v-pre>&#123;&#123;truncate value&#125;&#125;</code> - Truncates a string to 8 characters
- <code v-pre>&#123;&#123;format_date value "%Y-%m-%d %H:%M:%S"&#125;&#125;</code> - Formats a timestamp

## Default Template

This is the default template used when no custom template is provided:

::: raw
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
{{#if commit_hash}}| **Commit** | <code v-pre>{{short_commit}}</code> |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}

## Tree-sitter Query
```
:::scheme
&#123;&#123;query&#125;&#125;
```

&#123;&#123;#if tags&#125;&#125;
## Tags
&#123;&#123;#each tags&#125;&#125;
- <code v-pre>&#123;&#123;escape_markdown this&#125;&#125;</code>
&#123;&#123;/each&#125;&#125;
&#123;&#123;/if&#125;&#125;

&#123;&#123;#if annotations&#125;&#125;
## Annotations
&#123;&#123;#each annotations&#125;&#125;
### &#123;&#123;added_by&#125;&#125;
*&#123;&#123;source&#125;&#125;* added: &#123;&#123;added_at&#125;&#125;

&#123;&#123;#if notes&#125;&#125;&#123;&#123;escape_markdown notes&#125;&#125;&#123;&#123;/if&#125;&#125;

&#123;&#123;#if context&#125;&#125;
```
&#123;&#123;escape_markdown context&#125;&#125;
```
&#123;&#123;/if&#125;&#125;
&#123;&#123;/each&#125;&#125;
&#123;&#123;/if&#125;&#125;

&#123;&#123;#if resolutions&#125;&#125;
## Resolution History
| Time | Method | File | Lines | Matches | Commit |
|------|--------|------|-------|---------|--------|
&#123;&#123;#each resolutions&#125;&#125;
| &#123;&#123;resolved_at&#125;&#125; | &#123;&#123;method&#125;&#125; | &#123;&#123;file_path&#125;&#125; | &#123;&#123;line_range&#125;&#125; | &#123;&#123;match_count&#125;&#125; | &#123;&#123;#if commit_hash&#125;&#125;<code v-pre>&#123;&#123;short_commit&#125;&#125;</code>&#123;&#123;else&#125;&#125;-&#123;&#123;/if&#125;&#125; |
&#123;&#123;/each&#125;&#125;
&#123;&#123;/if&#125;&#125;
```

## Template Storage

Templates are stored in `.codemark/templates/` directory:
- `codemark_show.md` - Template for `codemark show` command (default shown above)
- `details_panel.md` - Template for the TUI bottom Details pane (annotations/notes)
- `codemark_collection_overview.md` - Template for the TUI live collection overview
- `list.md` - Template for `codemark list` command (optional, simple format)

Users can override these by creating their own files in this directory.
