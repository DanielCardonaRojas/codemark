# Keybindings & UI

Codemark includes a native, terminal-based dashboard (TUI) designed for speed and efficiency, heavily inspired by `lazygit`.

## Launching the TUI

Run the following command in your terminal:
```bash
codemark tui
```
*(Note: A Nerd Font is required to render icons properly).*

## Navigation

The dashboard is fully keyboard-driven with Vim-style motions:

- `j` / `k` (or `Up` / `Down`): Navigate lists and bookmarks.
- `Tab`: Cycle focus between different panes (e.g., from the sidebar to the main view).
- `[` / `]`: Switch tabs within a pane.
- `+` / `-`: Resize the current pane.
- `/`: Open the search bar (toggle between Full-Text and Semantic search).
- `o`: Open the currently selected bookmark in your configured editor.
- `?`: Toggle the context-aware help overlay.

## Search Modes

When you press `/`, you can search your bookmarks using two modes:
1. **FTS (Full-Text Search):** Uses SQLite FTS5 for exact text matches.
2. **Semantic Search:** Uses local vector embeddings (via `candle`) to find bookmarks by intent or meaning, without requiring an API key.

## Syncing (Push / Pull)
- `P` (Shift+P): Push and publish collections to a remote `codetours` server.
- `p`: Pull shared tours from the remote server.
