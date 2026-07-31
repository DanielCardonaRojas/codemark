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

Codemark loads grammars with Tree-sitter **0.25**'s `WasmStore`. A `.wasm` built
against a different Tree-sitter version can fail to load or crash while parsing.

So the single most important check is: **the grammar's `.wasm` must be built with
Tree-sitter 0.25.** Everything below is about getting a 0.25-compatible `.wasm`.

You can confirm a repo's version from its `tree-sitter.json`:

```json
// tree-sitter.json  →  metadata.version
{ "metadata": { "version": "0.25.1", ... } }
```

`version` here is the **grammar release version**, which the Tree-sitter org keeps
in lockstep with the Tree-sitter minor it targets — so a grammar at `0.25.x` is
built for the 0.25 ABI. That's exactly what codemark wants. (Grammars still on
`0.23.x`/`0.24.x` should be rebuilt from source — see the fallback below.)

---

## Recommended: `codemark languages install` (automatic)

Many official grammars ship a `.wasm` artifact right in their GitHub Releases.
`codemark languages install` does the whole thing for you: it takes the latest
release, checks it was built with the 0.25 Tree-sitter CLI, derives the name and
extensions, downloads that release's `.wasm`, and installs it through the
hardened path.

```bash
codemark languages install tree-sitter/tree-sitter-bash
# → downloads tree-sitter-bash.wasm, name=bash, extensions from file-types,
#   installs and validates. Then:
codemark languages validate
codemark languages list        # bash now shows as type "dynamic"
```

Accepted source forms: `owner/repo`, `github:owner/repo`, or a
`https://github.com/owner/repo` URL.

- **ABI check via `package.json`:** codemark loads 0.25 grammars, so install
  reads the release tag's `package.json` `tree-sitter-cli` — the CLI that
  *compiled* the grammar, which is the real ABI signal. (Don't be fooled by
  `tree-sitter.json`'s `metadata.version`: that's the grammar's own package
  version — e.g. `tree-sitter-elm` is `5.9.4` while built with the 0.26 CLI — and
  has nothing to do with the ABI.)
  - CLI **< 0.25** → error (too old; rebuild from source).
  - CLI **> 0.25** → error, telling you to pin an older 0.25 release with
    `--release` (see below).
  - CLI **= 0.25.x** → installs.
  - No readable `package.json` → codemark doesn't block; the post-download
    staged-load validation is the backstop.
- **`--release <tag>`:** when the latest release has moved to a newer Tree-sitter
  (e.g. [tree-sitter-scala](https://github.com/tree-sitter/tree-sitter-scala/releases)
  is now `v0.26.0`), pin an older 0.25 build:
  `codemark languages install tree-sitter/tree-sitter-scala --release v0.25.1`.
  The chosen tag is checked the same way.
- **Name / extensions** come from the selected release's `tree-sitter.json`
  (grammar `name` and `file-types`). Override with `--name` / `--extensions`.
- The generated manifest has an **empty `profile`** — parsing works immediately;
  see [The manifest and the language profile](#the-manifest-and-the-language-profile)
  to improve breadcrumbs.

Examples verified to ship a 0.25 `.wasm`:
[tree-sitter-bash](https://github.com/tree-sitter/tree-sitter-bash/releases),
[tree-sitter-julia](https://github.com/tree-sitter/tree-sitter-julia/releases).

### Manual download (if you'd rather not use `install`)

```bash
# Confirm the release targets 0.25 and lists a .wasm asset, capturing the tag so
# the download can't drift from what you inspected (the latest tag changes over
# time — don't hard-code it).
TAG=$(gh release view --repo tree-sitter/tree-sitter-bash latest --json tagName,assets \
  --jq '{tag: .tagName, assets: [.assets[].name]} | .tag')
echo "latest tag: $TAG"   # e.g. v0.25.1

curl -sL -o /tmp/tree-sitter-bash.wasm \
  "https://github.com/tree-sitter/tree-sitter-bash/releases/download/$TAG/tree-sitter-bash.wasm"

codemark languages add --name bash --extensions sh,bash /tmp/tree-sitter-bash.wasm
```

---

## Fallback: build the `.wasm` yourself

Use this when a grammar has no prebuilt `.wasm`, or is still on an older
Tree-sitter version and needs rebuilding against 0.25.

```bash
# CLI must be 0.25 to match codemark's runtime
npm install -g tree-sitter-cli@0.25    # or: cargo install tree-sitter-cli --version ^0.25
tree-sitter --version                  # confirm 0.25.x

git clone --depth 1 https://github.com/tree-sitter/tree-sitter-ruby
cd tree-sitter-ruby
tree-sitter build --wasm               # emits tree-sitter-ruby.wasm

codemark languages add --name ruby --extensions rb,rake ./tree-sitter-ruby.wasm
```

`tree-sitter build --wasm` needs **Docker** or a local **Emscripten** (`emcc`)
toolchain. See [tree-sitter-local-setup.md](./tree-sitter-local-setup.md) for CLI
setup details.

---

## The manifest and the language profile

`codemark languages add` writes a `manifest.json` next to the `.wasm` in the
grammar cache. It starts with an **empty profile**, which is enough for parsing
and structural resolution to work — you only need to fill the profile to get good
**breadcrumbs** and **query summaries**.

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
    "landmark_kinds": ["function_declaration", "local_function_declaration"],

    // node kind → human-readable label shown in query summaries / TUI icons.
    "node_labels": {
      "function_declaration": "function",
      "if_statement": "if statement",
      "call_expression": "call"
    },

    // [container node kind, name-field] pairs — the ancestors that make useful
    // breadcrumbs, and which child field holds their name.
    "containers": [["function_definition_statement", "name"]],

    // Optional: node kind → field to summarize (e.g. an if statement's condition).
    "semantic_fields": { "if_statement": "condition" }
  }
}
```

See the committed reference: [`tests/fixtures/grammars/lua/manifest.json`](../../tests/fixtures/grammars/lua/manifest.json).

### Discovering the right node kinds

Parse a sample file with the Tree-sitter CLI and read the AST to learn the node
names for `landmark_kinds` / `node_labels` / `containers`:

```bash
tree-sitter parse example.rb        # prints the full AST with node kinds
```

Then edit the manifest in the cache dir and re-run `codemark languages validate`.

---

## Versioning convention

Two independent version numbers are in play — don't conflate them:

- **Grammar / Tree-sitter version** — the `metadata.version` in the grammar's
  `tree-sitter.json` (e.g. `0.25.1`). This determines **ABI compatibility** and
  must be a **0.25.x** for codemark. It's the grammar author's number; you don't
  choose it, you check it.

- **Manifest `version`** — the `version` field in *your* `manifest.json`. This
  versions *your profile* (the labels/landmarks/containers you authored), not the
  grammar. Bump it when you change the profile. Recommended convention:
  **semver, starting at `0.1.0`**, incrementing as you refine the profile —
  independent of the grammar's version. (The Lua fixture uses `0.2.0` for its
  hand-tuned profile even though it wraps a specific grammar build.)

If you want the grammar's Tree-sitter version recorded for provenance, add it as
a separate field (e.g. `"grammar_version": "0.25.1"`) rather than overloading
`version`; codemark ignores unknown manifest fields.

---

## Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| Grammar installs but files aren't recognized | Binary built without `--features wasm`. Rebuild with the feature. |
| `validate` reports "failed to load" | `.wasm` built for a non-0.25 Tree-sitter. Rebuild from source with CLI 0.25. |
| `add` rejects the name | Name is (or aliases) a built-in — pick a distinct name. |
| An extension is dropped on install | It's owned by a built-in (e.g. `rs`, `py`); built-ins win, so a dynamic grammar can't claim it. |
| Grammar not picked up in a running TUI | Discovery refreshes on terminal focus; refocus the TUI, or restart. |
