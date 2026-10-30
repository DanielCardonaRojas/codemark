{{#if breadcrumbs}}
**Context**
{{#each breadcrumbs}}
`{{line}}` {{text}}
{{/each}}

{{/if}}
{{#if annotations}}
{{#each annotations}}
{{#if notes}}
{{notes}}
{{/if}}

{{#if context}}
**Context**
{{context}}
{{/if}}

{{/each}}
{{/if}}
