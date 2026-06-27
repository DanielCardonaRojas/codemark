# vhs tape: static thumbnail — search results (Semantic mode).
#
# Produces: codemark_tui_search_thumb.png
#
# A single still used as the poster/thumbnail for the search demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the repo root
# so the TUI uses the tracked .codemark/codemark.db.
#
# Flow: focus search (s), toggle to Semantic mode (Ctrl+S), type a query, run it
# with Enter, wait for the (slower) semantic results, then capture.

Set FontFamily "SFMono Nerd Font"
Set FontSize 24
Set Width 2100
Set Height 1290
Set Padding 24
Set Theme "Catppuccin Mocha"

Hide
Type "codemark-tui"
Enter
Sleep 2.5s
Show

# Focus search, switch to Semantic mode, type a query, and run it with Enter.
Type "s"
Sleep 400ms
Ctrl+S
Sleep 400ms
Type "authentication"
Sleep 400ms
Enter
# Semantic search is slow — wait for results before capturing.
Sleep 2.5s

Screenshot "__OUT__/codemark_tui_search_thumb.png"
