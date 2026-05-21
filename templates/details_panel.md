{{#if annotations}}
{{#each annotations}}
{{#if notes}}
{{notes}}

{{/if}}

{{#if context}}
**Context**
{{escape_markdown context}}
{{/if}}

{{/each}}
{{else}}
*No annotations*

{{/if}}
