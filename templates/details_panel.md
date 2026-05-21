{{#if annotations}}
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
{{else}}
*No annotations*

{{/if}}
