# Agent Workflow Walkthrough

This walkthrough uses a small Rust repository to show how an agent saves context,
resumes from a collection, and resolves a bookmark after lines move. Commands are
for a POSIX shell. JSON is the default output; use `--format json` to request it
explicitly (there is no `--json` flag).

## Session 1: Save context

Create a repository and a file to bookmark:

```bash
mkdir codemark-example && cd codemark-example
git init
mkdir src
cat > src/main.rs <<'EOF'
fn authenticate() -> bool {
    true
}

fn main() {
    println!("{}", authenticate());
}
EOF
git add src/main.rs
git -c user.name=Example -c user.email=example@example.com commit -m "Initial example"
codemark init
```

For this walkthrough, disable optional semantic embeddings so no model download
is needed:

```bash
cat > .codemark/config.toml <<'EOF'
[semantic]
enabled = false
EOF
```

Bookmark the authentication function and save the returned `data.id` as `ID`.
Set `--created-by` to the agent's slug when an agent creates the bookmark; omit
it for a human-created bookmark.

```bash
codemark add --file src/main.rs --range 1-3 --tag auth \
  --note "token validation entrypoint" --created-by codex
# Replace <returned-id> with data.id from the preceding command.
ID=<returned-id>

codemark collection create request-lifecycle --description "Authentication flow"
codemark collection add request-lifecycle "$ID"
codemark heal --validate-only --collection request-lifecycle
```

`--range 1-3` is a line range. Byte ranges require the `b` prefix, such as
`b1024-1280`. Validation uses `heal --validate-only`, not a `validate` command;
it skips resolution-history writes but still updates bookmark status.

## Session 2: Resume from saved context

In the same repository, a later session can recover the collection and its
current code locations without repeating the initial investigation:

```bash
codemark collection resolve request-lifecycle --format json
codemark show "$ID"
codemark search "auth" --format json
codemark list --tag auth --format json
codemark resolve --health active --format json
```

Keep the returned ID between sessions, or recover it from `list` or the
collection. `show` includes the stored metadata and code preview; `resolve`
returns the current location. To add another note without creating a new
bookmark:

```bash
codemark annotate "$ID" --note "Reviewed the authentication entrypoint"
```

## Session 3: Resolve after lines move

Insert two lines above the function without changing its name or body:

```bash
{ printf '// Inserted above bookmark\n\n'; cat src/main.rs; } > src/main.rs.tmp
mv src/main.rs.tmp src/main.rs

codemark resolve "$ID" --format json
codemark collection resolve request-lifecycle --format json
codemark diff --since HEAD --format json
codemark heal --collection request-lifecycle --format json
```

The function now starts on line 3 instead of line 1. Its structural query still
matches, so the bookmark resolves at its new line range. This example keeps the
file path unchanged: it does not promise automatic relocation to a renamed file
or a different file after a function is extracted.

To validate without recording resolution history, use:

```bash
codemark heal --validate-only --collection request-lifecycle --format json
```

When the investigation is finished, remove the example bookmark with:

```bash
codemark remove "$ID"
```
