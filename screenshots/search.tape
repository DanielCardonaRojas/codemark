# vhs tape: animated demo GIF — full-text & semantic search.
#
# Produces: codemark_tui_search.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the repo root so the TUI uses the tracked .codemark/codemark.db.
#
# Flow:
#   1. focus the search bar and type a query (FTS mode, the default) — results
#      filter live as you type
#   2. toggle to Semantic mode with Ctrl+S — the same query is re-run by meaning
#   3. clear and try an intent-style query that semantic search resolves
#
# Key map used (see crates/codemark-tui/src/browser):
#   s        focus the search bar (from any other pane)
#   <typing> builds the query; search runs live
#   Ctrl+S   cycle search mode: FTS <-> Semantic
#   Esc      clear the query and leave the search bar

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

# --- 1. Focus search and type a query (FTS, the default) ---------------------
Type "s"
Sleep 500ms
Type "authentication"
Sleep 1.5s

# --- 2. Toggle to Semantic mode (re-runs the same query by meaning) ----------
Ctrl+S
Sleep 2s

# --- 3. Clear, then run an intent-style query (still in Semantic mode) -------
# Semantic mode persists from step 2, so this query is resolved by meaning.
Escape
Sleep 600ms
Type "s"
Sleep 400ms
Type "where do we resolve a bookmark after the code moves"
Sleep 2.5s

Sleep 600ms
