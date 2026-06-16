# {{name}}
{{#if description}}

{{escape_markdown description}}
{{/if}}

## Overview
| Property | Value |
|----------|-------|
| **Steps** | {{step_count}} |
{{#if branch}}
| **Branch** | {{branch}} |
{{/if}}
| **Visibility** | {{visibility}} |
{{#if health}}
| **Health** | {{health}} |
{{/if}}
| **Created** | {{format_date created_at "%Y-%m-%d %H:%M:%S"}} |
{{#if created_by}}
| **Author** | {{escape_markdown created_by}} |
{{/if}}
{{#if published}}
| **Published** | {{#if published_at}}{{format_date published_at "%Y-%m-%d %H:%M:%S"}}{{else}}yes{{/if}} |
{{/if}}
{{#if repo_url}}
| **Repo** | {{repo_url}} |
{{/if}}

{{#if tags}}
## Tags

{{#each tags}} #{{this}} {{/each}}
{{/if}}

{{#if links}}
## Links

{{#each links}}
- **{{kind}}**: {{escape_markdown label}} ({{url}})
{{/each}}
{{/if}}

{{#if steps}}
## Steps

{{#each steps}}
- **{{index}}.** `{{file_name}}`{{#if summary}} — {{escape_markdown summary}}{{/if}}
{{/each}}
{{/if}}
