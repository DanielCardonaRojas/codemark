# Configuration

Codemark reads configuration from two locations, allowing for global defaults and per-repository overrides.

## Config File Locations

| Platform | Global Config Path |
|----------|-------------------|
| All (if `$XDG_CONFIG_HOME` is set) | `$XDG_CONFIG_HOME/codemark/config.toml` |
| macOS | `~/Library/Application Support/codemark/config.toml` |
| Linux | `~/.config/codemark/config.toml` |
| Windows | `%APPDATA%\codemark\config.toml` |

### Local Override

You can also place a `.codemark/config.toml` file in your repository root. Values in the local config will override global defaults.

## Editor Configuration (`[open]`)

The `[open]` section configures the `codemark open` command, which allows you to jump directly to bookmarked code.

```toml
[open]
# Default editor command (supports placeholders)
default = "nvim +{LINE_START} {FILE}"

# Extension-specific overrides
[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
swift = "xed --line {LINE_START} {FILE}"
py = "code --goto {FILE}:{LINE_START}:{LINE_END}"
ts = "code --goto {FILE}:{LINE_START}:{LINE_END}"
md = "typora {FILE}"

# Terminal editors (codemark waits for them to exit)
[open.editor_types]
terminal = ["vim", "vi", "nvim", "neovim", "emacs", "nano", "micro", "helix", "hx"]
# GUI editors (codemark spawns and returns immediately)
gui = ["xed", "code", "idea", "subl", "typora"]
```

### Placeholders

| Placeholder | Description |
|-------------|-------------|
| `{FILE}` | Absolute path to the file |
| `{LINE_START}` | Starting line number (1-indexed) |
| `{LINE_END}` | Ending line number (1-indexed) |
| `{ID}` | Bookmark ID |

## Semantic Search (`[semantic]`)

Configure how vector embeddings are generated for semantic search.

```toml
[semantic]
enabled = true
model = "local"  # "local" or "openai"
local_model = "all-MiniLM-L6-v2"
# openai_model = "text-embedding-3-small"
# openai_api_key = "sk-..."
batch_size = 32
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CODEMARK_DB` | Override database path |
| `CODEMARK_LOG` | Log level (error, warn, info, debug, trace) |
| `NO_COLOR` | Disable colored output |
| `OPENAI_API_KEY` | Key for OpenAI embedding provider |
