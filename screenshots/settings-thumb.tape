# vhs tape: static thumbnail — the Settings overlay (Theme tab).
#
# Produces: codemark_tui_settings_thumb.png
#
# A single still used as the poster/thumbnail for the Settings demo in the
# demo gallery (dev-docs/guides/demo-gallery.md). Rendered by capture.sh, which
# substitutes __OUT__ with the absolute output dir and runs from the repo root
# so the TUI uses the tracked .codemark/codemark.db.
#
# Flow: open Settings (,) then advance to the Theme tab (]) and capture.

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

# Open Settings (Configuration tab) and move to the Theme tab.
Type ","
Sleep 700ms
Type "]"
Sleep 900ms

Screenshot "__OUT__/codemark_tui_settings_thumb.png"
