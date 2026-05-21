# Bookmark: {{short_id}}
{{#if tags}}
## Tags

{{#each tags}} #{{this}} {{/each}}
{{/if}}

{{#if annotations}}
{{#each annotations}}
{{#if context}}
{{escape_markdown context}}
{{/if}}
{{/each}}
{{/if}}

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
{{#if commit_hash}}| **Commit** | {{short_commit}} |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}


{{#if resolutions}}
## Resolution History

{{#each resolutions}}
Date: {{resolved_at}}
Method: {{method}}
Range: {{line_range}}
Commit: {{#if commit_hash}}{{short_commit}}{{else}}-{{/if}}
{{/each}}
{{/if}}
