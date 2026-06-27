# vhs tape: static thumbnail — filtering pane contents.
#
# Produces: codemark_tui_filter_thumb.png
#
# A single still used as the poster/thumbnail for the filtering demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the repo root
# so the TUI uses the tracked .codemark/codemark.db.
#
# Flow: enter filter mode on the Bookmarks pane (/), type a substring, capture.

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

# Filter the Bookmarks pane (focused by default) — narrows the list live.
Type "/"
Sleep 400ms
Type "resolve"
Sleep 1.2s

Screenshot "__OUT__/codemark_tui_filter_thumb.png"
