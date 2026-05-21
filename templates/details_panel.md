| Property | Value |
|----------|-------|
| **ID** | `{{short_id}}` |
{{#if created_by}}| **Author** | {{escape_markdown created_by}} |{{/if}}
| **Health** | {{status}} |
{{#if commit_hash}}| **Commit** | `{{short_commit}}` |{{/if}}
| **Created** | {{created_at}} |
{{#if tags}}
| **Tags** | {{#each tags}}#{{this}} {{/each}} |
{{/if}}
