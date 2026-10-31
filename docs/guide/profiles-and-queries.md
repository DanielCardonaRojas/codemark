# Profiles and Queries

A bookmark is a tree-sitter query, not a line number. [Query
Generation](./query-generation) explains how that query is built; this page
explains the other half — **where the language-specific knowledge comes from**,
and how a `manifest.json` in the
[grammar registry](https://github.com/DanielCardonaRojas/codemark-grammars)
changes what the generator produces.

Read this if you are adding a language, debugging a bookmark that resolves
oddly, or wondering why a breadcrumb is empty.

## The problem a profile solves

Tree-sitter grammars do not agree on anything. The same idea — "a function" —
is a different node kind in every language:

| Language | A function is… |
|---|---|
| Rust | `function_item` |
| Ruby | `method` |
| Lua | `function_declaration` |
| PHP | `method_declaration` |
| Go | `function_declaration`, `method_declaration` |

Codemark's generator is language-agnostic: it walks an AST and asks *is this
ancestor worth anchoring to?* Something has to answer that question per
grammar. That something is the **profile**.

## Three fields, three consumers

A profile has exactly three fields. Each is read by one place in the codebase,
and — the part worth internalizing — **they do not fail the same way**.

| Field | Consumer | Fallback if wrong |
|---|---|---|
| `landmark_kinds` | `generator::is_landmark_kind` | a built-in union of ~40 kinds |
| `node_labels` | `classifier::classify_node_type` | a global kind→label table |
| `containers` | `breadcrumbs::extract_breadcrumbs` | **none** |

Two degrade quietly. One does not. That asymmetry is the single most useful
thing to know when a profile misbehaves.

```json
{
  "name": "lua",
  "extensions": ["lua"],
  "profile": {
    "landmark_kinds": ["function_declaration", "function_definition"],
    "node_labels": {
      "function_declaration": "function",
      "function_definition": "function",
      "if_statement": "if statement",
      "function_call": "call"
    },
    "containers": ["function"]
  }
}
```

### `landmark_kinds` — what a query may anchor to

When you bookmark a line, the generator walks from that node to the file root
and collects the ancestors. Only **landmarks** are eligible to be named in the
resulting query:

```scheme
(function_item
  name: (identifier) @fn_name
  (#eq? @fn_name "validate")
  body: (block
    (expression_statement) @target))
```

That `function_item` is in the query because Rust's profile lists it as a
landmark. It is what makes the bookmark survive edits elsewhere in the file —
the query finds the function by name, then the statement within it.

**It is additive, not authoritative:**

```rust
fn is_landmark_kind(node_kind: &str, profile: &Profile) -> bool {
    profile.landmark_kinds.iter().any(|k| k == node_kind)
        || DECLARATION_TYPES.contains(&node_kind)
}
```

`DECLARATION_TYPES` is a union drawn from the built-in languages, so
`class_declaration` anchors for *your* grammar whether you list it or not.
This is why a wrong `landmark_kinds` often looks like it works. You can add
kinds; you cannot remove them.

### `node_labels` — a cross-language vocabulary

This is a translation layer, not a naming convenience. Each grammar spells its
constructs differently; `node_labels` maps those spellings onto one shared set
of concepts, so the UI can say "a function" without knowing the language.

| Language | Node kind | Label |
|---|---|---|
| Ruby | `method` | `method` |
| Bash | `function_definition` | `function` |
| Lua | `function_declaration` | `function` |
| Scala | `function_definition` | `function` |

Four grammars, three spellings, two concepts. The label drives query summaries
and icon selection — and, since the vocabulary is shared, a rule written once
applies to every language.

The value side is **not free-form**. Icon lookup keys off the label string, so
an invented one parses fine and silently loses its icon:

`function` · `method` · `class` · `interface` · `enum` · `impl` · `module` ·
`namespace` · `type` · `variable` · `macro` · `constructor` · `init` ·
`deinit` · `subscript` · `if statement` · `for loop` · `while loop` · `match` ·
`return` · `assignment` · `call` · `expression`

`codemark languages validate` warns about anything outside this set.

### `containers` — what appears in a breadcrumb

A list of **labels** (from `node_labels`) naming the landmarks the UI treats
specially:

```json
"containers": ["class", "method", "module"]
```

One entry covers every kind carrying that label, which is the point of keying
on the shared vocabulary rather than on raw node kinds.

A bookmark nested inside such a kind gets a trail:

```
Context: 2:module Billing › 3:  class Invoice › 11:    def compute
```

Three properties surprise people:

**1. No fallback.** The breadcrumb walker builds its whitelist purely from this
list. Name a label no kind in the profile carries, or leave it empty, and
breadcrumbs are blank with no error. `validate` reports an unmatched label as
an **error** for exactly this reason.

**2. A container need not be named.** `containers` drives two presentations of
one idea, and they need different things:

| Presentation | Needs | Unnamed container |
|---|---|---|
| Sticky header | the *line*, pinned while scrolling | works |
| Breadcrumb trail | the *name*, for a path to the target | skipped |

A Zig struct is an anonymous value bound by its enclosing
`variable_declaration` — it has no name of its own, but you still want it
pinned while reading its methods.

**3. A container is a landmark with a body.** Having a body is what makes a
kind worth pinning: it is something you can scroll inside. A one-liner such as
a C# `field_declaration` has nothing to scroll past.

## How a profile reaches the generator

```
manifest.json  →  Language::Dynamic  →  Profile  →  generator / classifier / breadcrumbs
```

A profile reaches codemark one of two ways:

- **Built-in languages** carry a compiled-in profile. Eight ship in the binary.
- **Dynamic grammars** load theirs from `manifest.json` beside the `.wasm` in
  the grammar cache, written by `codemark languages install`.

Both produce the same `Profile` struct, so a registry grammar is not a
second-class citizen — it can even replace a built-in with
`--override-builtin`.

## Seeing it in practice

Install a language and inspect what the profile does:

```bash
# Install from the registry
codemark languages install ruby

# Check every node kind and label against the actual grammar
codemark languages validate

# Bookmark something nested, then look at the query and the trail
codemark add --file app/models/invoice.rb --range 42
codemark show <id> --format markdown
```

`validate` is the fastest way to find a dead entry — it checks each kind and
label against the loaded grammar and reports anything that can never match.

## Why a wrong profile is hard to notice

Three compounding reasons:

1. **Node kinds are grammar-specific.** Lua has `function_call`, *not*
   `call_expression`. Copying kinds between languages produces a profile that
   loads cleanly and does nothing.
2. **Unknown profile keys are ignored**, deliberately, so a manifest from a
   newer codemark still loads on an older binary. A typo'd key silently does
   nothing.
3. **Two of three fields degrade rather than fail**, so partial correctness
   looks like success.

This is not hypothetical. Every built-in profile shipped node kinds its own
grammar did not define — 33 of them across eight languages — and nobody
noticed, because the fallbacks masked it. Dart was the exception that proved
the rule: five of its six landmark kinds did not exist, so its `containers`
resolved to nothing and the language produced **no breadcrumbs at all**.

The registry's answer is to verify rather than review: every profile is checked
against committed breadcrumb snapshots in CI, so a silently-dead entry fails
the build.

## Authoring a profile

Don't hand-write one. The registry has a generator that proposes a profile by
parsing real sample files:

```bash
codemark languages profile init ./tree-sitter-ruby.wasm \
  --sample sample.rb --name ruby
```

It reads the AST rather than the grammar's node-type list, because a node-type
list carries no usage signal — `tree-sitter-lua` defines 47 named kinds and
three belong in `landmark_kinds`. Deriving `containers` requires *observing*
one declaration nested inside another, which only a parse can show.

See [CONTRIBUTING.md](https://github.com/DanielCardonaRojas/codemark-grammars/blob/main/CONTRIBUTING.md)
in the registry for the full workflow, and
[MANIFEST.md](https://github.com/DanielCardonaRojas/codemark-grammars/blob/main/MANIFEST.md)
for a field-by-field reference including the language-specific gotchas found so
far.

## Related

- [Query Generation](./query-generation) — how the query itself is built
- [Tree-sitter](./tree-sitter) — the parsing layer underneath
- [Dynamic Grammars](./dynamic-grammars) — installing and managing WASM grammars
- [Health Status](./health-status) — what happens when a query stops resolving
