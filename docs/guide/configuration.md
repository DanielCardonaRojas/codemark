# Configuration

Codemark reads configuration from a layered `config.toml`: a **global** file with
defaults, optionally overridden by a **local** `.codemark/config.toml` in your
repository. Local values win only when explicitly set.

## Config file locations

| Scope | Path |
|-------|------|
| Global | `global_config_dir()/config.toml` |
| Local (override) | `<repo_root>/.codemark/config.toml` |

`global_config_dir()` resolves in priority order:

| Platform | Path |
|----------|------|
| All (if `$XDG_CONFIG_HOME` set) | `$XDG_CONFIG_HOME/codemark/config.toml` |
| macOS | `~/Library/Application Support/codemark/config.toml` |
| Linux | `~/.config/codemark/config.toml` |
| Windows | `%APPDATA%\codemark\config.toml` |

The first run writes the bundled default config to the global path if none
exists (it never overwrites your edits). The local `.codemark/config.toml` is
**merged** into the global — scalar values replace only when set; per-extension
editor commands *insert* (extend); terminal/gui editor lists *append + dedupe*.

## Config anatomy

The config has nine sections, all `#[serde(default)]`, so any subset parses:

### `[open]` — Editor configuration

Controls the `codemark open` command (and `o` in the TUI).

```toml
[open]
# Default editor command. Placeholders: {FILE}, {LINE_START}, {LINE_END}, {ID}
default = "vim +{LINE_START} {FILE}"
# Command for opening a whole repo (`o` on the TUI's Repos tab).
# Placeholders: {DIR}, {NAME}. Global config only. Unset: quit into $SHELL there.
repo = "code {DIR}"

[open.extensions]
# Per-extension overrides (case-insensitive)
rs = "nvim +{LINE_START} {FILE}"
swift = "xed --line {LINE_START} {FILE}"
py = "code --goto {FILE}:{LINE_START}:{LINE_END}"

[open.editor_types]
# Terminal editors: codemark waits for them to exit
terminal = ["vim", "vi", "nvim", "neovim", "emacs", "nano", "micro", "hx"]
# GUI editors: codemark spawns them and returns immediately
gui = ["xed", "code", "idea", "subl", "typora"]
```

**Resolution order:** extension-specific → `default` → `$EDITOR` env → `"vim"`.

| Placeholder | Expands to |
|-------------|-----------|
| `{FILE}` | Absolute path to the file |
| `{LINE_START}` | Starting line (1-indexed) |
| `{LINE_END}` | Ending line (1-indexed) |
| `{ID}` | Bookmark ID |
| `{DIR}` | Repo root (`repo` template only) |
| `{NAME}` | Repo folder name (`repo` template only) |

#### Opening a repo

`o` on the TUI's Repos tab runs the `repo` command for the selected repo, with
the repo root as its working directory. `repo` is read from the global config
only. A repo's own `.codemark/config.toml` can't set it, so a cloned repo can't
choose the program that runs when you press `o`. When `repo` is unset, the TUI quits into
your shell (`$SHELL`, falling back to `sh`) at the repo root, which is the
closest a program can get to `cd`-ing its parent shell. Type `exit` to return to
the shell you launched codemark from.

Other common choices:

```toml
[open]
# Pick one.
# Open the repo in an editor
repo = "code {DIR}"
# Attach to (or create) a tmux session named after the repo
# repo = "tmux new-session -A -s {NAME} -c {DIR}"
# Already inside tmux: create the session if needed and switch to it
# repo = "sh -c 'tmux has-session -t \"$1\" 2>/dev/null || tmux new-session -d -s \"$1\" -c \"$2\"; tmux switch-client -t \"$1\"' sh {NAME} {DIR}"
```

Like the other `[open]` commands, known GUI editors run in the background while
the TUI stays open; anything else takes over the terminal and the TUI exits.

Commands are tokenized with `shlex`. Codemark blocks (waits) for known terminal
editors — and for unknown programs, to be safe — and returns immediately for
known GUI editors.

### `[storage]`

```toml
[storage]
max_resolutions_per_bookmark = 20   # resolution history prune cap
```

### `[health]`

```toml
[health]
stale_after_days = 30        # (advisory; health is method+hash-driven)
auto_archive_after_days = 7  # used by `heal --auto-archive`
read_max_age_hours = 24      # cached collection-health freshness
```

### `[semantic]` — Semantic search

```toml
[semantic]
enabled = true
# model = "all-minilm-l6-v2"      # or "bge-small-en-v1.5" (both 384-dim)
# models_dir = "~/.cache/codemark/models"
# batch_size = 32
# distance_metric = "l2"          # "l2" | "cosine" | "ip"
# threshold = 1.3                 # unset → metric default (l2 1.3 / cosine 0.85 / ip 0.15)
```

See [Semantic Search & Embeddings](./embeddings) for how these are used.

### `[databases]` — Cross-repo (local only)

Only recognized in `.codemark/config.toml`. Additional databases are read-only.

```toml
[databases]
additional = [
  "../shared-library/.codemark/codemark.db",
  "~/projects/dependency-repo/.codemark/codemark.db",
]
```

### `[identity]`

Overrides Git config detection for database ownership and attribution.

```toml
[identity]
# email = "user@example.com"   # overrides git user.email
# name = "Full Name"            # overrides git user.name
# force = "custom-identity"     # overrides both email and name
```

### `[tui]`

```toml
[tui]
theme = "Catppuccin Mocha"
```

Bundled themes: OneHalfDark (default), Dracula, Nord, Gruvbox, Solarized,
Monokai Extended, GitHub, Visual Studio Dark+, zenburn, ansi, base16, plus the
cohesive base16 schemes **Catppuccin Mocha** and **Everforest Dark**. Drop your
own `.tmTheme` or base16 `.yaml` files into the `themes/` config subdirectory and
reference them by filename. See [Keybindings & the Dashboard](./keybindings).

### `[codetours]` / `[[codetours.servers]]`

Sync server configuration (merged by name).

### `[publish]`

```toml
[publish]
autopopulate_collection_tags = 3
```

## Environment variables

| Variable | Effect |
|----------|--------|
| `CODEMARK_DB` | Database path(s). OS-separator-delimited for multiple. **Forces override mode** — skips auto-detection and `additional`. |
| `CODEMARK_FORMAT` | Default output format (`json`, `table`, `line`, `markdown`). |
| `CODEMARK_COLLECTION_FILTER` | Default collection filter for list/search/resolve/heal/reindex. |
| `CODEMARK_LOG` | Log level (`error`..`trace`). |
| `NO_COLOR` | Disable colored output. |
| `EDITOR` | Editor command fallback (when `[open]` is unset). |
| `CODEMARK_TUI_THEME` | Overrides `config.tui.theme` for the dashboard. |
| `XDG_CONFIG_HOME` | Global config directory. |
| `CODEMARK_DATA_DIR` / `XDG_DATA_HOME` | Global data dir (registry DB). |
| `CODEMARK_GRAMMARS_DIR` | Grammar cache dir (dynamic WASM grammars). |
| `CODMARK_MODELS_DIR` | Embedding model cache dir. *(Note: `CODMARK_`, no E.)* |
| `CODMARK_IDENTITY_EMAIL` / `CODMARK_IDENTITY_NAME` / `CODMARK_IDENTITY` | Identity overrides. |
| `OPENAI_API_KEY` | (Reserved — local inference is the default; no key required.) |

::: warning The models env var is spelled CODMARK_
Several older env vars use `CODMARK_` (no `E`) — `CODMARK_MODELS_DIR` and the
`CODMARK_IDENTITY_*` family. This is preserved verbatim from source; spell it
exactly as shown.
:::
