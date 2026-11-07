# vhs tape: animated demo GIF — full-text & semantic search.
#
# Produces: codemark_tui_search.gif
#
# Rendered by capture.sh, which substitutes __OUT__ with the absolute output dir
# and runs from the seeded fixture clone so the TUI uses the fixture DB (see
# seed-fixture.sh), not the repo's live .codemark/codemark.db.
#
# Flow:
#   1. focus the search bar, type a query, and press Enter to run it (FTS, the
#      default mode)
#   2. refocus the bar, toggle to Semantic mode with Ctrl+S, and re-run by meaning
#   3. clear the bar, then run an intent-style query in Semantic mode
#
# Key map used (see crates/codemark-tui/src/browser):
#   s        focus the search bar (from any other pane)
#   <typing> build the query
#   Enter    run the search (it does NOT run live as you type) and move focus
#            to the results — so later keys go to the results pane until `s`
#   Ctrl+S   cycle search mode: FTS <-> Semantic (only while the bar is focused)
#   Esc      (bar focused) clear the query and leave the search bar
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

# --- 2. Refocus, toggle to Semantic mode, re-run the same query by meaning ---
# Enter moved focus to the results, so `s` first: Ctrl+S only applies to the
# focused search bar.
Type "s"
Sleep 400ms
Ctrl+S
Sleep 600ms
Enter
Sleep 2.5s

# --- 3. Clear, then run an intent-style query (Semantic) ---------------------
# `s` refocuses the bar; Esc there clears the query and leaves the bar; `s`
# again focuses the now-empty bar. Semantic mode persists across the clear.
Type "s"
Sleep 400ms
Escape
Sleep 700ms
Type "s"
Sleep 400ms
Type "where do we resolve a bookmark after the code moves"
Sleep 600ms
Enter
Sleep 3s

Sleep 600ms
