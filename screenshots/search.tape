# vhs tape: animated demo GIF — full-text & semantic search.
#
# Produces: codemark_tui_search.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Flow:
#   1. focus the search bar, type a query, and press Enter to run it (FTS, the
#      default mode)
#   2. toggle to Semantic mode with Ctrl+S and re-run by meaning
#   3. clear with double Esc, then run an intent-style query in Semantic mode
#
# Key map used (see crates/codemark-tui/src/browser):
#   s        focus the search bar (from any other pane)
#   <typing> build the query
#   Enter    run the search (it does NOT run live as you type)
#   Ctrl+S   cycle search mode: FTS <-> Semantic
#   Esc Esc  clear the query and leave the search bar (double Esc)
# Semantic search is slower than FTS, so allow extra time after Enter for it.

Output "__OUT__/codemark_tui_search.gif"

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"
Set Framerate 12

# Launch off-camera so the GIF starts on the loaded TUI, not the typing.
Hide
Type "codemark-tui"
Enter
Sleep 2.5s
Show

Sleep 500ms

# --- 1. Focus search, type a query, run it with Enter (FTS, the default) -----
Type "s"
Sleep 500ms
Type "authentication"
Sleep 600ms
Enter
Sleep 1.5s

# --- 2. Toggle to Semantic mode and re-run the same query by meaning ---------
Ctrl+S
Sleep 500ms
Enter
Sleep 2.5s

# --- 3. Clear with double Esc, then run an intent-style query (Semantic) -----
# Semantic mode persists, so this query is resolved by meaning.
Escape
Escape
Sleep 800ms
Type "s"
Sleep 400ms
Type "where do we resolve a bookmark after the code moves"
Sleep 600ms
Enter
Sleep 3s

Sleep 600ms
