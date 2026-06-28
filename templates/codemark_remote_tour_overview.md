# {{title}}

A remote tour available on the server. Pull it to browse its steps locally.

## Overview
| Property | Value |
|----------|-------|
{{#if author}}
| **Author** | {{author}} |
{{/if}}
{{#if repo_url}}
| **Repo** | {{repo_url}} |
{{/if}}
{{#if updated_at}}
| **Updated** | {{format_date updated_at "%Y-%m-%d %H:%M:%S"}} |
{{/if}}
