# vhs tape: static thumbnail — resizing and cycling pane layouts.
#
# Produces: codemark_tui_layout_thumb.png
#
# A single still used as the poster/thumbnail for the layout demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the repo root
# so the TUI uses the tracked .codemark/codemark.db.
#
# Flow: focus the preview pane (4), grow it to full width (+), capture.

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

# Focus the preview pane and grow it to full width for the poster frame.
Type "4"
Sleep 400ms
Type "+"
Sleep 1s

Screenshot "__OUT__/codemark_tui_layout_thumb.png"
