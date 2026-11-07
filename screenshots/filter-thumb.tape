# vhs tape: static thumbnail — filtering pane contents.
#
# Produces: codemark_tui_filter_thumb.png
#
# A single still used as the poster/thumbnail for the filtering demo in the demo
# gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the
# seeded fixture clone so the TUI uses the fixture DB (see seed-fixture.sh).
#
# Flow: switch pane 3 to Bookmarks (]), enter filter mode (/), type a
# substring, capture.

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

# Pane 3 starts on Collections; switch to Bookmarks, then filter it live.
Type "]"
Sleep 600ms
Type "/"
Sleep 400ms
Type "auth"
Sleep 1.2s

Screenshot "__OUT__/codemark_tui_filter_thumb.png"
