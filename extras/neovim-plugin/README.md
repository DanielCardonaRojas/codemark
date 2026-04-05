# codemark.nvim

Neovim integration for [codemark](../../README.md) — structural code bookmarking with tree-sitter queries.

## Requirements

- Neovim 0.10+
- `codemark` CLI installed and in PATH
- [telescope.nvim](https://github.com/nvim-telescope/telescope.nvim) (optional, for `:CodemarkBrowse`)

## Install

### lazy.nvim

```lua
{
  dir = "~/path/to/codemark/extras/neovim-plugin",
  config = function()
    require("codemark").setup()
  end,
}
```

### Manual

```bash
# Symlink into your Neovim packages
ln -s /path/to/codemark/extras/neovim-plugin ~/.local/share/nvim/site/pack/codemark/start/codemark.nvim
```

Then add to your `init.lua`:

```lua
require("codemark").setup()
```

## Usage

### Visual select → Bookmark

1. Visual select the code you want to bookmark (`V` for line mode)
2. `<leader>bd` — dry run (preview what would be bookmarked)
3. `<leader>ba` — add bookmark (prompts for note and tags)

### Browse with Telescope

`<leader>bb` — opens a Telescope picker with all bookmarks, fuzzy search, and live code preview.

Press `<CR>` to jump to the bookmarked code.

### Commands

| Command | Description |
|---------|-------------|
| `:CodemarkAdd` | Add bookmark from visual selection (prompts for note/tags) |
| `:CodemarkDryRun` | Preview what the visual selection would bookmark |
| `:CodemarkBrowse` | Browse all bookmarks with Telescope |
| `:CodemarkList` | List bookmarks for the current file |
| `:CodemarkStatus` | Show bookmark health summary |
| `:CodemarkPreview` | Preview nearest bookmark in a split |

### Default keymaps

| Key | Mode | Action |
|-----|------|--------|
| `<leader>ba` | Visual | Add bookmark |
| `<leader>bd` | Visual | Dry run |
| `<leader>bb` | Normal | Browse (Telescope) |
| `<leader>bl` | Normal | List current file bookmarks |
| `<leader>bs` | Normal | Status |

### Sign column

Bookmarked lines are marked with `▎` in the sign column. Signs refresh automatically on `BufEnter`.

## Configuration

```lua
require("codemark").setup({
  binary = "codemark",    -- path to codemark binary
  signs = true,           -- show signs in the gutter
  sign_text = "▎",        -- sign character
  sign_hl = "CodemarkSign", -- highlight group for signs
})
```

## How it works

The plugin shells out to the `codemark` CLI for all operations. No tree-sitter parsing happens in Neovim — codemark handles that. The plugin is purely UI glue:

- **Add**: gets visual range → `codemark add --file % --range start:end --dry-run --json` → prompt → `codemark add`
- **Browse**: `codemark list --format line` feeds Telescope, `codemark preview {id}` for the preview pane
- **Signs**: `codemark resolve --file % --json` on BufEnter, places signs at resolved line numbers
