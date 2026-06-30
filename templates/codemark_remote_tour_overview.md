# {{escape_markdown title}}

A remote tour available on the server. Pull it to browse its steps locally.

## Overview
| Property | Value |
|----------|-------|
{{#if author}}
| **Author** | {{escape_markdown author}} |
{{/if}}
{{#if repo_url}}
| **Repo** | [{{escape_markdown repo_url}}]({{repo_url}}) |
{{/if}}
{{#if updated_at}}
| **Updated** | {{format_date updated_at "%Y-%m-%d %H:%M:%S"}} |
{{/if}}
