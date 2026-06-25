{{#if comments}}
{{#each comments}}
**{{author}}** - *{{format_date created_at "%Y-%m-%d %H:%M"}}*

{{body}}

---
{{/each}}
{{else}}
_No comments._
{{/if}}
