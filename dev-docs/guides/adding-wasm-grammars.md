# Adding WASM Grammars for New Languages

Codemark supports new programming languages **at runtime, without recompiling** —
by loading a Tree-sitter grammar compiled to WebAssembly. This guide shows how to
get a grammar, install it, and write its profile.

> **Prerequisite:** the grammar-loading code path is behind an opt-in Cargo
> feature. You must run a codemark built with `--features wasm`, or the grammar
> installs to disk but is silently ignored at runtime (`codemark languages add`
> warns when the running binary lacks the feature).

---

## The one rule that matters: ABI compatibility

Codemark loads grammars through Tree-sitter 0.25's `WasmStore`, which accepts
**ABI versions 13 through 15**:

```c
/* tree_sitter/api.h */
#define TREE_SITTER_LANGUAGE_VERSION             15
#define TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION 13
```

Tree-sitter is **backwards-compatible but not forwards-compatible**: a grammar
built with an older CLI loads fine; one built with a *newer* CLI than codemark's
runtime does not. In practice ABI 13–15 covers CLI 0.21 through 0.25 — which is
nearly the entire published ecosystem, since the
[wiki parser list](https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers)
only catalogues parsers at ABI ≥ 13.

So you do **not** need to match 0.25 exactly. Building with a 0.25 CLI is simply
the safest default, because it can never be too new.

> A grammar repo's `tree-sitter.json` has a `metadata.version`, but that is the
> grammar package's **own** semver — *not* the ABI version — so don't rely on it
> to judge compatibility.

**You don't have to guess.** `codemark languages add` loads the `.wasm` through
the same `WasmStore` the runtime uses before committing anything, so an
incompatible module is rejected up front with a clear error. Try the grammar you
have; rebuild only if it's refused.

> Verified: the prebuilt `.wasm` from `tree-sitter-bash` **v0.23.3** — two minor
> versions before 0.25 — installs and parses correctly, producing properly
> anchored queries. Earlier revisions of this guide told you to reject it.

---

## Build the `.wasm`, then `add` it

```bash
# 0.25 is the safe default — it can never produce a too-new ABI for codemark.
npm install -g tree-sitter-cli@0.25    # or: cargo install tree-sitter-cli --version ^0.25
tree-sitter --version                  # confirm 0.25.x

git clone --depth 1 https://github.com/tree-sitter/tree-sitter-ruby
cd tree-sitter-ruby
tree-sitter build --wasm               # emits tree-sitter-ruby.wasm

codemark languages add --name ruby --extensions rb,rake ./tree-sitter-ruby.wasm
# Then:
codemark languages validate
codemark languages list                # ruby now shows as type "dynamic"
```

`tree-sitter build --wasm` needs **Docker** or a local **Emscripten** (`emcc`)
toolchain. See [tree-sitter-local-setup.md](./tree-sitter-local-setup.md) for CLI
setup details.

`codemark languages add` validates the `.wasm` (on a `--features wasm` build it
loads it through the 0.25 `WasmStore`, so an incompatible module is rejected up
front), writes a `manifest.json`, and installs both atomically. The generated
manifest has an **empty `profile`** — parsing works immediately; see
[The manifest and the language profile](#the-manifest-and-the-language-profile)
to improve breadcrumbs.

### Using a prebuilt `.wasm` from a release

Some grammar repos publish a `.wasm` in their GitHub releases, which skips the
Docker/emcc step entirely. **Try it first** — anything built with CLI 0.21–0.25
loads, and `add` tells you immediately if it doesn't. Only a `.wasm` from a CLI
*newer* than codemark's runtime is rejected, since Tree-sitter isn't
forwards-compatible.

Download it and `add` it the same way:

```bash
# -f makes curl fail on a 404/moved asset instead of saving an HTML error page
# to the .wasm path (which `add` would then accept as a regular file).
curl -fsSL -o /tmp/tree-sitter-bash.wasm \
  https://github.com/tree-sitter/tree-sitter-bash/releases/download/v0.25.1/tree-sitter-bash.wasm
codemark languages add --name bash --extensions sh,bash /tmp/tree-sitter-bash.wasm
```

---

## The manifest and the language profile

`codemark languages add` writes a `manifest.json` next to the `.wasm` in the
grammar cache. It starts with an **empty profile**, which is enough for parsing
and structural resolution to work — you only need to fill the profile to get good
**breadcrumbs** and **query summaries**.

> **Check the registry first.** If someone has already curated a profile for your
> language, don't write one:
>
> ```bash
> codemark languages registry list
> codemark languages add ./tree-sitter-ruby.wasm --from-registry ruby
> ```
>
> The registry ships profiles only — you still build the `.wasm`. The rest of
> this section is for a language the registry doesn't cover yet (and the result
> is worth contributing back).

The cache lives at the platform cache dir (override with `$CODEMARK_GRAMMARS_DIR`):

| Platform | Default location |
|----------|------------------|
| macOS    | `~/Library/Caches/codemark/grammars/<name>/` |
| Linux    | `~/.cache/codemark/grammars/<name>/` |
| Windows  | `%LOCALAPPDATA%\codemark\grammars\<name>\` |

### Manifest schema

```jsonc
{
  "name": "lua",                 // language name (must not alias a built-in, e.g. not "rs")
  "version": "0.2.0",            // your manifest's own version — see "Versioning" below
  "extensions": ["lua"],         // dotless, lowercase; must not collide with a built-in
  "profile": {
    // Node kinds that anchor a structural query (replaces the built-in
    // DECLARATION_TYPES set for this language).
    "landmark_kinds": ["function_declaration", "function_definition"],

    // node kind → human-readable label shown in query summaries / TUI icons.
    "node_labels": {
      "function_declaration": "function",
      "if_statement": "if statement",
      "function_call": "call"
    },

    // [container node kind, name-field] pairs — the ancestors that make useful
    // breadcrumbs, and which child field holds their name.
    "containers": [["function_declaration", "name"]]
  }
}
```

> **Every node kind above is grammar-specific.** The kinds in this example are
> real kinds from `tree-sitter-lua` — Lua has `function_call`, not
> `call_expression`, and no `local_function_declaration` at all. Copying kinds
> from another language's profile produces a manifest that loads without error
> and silently does nothing. Run `codemark languages validate` after editing: it
> checks every kind against the grammar and reports the ones that don't exist.

See the committed reference: [`tests/fixtures/grammars/lua/manifest.json`](../../tests/fixtures/grammars/lua/manifest.json).

### Generating a profile (recommended)

Rather than hand-authoring the profile, let codemark propose one by parsing real
sample files with the grammar:

```bash
# Requires a build with the authoring feature:
#   cargo build -p codemark-cli --features profile-authoring
codemark languages profile init ./tree-sitter-ruby.wasm \
  --sample sample.rb --name ruby --extensions rb,rake
```

It prints the proposed manifest plus an **evidence table** showing every node
kind it saw, how often, whether it became a landmark, which field held its name,
and why:

```
┌──────────────────────┬──────┬──────────┬────────────┬──────────────────────────────────────┐
│ Node kind            ┆ Seen ┆ Landmark ┆ Name field ┆ Why                                  │
╞══════════════════════╪══════╪══════════╪════════════╪══════════════════════════════════════╡
│ function_declaration ┆ 2    ┆ yes      ┆ name       ┆ name in field 'name', has a body,    │
│                      ┆      ┆          ┆            ┆ nests other declarations             │
│ if_statement         ┆ 1    ┆ -        ┆ -          ┆ control flow or expression —         │
│                      ┆      ┆          ┆            ┆ labelled but not a landmark          │
└──────────────────────┴──────┴──────────┴────────────┴──────────────────────────────────────┘
```

Review it, then install. `--out <file>` writes the manifest instead of printing
it; nothing is ever written to the grammar cache, so proposing and installing
stay separate steps.

The proposal is derived from **observed structure**, which is why samples matter
more than their length: cover the constructs you care about (nested classes,
methods, anonymous functions) and the container fields are read off the real
parse instead of assumed. More samples give a better proposal.

### Discovering the right node kinds by hand

If you'd rather write the profile yourself, parse a sample file with the
Tree-sitter CLI and read the AST to learn the node names for `landmark_kinds` /
`node_labels` / `containers`:

```bash
tree-sitter parse example.rb        # prints the full AST with node kinds
```

Then edit the manifest in the cache dir and re-run `codemark languages validate`,
which checks each kind and container field against the loaded grammar:

```
$ codemark languages validate
Found issues with installed grammars:
  - ~/Library/Caches/codemark/grammars/ruby: error: profile.landmark_kinds[1]: node
    kind 'function_item' is not defined by this grammar, so it can never match
  - ~/Library/Caches/codemark/grammars/ruby: error: profile.containers[0].1: 'label'
    is not a field in this grammar, so the container entry for 'method' names
    something that doesn't exist

Warnings (these grammars still work):
  - ~/Library/Caches/codemark/grammars/ruby: warning: profile.semantic_fields: unknown
    profile key — it is ignored at load time (known keys: landmark_kinds, node_labels,
    containers)
```

A near miss gets a suggestion — `'local_function_declaration' … (did you mean
'function_declaration'?)`.

An **error** means that part of the profile is dead: it can never match, and an
install is refused rather than committing it. A **warning** still works but is
probably not what you meant — note that `containers` has no fallback, so an
empty one silently produces no breadcrumbs at all. Validate before you trust a
profile you hand-wrote.

What `validate` **can't** check: tree-sitter's `Language` exposes field names
globally, not per node kind, so a container like `["mod_item", "value"]` — where
`value` is a real field but not one `mod_item` has — passes cleanly. That gap is
cosmetic today, because breadcrumbs match on the node kind alone and never read
the field; it matters for the schema being correct, not for current output.
[The generator](#generating-a-profile-recommended) reads the field off a real
parse rather than guessing.

The check that *does* affect output is the node kind: get `containers[].0` wrong
and the ancestor never matches, so breadcrumbs go blank. Confirm by bookmarking
a line nested inside the container and checking its context line.

---

## Versioning convention

Three different version numbers are in play — don't conflate them:

- **Tree-sitter CLI / ABI version** — the version of the `tree-sitter` CLI the
  grammar was **compiled with** (its `package.json` `tree-sitter-cli`
  dev-dependency). This determines **ABI compatibility**: codemark accepts
  ABI 13–15, i.e. roughly CLI 0.21–0.25. Only a *newer* CLI than codemark's
  runtime is a problem, so building with 0.25 is the safe default.

- **Grammar package version** — the `metadata.version` in the grammar's
  `tree-sitter.json`. This is the grammar *package's* own semver (e.g.
  `tree-sitter-elm` is `5.9.4`), unrelated to the Tree-sitter ABI. Ignore it for
  compatibility purposes.

- **Manifest `version`** — the `version` field in *your* `manifest.json`. This
  versions *your profile* (the labels/landmarks/containers you authored), not the
  grammar. Bump it when you change the profile. Recommended convention:
  **semver, starting at `0.1.0`**, incrementing as you refine the profile —
  independent of the grammar's version. (The Lua fixture uses `0.2.0` for its
  hand-tuned profile even though it wraps a specific grammar build.)

If you want the CLI version the grammar was built with recorded for provenance,
add it as a separate field (e.g. `"built_with_cli": "0.25.6"`) rather than
overloading `version`; codemark ignores unknown manifest fields.

---

## Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| Grammar installs but files aren't recognized | Binary built without `--features wasm`. Rebuild with the feature. |
| `validate` reports "failed to load" | `.wasm` built with a CLI *newer* than codemark's runtime (ABI > 15), or it isn't a Tree-sitter module. Rebuild from source with CLI 0.25. |
| `add` rejects the name | Name is (or aliases) a built-in — pick a distinct name. |
| An extension is dropped on install | It's owned by a built-in (e.g. `rs`, `py`); built-ins win, so a dynamic grammar can't claim it. |
| Grammar not picked up in a running TUI | Discovery refreshes on terminal focus; refocus the TUI, or restart. |
