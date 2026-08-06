# Handlebars Template Design

This document describes the Handlebars template system for codemark's markdown
output. Codemark uses [Handlebars](https://handlebarsjs.com/) templates to format
the output of commands like `codemark show` as well as the markdown previews shown
in the TUI.

## Customizing Templates

Templates resolve in priority order:

1. **User config directory** (highest priority):
   `~/.config/codemark/templates/&lt;template&gt;.md`
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
| <code v-pre>{{short_id}}</code> | String | First 8 chars of bookmark ID (computed) |
| <code v-pre>{{id}}</code> | String | Full bookmark ID |
| <code v-pre>{{file_path}}</code> | String | Path to the file |
| <code v-pre>{{file_name}}</code> | String | Just the filename (computed) |
| <code v-pre>{{language}}</code> | String | Programming language |
| <code v-pre>{{status}}</code> | String | `active`, `drifted`, `stale`, or `archived` |
| <code v-pre>{{query}}</code> | String | Tree-sitter query |
| <code v-pre>{{created_at}}</code> | String | Creation timestamp |
| <code v-pre>{{created_by}}</code> | String? | Creator (optional) |
| <code v-pre>{{commit_hash}}</code> | String? | Git commit hash (optional) |
| <code v-pre>{{short_commit}}</code> | String? | First 8 chars of commit (computed) |
| <code v-pre>{{last_resolved_at}}</code> | String? | Last resolution time (optional) |
| <code v-pre>{{resolution_method}}</code> | String? | `exact`, `relaxed`, `hash_fallback`, `failed` (optional) |
| <code v-pre>{{stale_since}}</code> | String? | When it became stale (optional) |

### Tags (<code v-pre>{{#each tags}}</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>{{this}}</code> | String | Individual tag name |

### Annotations (<code v-pre>{{#each annotations}}</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>{{added_at}}</code> | String | When annotation was added |
| <code v-pre>{{added_by}}</code> | String? | Who added it |
| <code v-pre>{{source}}</code> | String? | Source (e.g., "annotate" command) |
| <code v-pre>{{notes}}</code> | String? | Annotation notes |
| <code v-pre>{{context}}</code> | String? | Code context snippet |

### Resolution History (<code v-pre>{{#each resolutions}}</code> loop)

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>{{resolved_at}}</code> | String | When resolution occurred |
| <code v-pre>{{method}}</code> | String | Resolution method |
| <code v-pre>{{file_path}}</code> | String? | Resolved file path |
| <code v-pre>{{line_range}}</code> | String? | Line range (e.g., "10-20") |
| <code v-pre>{{line_range_colon}}</code> | String? | Line range with colon for tools (e.g., "10:20") |
| <code v-pre>{{match_count}}</code> | Number? | Number of matches |
| <code v-pre>{{commit_hash}}</code> | String? | Resolution commit |
| <code v-pre>{{short_commit}}</code> | String? | First 8 chars (computed) |

### Collection Overview (`codemark_collection_overview.md`)

Rendered in the TUI right pane as a live preview while browsing the Collections
tab (before a collection is entered with Enter). Uses a different context than
the bookmark templates:

| Placeholder | Type | Description |
|-------------|------|-------------|
| <code v-pre>{{name}}</code> | String | Collection name |
| <code v-pre>{{description}}</code> | String? | Collection description |
| <code v-pre>{{visibility}}</code> | String | `public` or `private` |
| <code v-pre>{{health}}</code> | String? | `active`, `drifted`, or `stale` |
| <code v-pre>{{created_at}}</code> | String | Creation timestamp |
| <code v-pre>{{created_by}}</code> | String? | Creator |
| <code v-pre>{{branch}}</code> | String? | Branch the collection was created on |
| <code v-pre>{{published}}</code> | Bool | Whether the collection has been published |
| <code v-pre>{{published_at}}</code> | String? | Publish timestamp |
| <code v-pre>{{repo_url}}</code> | String? | Source repository URL |
| <code v-pre>{{step_count}}</code> | Number | Number of bookmarks in the collection |

Loops: <code v-pre>{{#each tags}}</code> (each <code v-pre>{{this}}</code>), <code v-pre>{{#each links}}</code> (each <code v-pre>{{kind}}</code>,
<code v-pre>{{label}}</code>, <code v-pre>{{url}}</code>), and <code v-pre>{{#each steps}}</code> (each <code v-pre>{{index}}</code>, <code v-pre>{{file_path}}</code>,
<code v-pre>{{file_name}}</code>, <code v-pre>{{language}}</code>, <code v-pre>{{summary}}</code>).

### Custom Helpers

- <code v-pre>{{escape_markdown value}}</code> - Escapes special markdown characters
- <code v-pre>{{truncate value}}</code> - Truncates a string to 8 characters
- <code v-pre>{{format_date value "%Y-%m-%d %H:%M:%S"}}</code> - Formats a timestamp

## Default Template

This is the default template used when no custom template is provided:

````handlebars v-pre
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
````

## Template Storage

Templates are stored in `.codemark/templates/` directory:
- `codemark_show.md` - Template for `codemark show` command (default shown above)
- `details_panel.md` - Template for the TUI bottom Details pane (annotations/notes)
- `codemark_collection_overview.md` - Template for the TUI live collection overview
- `list.md` - Template for `codemark list` command (optional, simple format)

Users can override these by creating their own files in this directory.
