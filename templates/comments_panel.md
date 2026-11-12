{{#if comments}}
{{#each comments}}
{{#if parent_id}}
> ↳ **{{author}}** · *{{format_date created_at "%Y-%m-%d %H:%M"}}*
>
{{body_quoted}}

{{else}}
{{#unless @first}}
---

{{/unless}}
**{{author}}** · *{{format_date created_at "%Y-%m-%d %H:%M"}}*{{#if anchor_hint}} · on `{{anchor_hint}}`{{/if}}

{{body}}

{{/if}}
{{/each}}
{{else}}
_No comments._
{{/if}}
