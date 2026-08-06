# Configuration

Codemark is highly customizable to fit your workflow. Configuration is typically managed via a `config.toml` file in your Codemark config directory.

## Editor Integration

By default, pressing `o` in the TUI will open the bookmark in your terminal editor. You can configure per-file-extension commands to open specific files in different editors (e.g., opening `.rs` files in Neovim and `.ts` files in VS Code).

*(Detailed editor configuration examples go here).*

## Themes

The TUI ships with built-in color schemes. You can set your preferred theme in your config file under the `[tui]` section or via an environment variable.

```toml
[tui]
theme = "Catppuccin Mocha"
```
Or via environment variable:
```bash
export CODEMARK_TUI_THEME="Everforest Dark"
```

To see all available themes, run:
```bash
codemark-tui --list-schemes
```

## Markdown Templates

Codemark formats command output and TUI previews using [Handlebars](https://handlebarsjs.com/) templates. You can override the defaults by placing your own templates in the configuration directory:

```bash
mkdir -p ~/.config/codemark/templates
cp ./templates/codemark_show.md ~/.config/codemark/templates/
```
You can customize exactly what metadata is displayed when you view a bookmark's details or a collection overview.
