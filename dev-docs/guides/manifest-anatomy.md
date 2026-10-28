# Anatomy of a `manifest.json`

Every dynamically loaded grammar has a `manifest.json` next to its
`grammar.wasm`. This page explains each field, **who actually reads it**, and
what happens when it's wrong — which is rarely an error message.

For how to obtain and install a grammar, see
[adding-wasm-grammars.md](./adding-wasm-grammars.md).

---

## A complete example

```json
{
  "name": "lua",
  "version": "0.3.0",
  "extensions": ["lua"],
  "profile": {
    "landmark_kinds": [
      "function_declaration",
      "function_definition",
      "variable_declaration"
    ],
    "node_labels": {
      "function_declaration": "function",
      "if_statement": "if statement",
      "function_call": "call"
    },
    "containers": [
      ["function_declaration", "name"]
    ]
  }
}
```

It lives at `<grammar cache>/<dir>/manifest.json`:

| Platform | Cache directory |
|---|---|
| macOS | `~/Library/Caches/codemark/grammars/` |
| Linux | `~/.cache/codemark/grammars/` |
| Windows | `%LOCALAPPDATA%\codemark\grammars\` |

Override with `$CODEMARK_GRAMMARS_DIR`. `codemark languages list` prints the
exact path for every installed grammar.

---

## Top-level fields

The reader is `Manifest` in `crates/codemark-core/src/parser/registry.rs`. It
deserializes exactly three fields:

```rust
struct Manifest {
    name: String,
    extensions: Vec<String>,
    profile: Option<Profile>,
}
```

### `name` — required

The language identifier. Three things depend on it:

1. **It is stored on every bookmark.** `bookmarks.language` holds this string,
   and resolution looks the language up by it later. Renaming a grammar after
   bookmarks exist orphans them.
2. **It must not collide with a built-in**, including aliases. `rust`, `rs`,
   `ts`, `py`, `cs` and friends are all rejected — a dynamic grammar with such a
   name could never win lookup, so registering it would be a dead entry.
3. **It must match the `.wasm`'s exported symbol.** A Tree-sitter module exports
   `tree_sitter_<name>`, so installing `tree-sitter-lua.wasm` under the name
   `mylua` fails at load with *"module did not contain language function:
   tree_sitter_mylua"*.

> **The directory name doesn't matter.** Registration keys off this field, not
> the folder. A grammar in `grammars/x/` whose manifest says `"name": "mylua"`
> registers as `mylua`. Keeping them equal is a convention worth following, but
> nothing enforces it.

Names are matched **case-insensitively**, so `Lua` and `lua` are the same
language. On a collision the first grammar by sorted path wins and the duplicate
is skipped with a warning.

### `extensions` — required

File extensions this grammar claims, **without the leading dot**:

```json
"extensions": ["rb", "rake", "gemspec"]
```

`codemark languages add` normalizes what you pass — trims whitespace, strips a
leading `.`, lowercases, drops empties — so `--extensions ".RB, rake"` becomes
`["rb", "rake"]`.

**A hand-edited manifest gets no such treatment.** Discovery lowercases for the
lookup key but does not strip dots, so a stored `".lua"` registers verbatim and
matches no file — `languages list` will happily show `[".lua"]` while nothing
resolves. Write extensions dotless.

Extensions owned by a built-in are dropped at discovery with a warning, since
the built-in wins `from_extension` anyway. The grammar's other extensions still
register — it isn't all-or-nothing.

### `version` — optional, and currently unread

**Nothing reads this field.** It is written by `languages add` (always
`"0.1.0"`) and ignored at discovery.

It versions *your profile*, not the grammar — bump it when you change
`landmark_kinds`, `node_labels` or `containers`. In the registry the equivalent
field is `profile_version` in `index.json`, which *is* displayed by
`languages registry list`.

Don't record the grammar's own version here. Three different numbers are in
play; see [Versioning convention](./adding-wasm-grammars.md#versioning-convention).

### `profile` — optional

Omitting it (or writing `{}`) is valid: parsing and structural resolution work
with an empty profile. What you lose is breadcrumbs and good query summaries.

Unknown keys inside `profile` are **ignored, not rejected**. That is deliberate —
a manifest written by a newer codemark must still load on an older binary — but
it means a typo'd key silently does nothing. `codemark languages validate`
reports them as warnings. A documented-but-unimplemented `semantic_fields` key
sat in a committed fixture for months because of exactly this.

---

## Inside `profile`

Three fields, defined in `crates/codemark-core/src/parser/profile.rs`. Each has
**one** consumer, and they fail in different ways:

| Field | Consumer | If wrong |
|---|---|---|
| `landmark_kinds` | `generator::is_landmark_kind` | Degrades quietly — falls back to a built-in union |
| `node_labels` | `classifier::classify_node_type` | Degrades quietly — falls back to a global table |
| `containers` | `engine::breadcrumbs::extract_breadcrumbs` | **Breadcrumbs vanish** — no fallback at all |

That asymmetry is the single most important thing on this page. Two of the three
fields have a safety net; `containers` does not.

### `landmark_kinds`

Node kinds that anchor a structural query. When the generator walks from a
bookmarked node up to the file root, landmark ancestors become part of the
stored query, which is what lets a bookmark survive edits elsewhere in the file.

```json
"landmark_kinds": ["function_declaration", "function_definition", "variable_declaration"]
```

Typically declarations: functions, methods, classes, structs, enums, modules,
type aliases, constants.

**It is additive, not authoritative.** The check is:

```rust
fn is_landmark_kind(node_kind: &str, profile: &Profile) -> bool {
    profile.landmark_kinds.iter().any(|k| k == node_kind)
        || DECLARATION_TYPES.contains(&node_kind)
}
```

`DECLARATION_TYPES` is a built-in union of 40 kinds drawn from the eight static
languages. So a kind like `class_declaration` is a landmark for *your* grammar
whether or not you list it — which is why a wrong `landmark_kinds` often appears
to work. You can add kinds; you cannot remove them.

### `node_labels`

**This is a translation layer into one cross-language vocabulary.** Every
grammar spells its constructs differently; `node_labels` maps those spellings
onto a shared set of concepts, so the rest of codemark — query summaries, TUI
icons, anything user-facing — can talk about "a function" without knowing which
language it came from.

The same concept across the profiles in the registry today:

| Language | Node kind | Label |
|---|---|---|
| Ruby | `method` | `method` |
| Bash | `function_definition` | `function` |
| Lua | `function_declaration` | `function` |
| PHP | `method_declaration` | `method` |
| Zig | `function_declaration` | `function` |
| Rust (built-in) | `function_item` | `function` |

Six grammars, five spellings, one concept. A user searching for functions, or a
TUI picking a function icon, works off the right-hand column and never needs the
left.

```json
"node_labels": {
  "function_declaration": "function",
  "if_statement": "if statement",
  "function_call": "call"
}
```

It isn't limited to landmarks — control flow and expressions get labels too
(`if statement`, `for loop`, `call`, `assignment`), which is what makes a query
summary readable.

Also additive: a missing entry falls through to a global table, and failing
that, to the raw node kind.

**Because it's a shared vocabulary, the value side is not free-form.** Icon
selection keys off the label string, so an invented one parses fine and silently
loses its icon — and, worse, splits a concept that should be unified. Writing
`"subroutine"` where every other language says `"function"` means those nodes
stop being comparable across languages. The labels the built-in profiles use:

`function` · `method` · `class` · `interface` · `enum` · `impl` · `module` ·
`namespace` · `type` · `variable` · `macro` · `constructor` · `init` ·
`deinit` · `subscript` · `if statement` · `for loop` · `while loop` · `match` ·
`return` · `assignment` · `call` · `expression`

`languages validate` warns about anything outside this set.

> **`node_labels` does not drive `@sticky` captures.** The `@sticky.function`
> annotations in generated queries come from `generator::get_sticky_tag`, a
> hardcoded `match` on node kind that takes no profile. Adding a label does not
> create a sticky capture.

### `containers`

Pairs of `[node_kind, name_field]` naming the ancestors that make useful
breadcrumbs.

```json
"containers": [
  ["class_declaration", "name"],
  ["impl_item", "type"]
]
```

A bookmark nested inside a listed kind gets a breadcrumb trail:

```
Context: 2:module Billing › 3:  class Invoice › 11:    def compute
```

Two things to know, both counterintuitive:

**1. There is no fallback.** `extract_breadcrumbs` builds its whitelist purely
from this list. Name a kind the grammar doesn't define — or leave the list empty —
and breadcrumbs are blank, with no error anywhere. Unlike the other two fields,
nothing covers for you.

**2. The second element is currently unused.** The breadcrumb walker reads only
`containers[].0` and then extracts the *whole source line*:

```rust
let whitelist: Vec<&str> =
    language.profile().containers.iter().map(|(kind, _)| kind.as_str()).collect();
```

So `["impl_item", "name"]` and `["impl_item", "type"]` behave identically today.
Still get it right: it's part of the schema, `validate` checks it exists in the
grammar, and breadcrumbs may yet use it to extract just the name rather than the
full line.

The field is on the **container**, not the thing it contains. Rust's `impl_item`
holds its name under `type`, not `name` — assuming `name` is how a committed
fixture once ended up pointing at a kind that didn't exist.

---

## Why wrong manifests are hard to notice

Three independent reasons a mistake stays silent:

1. **Node kinds are grammar-specific.** Ruby says `method`, Rust `function_item`,
   Lua `function_declaration`. Lua has `function_call`, *not* `call_expression`.
   Copying kinds between languages produces a manifest that loads cleanly and
   does nothing.
2. **Unknown profile keys are ignored** by design, for forward compatibility.
3. **Two of three fields degrade rather than fail**, so partial correctness looks
   like success.

### Always validate

```bash
codemark languages validate
```

It loads each grammar and checks every node kind and container field against it:

```
Found issues with installed grammars:
  - .../lua: error: profile.landmark_kinds[1]: node kind 'function_item' is not
    defined by this grammar, so it can never match

Warnings (these grammars still work):
  - .../lua: warning: profile.containers: no containers, so this language
    produces no breadcrumbs
```

**Errors** block an install, so a bad profile can't replace a working grammar.
**Warnings** are advisory. Close misses get a suggestion
(`'local_function_declaration' … did you mean 'function_declaration'?`).

The two error kinds differ in what they cost you, which matters when debugging
missing breadcrumbs:

- **An unknown node kind** (`landmark_kinds[i]`, `containers[i].0`) is dead
  weight — nothing in the grammar can ever match it. A wrong `containers[i].0`
  is the usual cause of blank breadcrumbs.
- **An unknown container field** (`containers[i].1`) blocks the install but does
  *not* affect matching, because breadcrumb extraction reads only the kind. If
  your breadcrumbs are missing, this error is not the reason.

**What `validate` can't check:** Tree-sitter exposes field names globally rather
than per node kind, so `["mod_item", "value"]` — a real field, but not one
`mod_item` has — passes. Harmless today given the field is unread, but it means
a clean `validate` isn't proof the pair is right.

### Confirm breadcrumbs separately

Because `containers` fails silently, validation alone is not enough. Bookmark a
line nested inside a container and look:

```bash
codemark add --file sample.rb --range 14
codemark show <id> --format table | grep Context
```

Empty `Context` on a line that *is* nested means a `containers` entry isn't
matching.

---

## Don't hand-write this

Two better routes, in order:

**Fetch a curated profile** — someone may have done the work already:

```bash
codemark languages registry list
codemark languages add ./tree-sitter-ruby.wasm --from-registry ruby
```

**Generate one from real code** — derives the profile from a parse, including
which field actually holds each container's name:

```bash
# needs a build with --features profile-authoring
codemark languages profile init ./tree-sitter-ruby.wasm --sample example.rb \
  --name ruby --extensions rb,rake
```

It prints an evidence table — every kind seen, how often, whether it became a
landmark, which field held its name, and why — so you can audit the proposal
rather than trust it.

---

## Related

- [adding-wasm-grammars.md](./adding-wasm-grammars.md) — obtaining and installing a grammar
- [`docs/guide/dynamic-grammars.md`](../../docs/guide/dynamic-grammars.md) — the loading architecture
- [`tests/fixtures/grammars/lua/manifest.json`](../../tests/fixtures/grammars/lua/manifest.json) — a verified reference manifest
- [codemark-grammars](https://github.com/DanielCardonaRojas/codemark-grammars) — the profile registry
