# Bookmark: {{short_id}}
{{#if tags}}
## Tags

{{#each tags}} #{{this}} {{/each}}
{{/if}}

{{#if annotations}}
## Notes

{{#each annotations}}
{{#if notes}}
{{escape_markdown notes}}

*— {{added_by}}{{#if source}} ({{source}}){{/if}}, {{format_date added_at "%Y-%m-%d %H:%M:%S"}}*

{{/if}}
{{#if context}}
**Context**
{{escape_markdown context}}

*— {{added_by}}{{#if source}} ({{source}}){{/if}}, {{format_date added_at "%Y-%m-%d %H:%M:%S"}}*

{{/if}}
{{/each}}
{{/if}}

## Metadata
| Property | Value |
|----------|-------|
| **File** | {{file_path}} |
| **Language** | {{language}} |
| **Status** | {{status}} |
| **Created** | {{format_date created_at "%Y-%m-%d %H:%M:%S"}} |
{{#if created_by}}| **Author** | {{escape_markdown created_by}} |{{/if}}
{{#if last_resolved_at}}| **Last Resolved** | {{format_date last_resolved_at "%Y-%m-%d %H:%M:%S"}} |{{/if}}
{{#if resolution_method}}| **Resolution Method** | {{resolution_method}} |{{/if}}
{{#if commit_hash}}| **Commit** | {{short_commit}} |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}


{{#if resolutions}}
## Resolution History

{{#each resolutions}}
Date: {{format_date resolved_at "%Y-%m-%d %H:%M:%S"}}
Method: {{method}}
Range: {{line_range}}
Commit: {{#if commit_hash}}{{short_commit}}{{else}}-{{/if}}
{{/each}}
{{/if}}

{{#if snapshot}}
## Snapshot

```
{{escape_markdown snapshot}}
```
{{/if}}

## Tree-sitter Query

```scm
{{escape_markdown query}}
```
