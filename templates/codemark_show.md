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
{{#if comments}}
## Comments

{{#each comments}}
{{body}}

*— {{author}}, {{format_date created_at "%Y-%m-%d %H:%M:%S"}}*

{{/each}}
{{/if}}

## Metadata
| Property | Value |
|----------|-------|
| **File** | {{file_path}} |
| **Language** | {{language}} |
| **Status** | {{status}} |
{{#if ui_status}}
| **UI Status** | {{ui_status}} |
{{/if}}
| **Created** | {{format_date created_at "%Y-%m-%d %H:%M:%S"}} |
{{#if created_by}}
| **Author** | {{escape_markdown created_by}} |
{{/if}}
{{#if current_resolution_id}}
| **Current Resolution** | {{current_resolution_id}} |
{{/if}}
{{#if last_resolved_at}}
| **Last Resolved** | {{format_date last_resolved_at "%Y-%m-%d %H:%M:%S"}} |
{{/if}}
{{#if resolution_method}}
| **Resolution Method** | {{resolution_method}} |
{{/if}}
{{#if commit_hash}}
| **Commit** | {{short_commit}} |
{{/if}}
{{#if stale_since}}
| **Stale Since** | {{stale_since}} |
{{/if}}


{{#if resolutions}}
## Resolution History

{{#each resolutions}}
{{#if is_current}}**[CURRENT]** {{/if}}**ID:** {{id}}
**Date:** {{format_date resolved_at "%Y-%m-%d %H:%M:%S"}}
**Status:** {{status}}{{#if ui_status}}
**UI Status:** {{ui_status}}{{/if}}
**Method:** {{method}}
**Anchored:** {{#if is_anchored}}Yes{{else}}No{{/if}}
**Range:** {{#if line_range}}{{line_range}}{{/if}}
{{#if match_count}}**Matches:** {{match_count}}{{/if}}
{{#if commit_hash}}**Commit:** {{short_commit}}{{/if}}

{{/each}}
{{/if}}

